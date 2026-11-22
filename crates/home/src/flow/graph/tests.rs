use alloc::collections::BTreeMap;

use proto::{ChecklistItem, Datum, Entry, LogEvent, ProjectId, Section};

use super::{File, Graph, Problem, id_of, read};
use crate::flow::load;
use proto::flow::Flow;

const SPEC: &str = "01m7ztr3gtvsnpcy98b3pzdsy0";
const TASK: &str = "01m7zxfbnt3hsv50a6078gek37";
const DECISION: &str = "01m7zxfbnrxa7106qn4dx9f0jx";

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
        project: ProjectId(String::from(project)),
        stem: String::from(id),
        text: format!("+++\nid = \"{id}\"\n{front}+++\n\n{body}"),
    }
}

fn spec() -> File {
    file(
        "conductor",
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
                 issue,
                 field,
                 message,
             }| (issue.clone(), field.clone(), message.clone()),
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
            task("home", &format!("parent = \"{SPEC}\"\nblocked_by = []\n")),
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
            (String::from("parent"), id_of(SPEC).into_iter().collect()),
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
        issue: format!("conductor/{TASK}"),
        field: field.map(String::from),
        message: String::from("is missing"),
    };
    assert_eq!(at(Some("title")).to_string(), "title: is missing");
    assert_eq!(at(None).to_string(), "is missing");
}

#[test]
fn a_target_that_is_no_id_is_no_issue() {
    let graph = read(&flow(), &[task("home", "parent = \"nothing\"\n")]);
    assert_eq!(
        problems(&graph),
        Vec::from([problem(
            &format!("home/{TASK}"),
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
            "conductor",
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
fn a_reference_may_cross_projects_but_must_reach_an_issue_of_its_kind() {
    let crossing = read(
        &flow(),
        &[spec(), task("home", &format!("parent = \"{SPEC}\"\n"))],
    );
    assert_eq!(problems(&crossing), Vec::new());
    let dangling = read(
        &flow(),
        &[task("home", &format!("parent = \"{DECISION}\"\n"))],
    );
    assert_eq!(
        problems(&dangling),
        Vec::from([problem(
            &format!("home/{TASK}"),
            "parent",
            &format!("{DECISION} is no issue")
        )])
    );
    let decision = file(
        "home",
        DECISION,
        "title = \"Which?\"\nkind = \"decision\"\nstatus = \"open\"\n",
        "## Question\n",
    );
    let wrong = read(
        &flow(),
        &[
            decision,
            task("home", &format!("parent = \"{DECISION}\"\n")),
        ],
    );
    assert_eq!(
        problems(&wrong),
        Vec::from([problem(
            &format!("home/{TASK}"),
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
                "home",
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
            &format!("conductor/{SPEC}"),
            "status",
            "no status is called \"in-progress\"",
        )])
    );
}

#[test]
fn a_cycle_where_the_relation_forbids_one_is_a_problem() {
    let one = task("home", &format!("blocked_by = [\"{DECISION}\"]\n"));
    let other = file(
        "home",
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
                &format!("home/{member}"),
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
        let graph = read(&flow(), &[spec(), task("home", front)]);
        assert_eq!(
            problems(&graph),
            Vec::from([problem(&format!("home/{TASK}"), field, message)])
        );
    }
    let unknown = read(
        &flow(),
        &[file(
            "home",
            TASK,
            "title = \"T\"\nkind = \"chore\"\nstatus = \"open\"\n",
            "",
        )],
    );
    assert_eq!(
        problems(&unknown),
        Vec::from([problem(
            &format!("home/{TASK}"),
            "kind",
            "no kind is called \"chore\""
        )])
    );
    let renamed = read(
        &flow(),
        &[file(
            "home",
            TASK,
            "title = \"T\"\nstatus = \"in-progress\"\n",
            "",
        )],
    );
    assert_eq!(
        problems(&renamed),
        Vec::from([problem(
            &format!("home/{TASK}"),
            "status",
            "no status is called \"in-progress\""
        )])
    );
}

#[test]
fn an_id_is_its_file_s_name() {
    let mut moved = task("home", "");
    moved.stem = String::from(SPEC);
    assert_eq!(
        problems(&read(&flow(), &[moved])),
        Vec::from([problem(
            &format!("home/{SPEC}"),
            "id",
            "is not its file's name, a lower-case ULID",
        )])
    );
}
