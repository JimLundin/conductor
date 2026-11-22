use super::{Literal, Predicate, Quantifier, Role, Severity, Shape, load};

/// A copy of the home's `docs/agents/flow.toml`.
const FLOW: &str = include_str!("flow.toml");

/// The flow with `from` replaced by `to`, which it must contain.
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
fn a_field_has_a_known_type() {
    assert!(failure("type = \"texts\"", "type = \"tree\"").starts_with(
        "field[1].type: \"tree\" is not a field type; the types are enum"
    ));
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
    assert!(
        failure("name = \"intake\"\n", "name = \"intake\"\nskill = \"x\"\n")
            .starts_with("stage[0].skill: is not a key here")
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
        "field[0].default: \"meh\" is not one of its values"
    );
}
