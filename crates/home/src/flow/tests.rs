use crate::flow::{
    FieldRole, Literal, Predicate, Quantifier, Role, Severity, Shape,
};

use super::load;

const FLOW: &str = include_str!("test-flow.toml");

fn edited(from: &str, to: &str) -> Result<super::Flow, String> {
    assert!(FLOW.contains(from), "the flow has no {from:?}");
    load(&FLOW.replacen(from, to, 1))
}

fn failure(from: &str, to: &str) -> String {
    edited(from, to).err().unwrap_or_default()
}

#[test]
fn the_home_s_flow_loads_in_the_order_it_declares() {
    let loaded = load(FLOW).map(|flow| {
        (
            flow.default_kind.clone(),
            flow.statuses
                .iter()
                .map(|status| (status.name.clone(), status.role))
                .collect::<Vec<_>>(),
            flow.kinds
                .iter()
                .map(|kind| kind.name.clone())
                .collect::<Vec<_>>(),
            flow.stages
                .iter()
                .map(|stage| stage.name.clone())
                .collect::<Vec<_>>(),
            flow.relations
                .iter()
                .map(|relation| (relation.name.clone(), relation.shape))
                .collect::<Vec<_>>(),
            flow.rules.len(),
        )
    });
    let words = |names: &[&str]| {
        names
            .iter()
            .map(|name| String::from(*name))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        loaded,
        Ok((
            String::from("task"),
            [
                ("open", Role::Idle),
                ("ready", Role::Startable),
                ("active", Role::Active),
                ("review", Role::Review),
                ("done", Role::Terminal),
                ("dropped", Role::Dropped),
            ]
            .map(|(name, role)| (String::from(name), role))
            .to_vec(),
            words(&["spec", "task", "decision", "review", "bug"]),
            words(&[
                "intake", "shape", "spec", "slice", "build", "review", "land"
            ]),
            Vec::from([
                (String::from("parent"), Shape::Hierarchy),
                (String::from("blocked_by"), Shape::Dependency),
            ]),
            4,
        ))
    );
}

#[test]
fn a_rule_reads_as_composed_predicates() {
    let first = load(FLOW).map(|flow| flow.rules.first().cloned());
    let found = first.ok().flatten();
    assert_eq!(
        found
            .as_ref()
            .map(|rule| (rule.severity, &rule.when, &rule.require)),
        Some((
            Severity::Warn,
            &Predicate::All(Vec::from([
                Predicate::Kind(String::from("spec")),
                Predicate::StatusRole(Role::Terminal),
            ])),
            &Predicate::Across {
                quantifier: Quantifier::Any,
                via: String::from("children"),
                condition: Box::new(Predicate::All(Vec::from([
                    Predicate::Kind(String::from("review")),
                    Predicate::Field {
                        name: String::from("focus"),
                        value: Literal::Text(String::from("design")),
                    },
                    Predicate::StatusRole(Role::Terminal),
                ]))),
            },
        ))
    );
}

#[test]
fn a_relation_is_found_by_its_name_or_its_reverse() {
    let found = load(FLOW).map(|flow| {
        ["parent", "children", "blocks", "sibling"].map(|name| {
            flow.relation(name)
                .map(|(relation, reverse)| (relation.name.clone(), reverse))
        })
    });
    assert_eq!(
        found,
        Ok([
            Some((String::from("parent"), false)),
            Some((String::from("parent"), true)),
            Some((String::from("blocked_by"), true)),
            None,
        ])
    );
}

#[test]
fn a_relation_with_no_kinds_listed_joins_any_kind() {
    let joined = load(FLOW).map(|flow| {
        ["parent", "blocked_by"].map(|name| {
            flow.relation(name).map(|(relation, _)| {
                (
                    relation.starts_from("task"),
                    relation.starts_from("spec"),
                    relation.reaches("spec"),
                    relation.reaches("task"),
                )
            })
        })
    });
    assert_eq!(
        joined,
        Ok([
            Some((true, false, true, false)),
            Some((true, true, true, true)),
        ])
    );
}

#[test]
fn every_role_needs_a_status() {
    assert_eq!(
        failure(
            "[[status]]\nname = \"dropped\"\nrole = \"dropped\"\ncolour = \"grey\"\n",
            ""
        ),
        "flow: no status has the role \"dropped\""
    );
}

#[test]
fn a_relation_points_only_at_declared_kinds() {
    assert_eq!(
        failure("to = [\"spec\"]", "to = [\"epic\"]"),
        "relation[0].to[0]: no kind is called \"epic\""
    );
}

#[test]
fn a_field_has_a_known_type_and_a_reference_is_a_relation() {
    for word in ["tree", "ref", "refs"] {
        assert_eq!(
            failure("type = \"texts\"", &format!("type = \"{word}\"")),
            format!(
                "field[1]: unknown variant `{word}`, expected one of `enum`, \
                 `text`, `texts`, `time`, `bool`, in `type`"
            )
        );
    }
}

#[test]
fn a_default_is_a_value_of_its_field_s_type() {
    let typed = |default: &str| {
        failure(
            "name = \"branch\"\ntype = \"text\"\n",
            &format!(
                "name = \"branch\"\ntype = \"text\"\ndefault = {default}\n"
            ),
        )
    };
    assert_eq!(typed("true"), "field[2].default: is not text");
    assert_eq!(typed("[\"a\"]"), "field[2].default: is not text");
    assert_eq!(
        typed("3"),
        "field[2]: invalid type: integer `3`, expected text, a list of text, \
         true or false, or a time, in `default`"
    );
    assert_eq!(
        failure(
            "name = \"labels\"\ntype = \"texts\"\n",
            "name = \"labels\"\ntype = \"texts\"\ndefault = \"app\"\n"
        ),
        "field[1].default: is not a list of text"
    );
    assert!(
        edited(
            "name = \"created\"\ntype = \"time\"\n",
            "name = \"created\"\ntype = \"time\"\ndefault = 2026-11-22T00:00:00Z\n"
        )
        .is_ok()
    );
}

#[test]
fn a_rule_s_require_weighs_a_move_only_where_its_when_does() {
    for word in ["from", "to"] {
        assert_eq!(
            failure(
                "require = { has_section = \"Answer\" }",
                &format!(
                    "require = {{ {word} = {{ has_section = \"Answer\" }} }}"
                )
            ),
            format!(
                "rule[2].require.{word}: a rule's require weighs a move only where its when does"
            )
        );
    }
    assert_eq!(
        edited(
            "when = { all = [{ kind = \"decision\" }, { status_role = \"terminal\" }] }",
            "when = { from = { kind = \"decision\" } }"
        )
        .map(|flow| flow.rules.get(2).map(|rule| rule.when.clone())),
        Ok(Some(Predicate::From(Box::new(Predicate::Kind(String::from(
            "decision"
        ))))))
    );
}

#[test]
fn a_predicate_names_the_path_of_what_it_gets_wrong() {
    assert_eq!(
        failure(
            "{ field = { focus = \"design\" } }",
            "{ field = { focus = \"taste\" } }"
        ),
        "rule[0].require.any.where.all[1].field.focus: \"taste\" is not one of \
         its values"
    );
    assert!(
        failure("{ kind = \"review\" },\n", "{ colour = \"review\" },\n")
            .starts_with(
                "rule[0].require.any.where.all[0].colour: is not an atom"
            )
    );
    assert_eq!(
        failure("via = \"children\"", "via = \"siblings\""),
        "rule[0].require.any.via: no relation or reverse is called \"siblings\""
    );
    assert_eq!(
        failure(
            "{ has = \"parent\" }",
            "{ has = \"parent\", kind = \"spec\" }"
        ),
        "rule[3].require: a predicate has exactly one key"
    );
    assert_eq!(
        failure(
            "{ any = [{ kind = \"decision\" }, { kind = \"review\" }] }",
            "{ none = [{ kind = \"decision\" }] }"
        ),
        "rule[3].when.none: takes { via = <relation>, where = <predicate> }"
    );
}

#[test]
fn unknown_keys_and_duplicate_names_are_refused() {
    assert_eq!(
        failure("name = \"intake\"\n", "name = \"intake\"\nskill = \"x\"\n"),
        "stage[0]: unknown field `skill`, expected one of `name`, `skills`, \
         `next`, `sign_off`"
    );
    assert_eq!(
        failure("name = \"slice\"", "name = \"shape\""),
        "stage[3].name: another stage is also called \"shape\""
    );
    assert_eq!(
        failure("reverse = \"blocks\"", "reverse = \"children\""),
        "relation[0].reverse: \"children\" is already a relation's name or \
         reverse"
    );
}

#[test]
fn references_between_parts_must_resolve() {
    assert_eq!(
        failure("default_kind = \"task\"", "default_kind = \"chore\""),
        "default_kind: no kind is called \"chore\""
    );
    assert_eq!(
        failure("next = [\"slice\"]", "next = [\"slicing\"]"),
        "stage[2].next[0]: no stage is called \"slicing\""
    );
    assert_eq!(
        failure(
            "order = [\"method\", \"status\", \"parent\"]",
            "order = [\"method\", \"focus\"]"
        ),
        "kind[2].order[1]: \"focus\" is not a field or relation of \"decision\""
    );
    assert_eq!(
        failure("default = \"normal\"", "default = \"meh\""),
        "field[0].default: \"meh\" is not one of low, normal, high, urgent"
    );
}

#[test]
fn a_field_s_role_is_read_with_it() {
    let roles = load(FLOW).map(|flow| {
        FieldRole::ALL.map(|role| {
            flow.field_with("task", role)
                .map(|field| field.name.clone())
        })
    });
    assert_eq!(
        roles,
        Ok(["labels", "branch", "created"]
            .map(|name| Some(String::from(name))))
    );
}

#[test]
fn a_field_role_is_known_held_once_and_of_its_type() {
    assert_eq!(
        failure("role = \"labels\"", "role = \"tags\""),
        "field[1]: unknown variant `tags`, expected one of `labels`, `branch`, \
         `created`, in `role`"
    );
    assert_eq!(
        failure("role = \"created\"", "role = \"branch\""),
        "field[3].role: a field with the role \"branch\" has type text"
    );
    assert_eq!(
        failure(
            "name = \"created\"\ntype = \"time\"\nrole = \"created\"",
            "name = \"created\"\ntype = \"text\"\nrole = \"branch\""
        ),
        "field[3].role: field[2] also has the role \"branch\""
    );
}
