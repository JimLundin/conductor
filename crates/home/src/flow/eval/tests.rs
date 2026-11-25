use alloc::boxed::Box;
use alloc::collections::BTreeMap;

use crate::issue::{Datum, Issue, Section};
use crate::{IssueId, ProjectId};
use proptest::prelude::*;

use super::{Failure, Move, Step, holds};
use crate::flow::graph::Graph;
use crate::flow::load;
use crate::flow::{Flow, Literal, Predicate, Quantifier, Role};

fn flow() -> Flow {
    load(include_str!("../test-flow.toml")).unwrap_or_else(|_| Flow {
        default_kind: String::new(),
        statuses: Vec::new(),
        fields: Vec::new(),
        kinds: Vec::new(),
        relations: Vec::new(),
        stages: Vec::new(),
        rules: Vec::new(),
    })
}

fn id(name: &str) -> IssueId {
    IssueId {
        authority: String::from("test"),
        key: String::from(name),
    }
}

fn step(via: &str, name: &str) -> Step {
    Step {
        via: String::from(via),
        issue: id(name),
    }
}

fn terminal() -> Predicate {
    Predicate::StatusRole(Role::Terminal)
}

fn issue(name: &str, kind: &str, status: &str) -> Issue {
    Issue {
        created: None,
        id: id(name),
        project: ProjectId::new(1),
        title: String::from(name),
        kind: String::from(kind),
        status: String::from(status),
        stage: None,
        fields: BTreeMap::new(),
        links: BTreeMap::new(),
        body: String::new(),
        sections: Vec::new(),
        checklist: Vec::new(),
        log: Vec::new(),
        notes: Vec::new(),
        branch: String::new(),
    }
}

fn linked(mut issue: Issue, relation: &str, ids: &[&str]) -> Issue {
    issue.links.insert(
        String::from(relation),
        ids.iter().map(|target| id(target)).collect(),
    );
    issue
}

fn among(graph: &Graph) -> Vec<&Issue> {
    graph.issues.iter().collect()
}

fn rule(flow: &Flow, index: usize) -> Predicate {
    flow.rules
        .get(index)
        .map_or(Predicate::All(Vec::new()), |rule| rule.require.clone())
}

#[test]
fn a_done_spec_needs_a_done_design_review_among_its_children() {
    let flow = flow();
    let spec = issue("spec", "spec", "done");
    let mut review =
        linked(issue("review", "review", "active"), "parent", &["spec"]);
    review
        .fields
        .insert(String::from("focus"), Datum::Text(String::from("design")));
    let graph = Graph {
        issues: Vec::from([spec.clone(), review.clone()]),
        problems: Vec::new(),
    };
    assert_eq!(
        holds(
            &flow,
            &among(&graph),
            &spec,
            Move::default(),
            &rule(&flow, 0)
        ),
        Err(Failure {
            predicate: terminal(),
            issue: id("review"),
            path: Vec::from([step("children", "review")]),
        })
    );
    review.status = String::from("done");
    let finished = Graph {
        issues: Vec::from([spec.clone(), review]),
        problems: Vec::new(),
    };
    assert_eq!(
        holds(
            &flow,
            &among(&finished),
            &spec,
            Move::default(),
            &rule(&flow, 0)
        ),
        Ok(())
    );
    let alone = Graph {
        issues: Vec::from([spec.clone()]),
        problems: Vec::new(),
    };
    assert_eq!(
        holds(
            &flow,
            &among(&alone),
            &spec,
            Move::default(),
            &rule(&flow, 0)
        ),
        Err(Failure {
            predicate: rule(&flow, 0),
            issue: id("spec"),
            path: Vec::new(),
        })
    );
}

#[test]
fn blocking_holds_until_every_blocker_is_built_and_ends_on_a_cycle() {
    let flow = flow();
    let one = linked(issue("one", "task", "active"), "blocked_by", &["two"]);
    let two = linked(issue("two", "task", "open"), "blocked_by", &["one"]);
    let graph = Graph {
        issues: Vec::from([one.clone(), two]),
        problems: Vec::new(),
    };
    let standing = Move {
        from: Some(&one),
        to: Some(&one),
    };
    assert_eq!(
        holds(&flow, &among(&graph), &one, standing, &rule(&flow, 1)),
        Err(Failure {
            predicate: Predicate::StatusRole(Role::Review),
            issue: id("two"),
            path: Vec::from([step("blocked_by", "two")]),
        })
    );
}

#[test]
fn a_move_is_weighed_from_and_to() {
    let flow = flow();
    let mut before = issue("spec", "spec", "open");
    before.stage = Some(String::from("spec"));
    let mut after = before.clone();
    after.stage = Some(String::from("slice"));
    let graph = Graph::default();
    let moving = Move {
        from: Some(&before),
        to: Some(&after),
    };
    let leaving = Predicate::All(Vec::from([
        Predicate::From(Box::new(Predicate::Stage(String::from("spec")))),
        Predicate::To(Box::new(Predicate::Stage(String::from("slice")))),
    ]));
    assert_eq!(
        holds(&flow, &among(&graph), &after, moving, &leaving),
        Ok(())
    );
    assert_eq!(
        holds(&flow, &among(&graph), &after, Move::default(), &leaving),
        Err(Failure {
            predicate: Predicate::From(Box::new(Predicate::Stage(
                String::from("spec")
            ))),
            issue: id("spec"),
            path: Vec::new(),
        })
    );
}

#[test]
fn a_field_left_out_has_its_default() {
    let flow = flow();
    let task = issue("task", "task", "open");
    let normal = Predicate::Field {
        name: String::from("priority"),
        value: Literal::Text(String::from("normal")),
    };
    assert_eq!(holds(&flow, &[], &task, Move::default(), &normal), Ok(()));
}

#[test]
fn a_bool_time_or_text_default_matches_on_its_typed_value() {
    let typed = load(&include_str!("../test-flow.toml").replacen(
        "[[kind]]",
        "[[field]]\nname = \"flagged\"\ntype = \"bool\"\ndefault = true\n\n\
         [[field]]\nname = \"due\"\ntype = \"time\"\ndefault = 2026-11-22T00:00:00Z\n\n\
         [[field]]\nname = \"note\"\ntype = \"text\"\ndefault = \"none\"\n\n[[kind]]",
        1,
    ));
    assert!(typed.is_ok(), "{typed:?}");
    let Ok(flow) = typed else {
        return;
    };
    let task = issue("task", "task", "open");
    let field = |name: &str, value: Literal| Predicate::Field {
        name: String::from(name),
        value,
    };
    let text = |word: &str| Literal::Text(String::from(word));
    for (predicate, meets) in [
        (field("flagged", Literal::Bool(true)), true),
        (field("flagged", Literal::Bool(false)), false),
        (field("due", text("2026-11-22T00:00:00Z")), true),
        (field("due", text("2026-11-23T00:00:00Z")), false),
        (field("note", text("none")), true),
        (field("note", text("true")), false),
    ] {
        assert_eq!(
            holds(&flow, &[], &task, Move::default(), &predicate).is_ok(),
            meets,
            "{predicate:?}"
        );
    }
}

fn meets(
    flow: &Flow,
    graph: &Graph,
    issue: &Issue,
    moving: Move<'_>,
    predicate: &Predicate,
) -> bool {
    let along = |via: &str| -> Vec<&Issue> {
        let Some((relation, reverse)) = flow.relation(via) else {
            return Vec::new();
        };
        graph
            .issues
            .iter()
            .filter(|other| {
                let (from, to) = if reverse {
                    (*other, issue)
                } else {
                    (issue, *other)
                };
                from.links
                    .get(&relation.name)
                    .is_some_and(|ids| ids.contains(&to.id))
            })
            .collect()
    };
    match predicate {
        Predicate::Kind(kind) => issue.kind == *kind,
        Predicate::Stage(stage) => {
            issue.stage.as_deref() == Some(stage.as_str())
        }
        Predicate::StatusRole(role) => {
            flow.status(&issue.status).map(|status| status.role) == Some(*role)
        }
        Predicate::Field { name, value } => {
            let default = flow
                .field(&issue.kind, name)
                .and_then(|field| field.default.clone());
            match (issue.fields.get(name).cloned().or(default), value) {
                (Some(Datum::Text(text)), Literal::Text(wanted)) => {
                    text == *wanted
                }
                (Some(Datum::Texts(texts)), Literal::Text(wanted)) => {
                    texts.contains(wanted)
                }
                (Some(Datum::Bool(flag)), Literal::Bool(wanted)) => {
                    flag == *wanted
                }
                _ => false,
            }
        }
        Predicate::Has(name) => {
            (name == "stage" && issue.stage.is_some())
                || issue.fields.contains_key(name)
                || issue.links.get(name).is_some_and(|ids| !ids.is_empty())
        }
        Predicate::HasSection(section) => {
            issue.sections.iter().any(|had| had.heading == *section)
        }
        Predicate::From(inner) => moving
            .from
            .is_some_and(|from| meets(flow, graph, from, moving, inner)),
        Predicate::To(inner) => moving
            .to
            .is_some_and(|to| meets(flow, graph, to, moving, inner)),
        Predicate::All(parts) => parts
            .iter()
            .all(|part| meets(flow, graph, issue, moving, part)),
        Predicate::Any(parts) => parts
            .iter()
            .any(|part| meets(flow, graph, issue, moving, part)),
        Predicate::Not(inner) => !meets(flow, graph, issue, moving, inner),
        Predicate::Across {
            quantifier,
            via,
            condition,
        } => {
            let reached = along(via);
            let each =
                |target: &&Issue| meets(flow, graph, target, moving, condition);
            match quantifier {
                Quantifier::Any => reached.iter().any(each),
                Quantifier::All => reached.iter().all(each),
                Quantifier::None => !reached.iter().any(each),
            }
        }
    }
}

const IDS: [&str; 5] = ["i0", "i1", "i2", "i3", "i4"];
const KINDS: [&str; 5] = ["spec", "task", "decision", "review", "bug"];
const STATUSES: [&str; 6] =
    ["open", "ready", "active", "review", "done", "dropped"];
const STAGES: [&str; 7] = [
    "intake", "shape", "spec", "slice", "build", "review", "land",
];

fn pick<const N: usize>(
    words: [&'static str; N],
) -> impl Strategy<Value = String> {
    prop::sample::select(words.to_vec()).prop_map(String::from)
}

fn issues() -> impl Strategy<Value = Vec<Issue>> {
    let one = (
        pick(KINDS),
        pick(STATUSES),
        prop::option::of(pick(STAGES)),
        prop::option::of(pick(["low", "normal", "high"])),
        prop::option::of(pick(["code", "design"])),
        prop::option::of(pick(IDS)),
        prop::collection::vec(pick(IDS), 0..3),
        prop::collection::vec(pick(["What", "Answer", "Findings"]), 0..3),
    );
    prop::collection::vec(one, 1..=IDS.len()).prop_map(|drawn| {
        drawn
            .into_iter()
            .zip(IDS)
            .map(
                |(
                    (
                        kind,
                        status,
                        stage,
                        priority,
                        focus,
                        parent,
                        blockers,
                        sections,
                    ),
                    id,
                )| {
                    let mut made = issue(id, &kind, &status);
                    made.stage = stage;
                    made.fields.extend(priority.map(|word| {
                        (String::from("priority"), Datum::Text(word))
                    }));
                    made.fields.extend(focus.map(|word| {
                        (String::from("focus"), Datum::Text(word))
                    }));
                    made.links.extend(parent.map(|target| {
                        (String::from("parent"), Vec::from([self::id(&target)]))
                    }));
                    made.links.insert(
                        String::from("blocked_by"),
                        blockers
                            .iter()
                            .map(|target| self::id(target))
                            .collect(),
                    );
                    made.sections = sections
                        .into_iter()
                        .map(|heading| Section {
                            heading,
                            text: String::new(),
                        })
                        .collect();
                    made
                },
            )
            .collect()
    })
}

fn atoms() -> impl Strategy<Value = Predicate> {
    prop_oneof![
        pick(KINDS).prop_map(Predicate::Kind),
        pick(STAGES).prop_map(Predicate::Stage),
        prop::sample::select(Role::ALL.to_vec())
            .prop_map(Predicate::StatusRole),
        (
            pick(["priority", "focus"]),
            pick(["normal", "high", "design", "code"])
        )
            .prop_map(|(name, word)| Predicate::Field {
                name,
                value: Literal::Text(word)
            }),
        pick(["parent", "blocked_by", "stage", "priority", "focus"])
            .prop_map(Predicate::Has),
        pick(["What", "Answer"]).prop_map(Predicate::HasSection),
    ]
}

fn predicates() -> impl Strategy<Value = Predicate> {
    atoms().prop_recursive(4, 32, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..3).prop_map(Predicate::All),
            prop::collection::vec(inner.clone(), 0..3).prop_map(Predicate::Any),
            inner
                .clone()
                .prop_map(|part| Predicate::Not(Box::new(part))),
            inner
                .clone()
                .prop_map(|part| Predicate::From(Box::new(part))),
            inner.clone().prop_map(|part| Predicate::To(Box::new(part))),
            (
                prop::sample::select(Vec::from([
                    Quantifier::Any,
                    Quantifier::All,
                    Quantifier::None
                ])),
                pick(["parent", "children", "blocked_by", "blocks"]),
                inner,
            )
                .prop_map(|(quantifier, via, condition)| {
                    Predicate::Across {
                        quantifier,
                        via,
                        condition: Box::new(condition),
                    }
                }),
        ]
    })
}

proptest! {
    #[test]
    fn evaluation_agrees_with_the_meaning_and_explains_each_failure(
        drawn in issues(),
        predicate in predicates(),
        at in 0..IDS.len(),
        weighed in any::<bool>(),
    ) {
        let flow = flow();
        let graph = Graph { issues: drawn, problems: Vec::new() };
        let Some(subject) = graph.issues.get(at % graph.issues.len()) else {
            return Ok(());
        };
        let moving = if weighed {
            Move { from: graph.issues.first(), to: Some(subject) }
        } else {
            Move::default()
        };
        let evaluated = holds(&flow, &among(&graph), subject, moving, &predicate);
        prop_assert_eq!(evaluated.is_ok(), meets(&flow, &graph, subject, moving, &predicate));
        if let Err(failure) = evaluated {
            let failed = graph.issues.iter().find(|issue| issue.id == failure.issue);
            let fails_there = failed.is_some_and(|issue| {
                !meets(&flow, &graph, issue, moving, &failure.predicate)
            });
            prop_assert!(fails_there);
            for step in &failure.path {
                prop_assert!(flow.relation(&step.via).is_some());
                prop_assert!(graph.issues.iter().any(|issue| issue.id == step.issue));
            }
        }
    }
}
