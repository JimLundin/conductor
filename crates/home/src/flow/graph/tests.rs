use alloc::collections::BTreeMap;

use crate::issue::{ChecklistItem, Datum, Entry, LogEvent, Section};
use crate::{IssueId, ProjectId, UnixMillis};

use super::{Graph, Problem};
use crate::flow::Flow;
use crate::flow::load;
use crate::issue::storage::{File, one};

const SPEC: &str = "0tm7zt00";
const TASK: &str = "0tm7zx0b";
const DECISION: &str = "0tm7zx0a";
const HOME: &str = "hme000";
const CONDUCTOR: &str = "cnd000";

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

fn file(project: &str, id: &str, front: &str, body: &str) -> File {
    File {
        project: ProjectId::parse(project).unwrap_or(ProjectId::new(0)),
        stem: String::from(id),
        text: format!("+++\nid = \"{id}\"\n{front}+++\n\n{body}"),
    }
}

fn spec() -> File {
    file(
        CONDUCTOR,
        SPEC,
        "title = \"Interpret the flow\"\nkind = \"spec\"\nstage = \"build\"\n\
         status = \"active\"\npriority = \"high\"\nlabels = [\"home\"]\n\
         created = 2026-11-21T19:53:48Z\n",
        "Opening words.\n\n## Problem\n\nWhy.\n\n```\n## not a heading\n```\n\n\
         ## Acceptance\n\n- It works.\n\n## Checklist\n\n- [x] the loader\n\
         - [ ] the reader\n\n## Notes\n\n\
         - 2026-11-21T19:53:48Z laptop·claude stage shape→spec: shaped.\n\
         - 2026-11-18 laptop · codex · api: an older note\n  that goes on.\n\
         - 2026-11-22T10:00:00Z laptop·claude note: checked.\n",
    )
}

fn task(project: &str, front: &str) -> File {
    file(
        project,
        TASK,
        &format!(
            "title = \"Load the flow\"\nkind = \"task\"\nstatus = \"done\"\n{front}"
        ),
        "## What\n\nLoad it.\n",
    )
}

fn problems(graph: &Graph) -> Vec<(String, Option<String>, String)> {
    graph
        .problems
        .iter()
        .map(
            |Problem {
                 project,
                 record,
                 field,
                 message,
             }| {
                (
                    format!("{project}/{record}"),
                    field.clone(),
                    message.clone(),
                )
            },
        )
        .collect()
}

fn problem(
    issue: &str,
    field: &str,
    message: &str,
) -> (String, Option<String>, String) {
    (
        String::from(issue),
        Some(String::from(field)),
        String::from(message),
    )
}

#[test]
fn an_issue_reads_its_fields_links_sections_and_log() {
    let graph = read(
        &flow(),
        &[
            spec(),
            task(
                CONDUCTOR,
                &format!("parent = \"{SPEC}\"\nblocked_by = []\n"),
            ),
        ],
    );
    assert_eq!(problems(&graph), Vec::new());
    let first = graph.issues.first();
    assert_eq!(
        first.map(|issue| (
            issue.id.to_string(),
            issue.kind.as_str(),
            issue.status.as_str(),
            issue.stage.as_deref(),
            &issue.fields,
            &issue.sections,
        )),
        Some((
            String::from(SPEC),
            "spec",
            "active",
            Some("build"),
            &BTreeMap::from([
                (
                    String::from("created"),
                    Datum::Text(String::from("2026-11-21T19:53:48Z"))
                ),
                (
                    String::from("labels"),
                    Datum::Texts(Vec::from([String::from("home")]))
                ),
                (String::from("priority"), Datum::Text(String::from("high"))),
            ]),
            &[
                ("", "Opening words."),
                ("Problem", "Why.\n\n```\n## not a heading\n```"),
                ("Acceptance", "- It works."),
            ]
            .map(|(heading, text)| Section {
                heading: String::from(heading),
                text: String::from(text),
            })
            .to_vec(),
        ))
    );
    assert_eq!(
        first.map(|issue| issue.checklist.clone()),
        Some(Vec::from([
            ChecklistItem {
                done: true,
                text: String::from("the loader"),
            },
            ChecklistItem {
                done: false,
                text: String::from("the reader"),
            },
        ]))
    );
    assert_eq!(
        first.map(|issue| issue.log.clone()),
        Some(Vec::from([
            Entry {
                at: Some(String::from("2026-11-21T19:53:48Z")),
                by: Some(String::from("laptop·claude")),
                event: LogEvent::Stage {
                    from: String::from("shape"),
                    to: String::from("spec")
                },
                text: String::from("shaped."),
            },
            Entry {
                at: None,
                by: None,
                event: LogEvent::Note,
                text: String::from(
                    "2026-11-18 laptop · codex · api: an older note\nthat goes on."
                ),
            },
            Entry {
                at: Some(String::from("2026-11-22T10:00:00Z")),
                by: Some(String::from("laptop·claude")),
                event: LogEvent::Note,
                text: String::from("checked."),
            },
        ]))
    );
    assert_eq!(
        graph.issues.get(1).map(|issue| issue.links.clone()),
        Some(BTreeMap::from([
            (String::from("blocked_by"), Vec::new()),
            (
                String::from("parent"),
                IssueId::new(SPEC).into_iter().collect()
            ),
        ]))
    );
}

#[test]
fn an_issue_keeps_its_body_before_the_checklist_and_the_log() {
    let graph = read(&flow(), &[spec()]);
    assert_eq!(
        graph.issues.first().map(|issue| issue.body.as_str()),
        Some(
            "Opening words.\n\n## Problem\n\nWhy.\n\n```\n## not a heading\n```\n\n\
             ## Acceptance\n\n- It works."
        )
    );
}

#[test]
fn a_problem_says_what_is_wrong_after_the_field_that_causes_it() {
    let at = |field: Option<&str>| Problem {
        project: ProjectId::new(0),
        record: String::from(TASK),
        field: field.map(String::from),
        message: String::from("is missing"),
    };
    assert_eq!(at(Some("title")).to_string(), "title: is missing");
    assert_eq!(at(None).to_string(), "is missing");
}

#[test]
fn a_target_that_is_no_id_is_no_issue() {
    let graph = read(&flow(), &[task(HOME, "parent = \"nothing\"\n")]);
    assert_eq!(
        problems(&graph),
        Vec::from([problem(
            &format!("{HOME}/{TASK}"),
            "parent",
            "nothing is no issue"
        )])
    );
}

#[test]
fn an_issue_without_a_kind_reads_as_the_default_kind() {
    let graph = read(
        &flow(),
        &[file(
            CONDUCTOR,
            TASK,
            "title = \"Old\"\nstatus = \"open\"\n",
            "Body.\n",
        )],
    );
    assert_eq!(
        graph.issues.first().map(|issue| issue.kind.as_str()),
        Some("task")
    );
}

#[test]
fn a_reference_must_reach_an_issue_of_its_project_and_its_kind() {
    let crossing = read(
        &flow(),
        &[spec(), task(HOME, &format!("parent = \"{SPEC}\"\n"))],
    );
    assert_eq!(
        problems(&crossing),
        Vec::from([problem(
            &format!("{HOME}/{TASK}"),
            "parent",
            &format!("{SPEC} is no issue")
        )])
    );
    let dangling = read(
        &flow(),
        &[task(HOME, &format!("parent = \"{DECISION}\"\n"))],
    );
    assert_eq!(
        problems(&dangling),
        Vec::from([problem(
            &format!("{HOME}/{TASK}"),
            "parent",
            "0tm7zx0a is no issue"
        )])
    );
    let decision = file(
        HOME,
        DECISION,
        "title = \"Which?\"\nkind = \"decision\"\nstatus = \"open\"\n",
        "## Question\n",
    );
    let wrong = read(
        &flow(),
        &[decision, task(HOME, &format!("parent = \"{DECISION}\"\n"))],
    );
    assert_eq!(
        problems(&wrong),
        Vec::from([problem(
            &format!("{HOME}/{TASK}"),
            "parent",
            &format!("{DECISION} is a decision, which parent can't point to"),
        )])
    );
}

#[test]
fn a_reference_to_an_issue_that_has_its_own_problem_is_left_to_it() {
    let mut broken = spec();
    broken.text = broken
        .text
        .replace("status = \"active\"", "status = \"in-progress\"");
    let graph = read(
        &flow(),
        &[
            broken,
            task(
                CONDUCTOR,
                &format!(
                    "parent = \"{SPEC}\"
"
                ),
            ),
        ],
    );
    assert_eq!(
        problems(&graph),
        Vec::from([problem(
            &format!("{CONDUCTOR}/{SPEC}"),
            "status",
            "no status is called \"in-progress\"",
        )])
    );
}

#[test]
fn a_cycle_where_the_relation_forbids_one_is_a_problem() {
    let one = task(HOME, &format!("blocked_by = [\"{DECISION}\"]\n"));
    let other = file(
        HOME,
        DECISION,
        &format!(
            "title = \"Which?\"\nkind = \"decision\"\nstatus = \"open\"\nblocked_by = [\"{TASK}\"]\n"
        ),
        "## Question\n",
    );
    let graph = read(&flow(), &[one, other]);
    assert_eq!(
        problems(&graph),
        [TASK, DECISION]
            .map(|member| problem(
                &format!("{HOME}/{member}"),
                "blocked_by",
                &format!(
                    "is part of a cycle: {DECISION} → {TASK} → {DECISION}"
                ),
            ))
            .to_vec()
    );
}

#[test]
fn an_undeclared_key_or_value_is_a_problem_naming_its_field() {
    let cases = [
        (
            "severity = \"high\"\n",
            "severity",
            "is not a field or relation of \"task\"",
        ),
        (
            "priority = \"someday\"\n",
            "priority",
            "\"someday\" is not one of low, normal, high, urgent",
        ),
        (
            &format!("parent = [\"{SPEC}\"]\n") as &str,
            "parent",
            "points to one issue, not a list",
        ),
        (
            "focus = \"design\"\n",
            "focus",
            "is not a field or relation of \"task\"",
        ),
    ];
    for (front, field, message) in cases {
        let graph = read(&flow(), &[spec(), task(HOME, front)]);
        assert_eq!(
            problems(&graph),
            Vec::from([problem(&format!("{HOME}/{TASK}"), field, message)])
        );
    }
    let unknown = read(
        &flow(),
        &[file(
            HOME,
            TASK,
            "title = \"T\"\nkind = \"chore\"\nstatus = \"open\"\n",
            "",
        )],
    );
    assert_eq!(
        problems(&unknown),
        Vec::from([problem(
            &format!("{HOME}/{TASK}"),
            "kind",
            "no kind is called \"chore\""
        )])
    );
    let renamed = read(
        &flow(),
        &[file(
            HOME,
            TASK,
            "title = \"T\"\nstatus = \"in-progress\"\n",
            "",
        )],
    );
    assert_eq!(
        problems(&renamed),
        Vec::from([problem(
            &format!("{HOME}/{TASK}"),
            "status",
            "no status is called \"in-progress\""
        )])
    );
}

#[test]
fn an_id_is_its_file_s_name() {
    let mut moved = task(HOME, "");
    moved.stem = String::from(SPEC);
    assert_eq!(
        problems(&read(&flow(), &[moved])),
        Vec::from([problem(
            &format!("{HOME}/{SPEC}"),
            "id",
            "is not its file's name, an issue id",
        )])
    );
}

fn flow_with(edits: &[(&str, &str)]) -> Result<Flow, String> {
    let text = edits.iter().fold(
        String::from(include_str!("../test-flow.toml")),
        |text, (from, to)| {
            assert!(text.contains(from), "the flow has no {from:?}");
            text.replace(from, to)
        },
    );
    load(&text)
}

#[test]
fn an_issue_lands_on_the_branch_its_branch_field_names_whatever_it_is_called()
-> Result<(), String> {
    let renamed = flow_with(&[
        ("name = \"branch\"", "name = \"lands_on\""),
        ("\"branch\"]", "\"lands_on\"]"),
    ])?;
    let graph = read(&renamed, &[task(HOME, "lands_on = \"feature/x\"\n")]);
    assert_eq!(
        graph.issues.first().map(|issue| issue.branch.as_str()),
        Some("feature/x")
    );
    let roleless = flow_with(&[("role = \"branch\"\n", "")])?;
    let graph = read(&roleless, &[task(HOME, "branch = \"feature/x\"\n")]);
    assert_eq!(
        graph.issues.first().map(|issue| issue.branch.clone()),
        Some(String::from("issue/0tm7zx0b"))
    );
    Ok(())
}

#[test]
fn two_projects_can_each_hold_an_issue_with_one_id() {
    let graph = read(&flow(), &[task(HOME, ""), task(CONDUCTOR, "")]);
    assert_eq!(problems(&graph), Vec::new());
    assert_eq!(graph.issues.len(), 2);
}

fn read(flow: &Flow, files: &[File]) -> Graph {
    super::read(
        flow,
        &files.iter().map(|file| one(flow, file)).collect::<Vec<_>>(),
    )
}

#[test]
fn creation_time_orders_issues_independently_of_their_identity() {
    let graph = read(
        &flow(),
        &[
            file(
                HOME,
                TASK,
                "title = \"Earlier\"\nkind = \"task\"\nstatus = \"open\"\ncreated = 2025-01-01T00:00:00Z\n",
                "",
            ),
            spec(),
        ],
    );
    assert_eq!(
        graph.issues.first().map(|issue| issue.title.as_str()),
        Some("Earlier")
    );
}

#[test]
fn creation_order_compares_instants_with_offsets_and_fractional_seconds() {
    let graph = read(
        &flow(),
        &[
            file(
                HOME,
                TASK,
                "title = \"Earlier\"\nstatus = \"open\"\ncreated = 2026-01-01T01:00:00+01:00\n",
                "",
            ),
            file(
                HOME,
                SPEC,
                "title = \"Later\"\nstatus = \"open\"\ncreated = 2026-01-01T00:00:00.500Z\n",
                "",
            ),
        ],
    );
    assert_eq!(
        graph
            .issues
            .iter()
            .map(|issue| (&*issue.title, issue.created))
            .collect::<Vec<_>>(),
        [
            ("Earlier", Some(UnixMillis(1_767_225_600_000))),
            ("Later", Some(UnixMillis(1_767_225_600_500)))
        ]
    );
}

#[test]
fn an_invalid_creation_time_remains_an_invalid_record() {
    let graph = read(
        &flow(),
        &[file(
            HOME,
            TASK,
            "title = \"Invalid date\"\nstatus = \"open\"\ncreated = \"yesterday\"\n",
            "",
        )],
    );
    assert_eq!(graph.issues.len(), 0);
    assert_eq!(graph.problems.len(), 1);
}
