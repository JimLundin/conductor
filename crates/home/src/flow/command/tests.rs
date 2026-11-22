use proto::ProjectId;

use super::{Command, Context, New, Outcome, Part, command, run, stamp};
use crate::flow::check::{Inputs, check};
use crate::flow::graph::File;

const FLOW: &str = include_str!("../flow.toml");
const ROOT: &str = "e22b7f7b2976650400965d4147ba771675f96076";
const FOLDER: &str = "01m7w85jegh2zxfrxb37dadxnk";
const SPEC: &str = "01m7ztr3ha005c92e0ykb9q429";
const TASK: &str = "01m7zxfbp2qygtekcr6b6ndg28";
/// 2026-11-22T12:00:00Z.
const NOW: u64 = 1_795_348_800_000;

fn words(text: &str) -> Vec<String> {
    text.split(' ').map(String::from).collect()
}

fn issue(id: &str, front: &str, body: &str) -> File {
    File {
        project: ProjectId(String::from(FOLDER)),
        stem: String::from(id),
        text: format!("+++\nid = \"{id}\"\n{front}+++\n\n{body}"),
    }
}

fn home() -> Inputs {
    Inputs {
        flow: Ok(String::from(FLOW)),
        projects: Vec::from([(
            String::from(FOLDER),
            Ok(format!(
                "name = \"home\"\ntitle = \"Home\"\nroot = \"{ROOT}\"\n"
            )),
        )]),
        issues: Vec::from([
            issue(
                SPEC,
                "title = \"The flow\"\nkind = \"spec\"\nstage = \"spec\"\nstatus = \"open\"\n",
                "## Problem\n\n## Intended design\n\n## Acceptance\n\n## Notes\n\n\
                 - 2026-11-21T19:53:48Z laptop·claude note: written.\n",
            ),
            issue(
                TASK,
                &format!(
                    "title = \"Write it\"\nkind = \"task\"\nstatus = \"open\"\nparent = \"{SPEC}\"\n"
                ),
                "## What\n\n## Notes\n\n- 2026-11-21T20:41:27Z laptop·claude note: sliced.\n",
            ),
        ]),
    }
}

fn context() -> Context {
    Context {
        now: NOW,
        random: 7,
        by: String::from("laptop·claude"),
    }
}

/// The home with `outcome`'s writes made.
fn after(inputs: &Inputs, outcome: &Outcome) -> Inputs {
    let mut issues: Vec<File> = inputs
        .issues
        .iter()
        .filter(|file| {
            !outcome.writes.iter().any(|write| write.stem == file.stem)
        })
        .cloned()
        .collect();
    issues.extend(outcome.writes.iter().map(|write| File {
        project: write.project.clone(),
        stem: write.stem.clone(),
        text: write.text.clone(),
    }));
    Inputs {
        issues,
        ..inputs.clone()
    }
}

fn text(outcome: &Outcome) -> String {
    outcome
        .writes
        .first()
        .map_or_else(String::new, |write| write.text.clone())
}

#[test]
fn the_words_name_a_command() {
    assert_eq!(command(&words("check")), Command::Check);
    assert_eq!(
        command(&words("issue new home task Do-it parent=x")),
        Command::New(New {
            project: String::from("home"),
            kind: String::from("task"),
            title: String::from("Do-it"),
            fields: Vec::from([String::from("parent=x")]),
        })
    );
    assert_eq!(
        command(&words("issue move x stage slice signed off")),
        Command::Move {
            id: String::from("x"),
            part: Part::Stage,
            to: String::from("slice"),
            reason: String::from("signed off"),
        }
    );
    assert_eq!(
        command(&words("issue note x it works")),
        Command::Note {
            id: String::from("x"),
            text: String::from("it works"),
        }
    );
    assert_eq!(command(&words("issue set x")), Command::Usage);
    assert_eq!(command(&words("issue move x phase build")), Command::Usage);
}

#[test]
fn a_new_issue_gets_a_fresh_id_its_sections_and_a_log_that_check_accepts() {
    let inputs = home();
    let outcome = run(
        &command(&words(&format!(
            "issue new home task Wire-it parent={SPEC} blocked_by={TASK}"
        ))),
        &inputs,
        &context(),
    );
    assert!(!outcome.failed, "{}", outcome.output);
    let id = outcome
        .writes
        .first()
        .map_or_else(String::new, |write| write.stem.clone());
    assert_eq!(id.len(), 26);
    assert_eq!(
        text(&outcome),
        format!(
            "+++\nid = \"{id}\"\ntitle = \"Wire-it\"\nkind = \"task\"\nstatus = \"open\"\n\
             parent = \"{SPEC}\"\nblocked_by = [\"{TASK}\"]\ncreated = 2026-11-22T12:00:00Z\n+++\n\n\
             ## What\n\n## Acceptance\n\n## Notes\n\n\
             - 2026-11-22T12:00:00Z laptop·claude note: opened.\n"
        )
    );
    assert_eq!(outcome.output, format!("wrote home/{id}\n"));
    assert!(!check(&after(&inputs, &outcome)).failed());
}

#[test]
fn a_structural_problem_refuses_the_write() {
    let refused = |said: &str| {
        let outcome = run(&command(&words(said)), &home(), &context());
        (outcome.failed && outcome.writes.is_empty(), outcome.output)
    };
    let (dangling, output) =
        refused("issue new home task T parent=01m7zxfbnrxa7106qn4dx9f0jx");
    assert!(dangling);
    assert!(
        output.contains("parent: 01m7zxfbnrxa7106qn4dx9f0jx is no issue"),
        "{output}"
    );
    let (undeclared, said) = refused("issue new home task T severity=high");
    assert!(undeclared);
    assert!(
        said.contains("severity: is not a field or relation of \"task\""),
        "{said}"
    );
    let inputs = home();
    let blocking = run(
        &command(&words(&format!("issue set {SPEC} blocked_by={TASK}"))),
        &inputs,
        &context(),
    );
    assert!(!blocking.failed, "{}", blocking.output);
    let cycle = run(
        &command(&words(&format!("issue set {TASK} blocked_by={SPEC}"))),
        &after(&inputs, &blocking),
        &context(),
    );
    assert!(cycle.failed && cycle.writes.is_empty());
    assert!(
        cycle.output.contains("blocked_by: is part of a cycle"),
        "{}",
        cycle.output
    );
}

#[test]
fn a_missing_section_is_written_with_a_warning() {
    let outcome = run(
        &command(&words(&format!("issue note {TASK} checked"))),
        &home(),
        &context(),
    );
    assert!(!outcome.failed);
    assert_eq!(
        outcome.output,
        format!(
            "wrote home/{TASK}\nwarn   home/{TASK}  lacks the section \"Acceptance\"\n"
        )
    );
    assert!(text(&outcome).ends_with(
        "- 2026-11-21T20:41:27Z laptop·claude note: sliced.\n\
         - 2026-11-22T12:00:00Z laptop·claude note: checked\n"
    ));
}

#[test]
fn a_move_logs_where_from_and_why_and_marks_an_unusual_one() {
    let usual = run(
        &command(&words(&format!("issue move {SPEC} stage slice signed off"))),
        &home(),
        &context(),
    );
    assert!(text(&usual).contains("stage = \"slice\"\n"));
    assert!(text(&usual).ends_with(
        "- 2026-11-22T12:00:00Z laptop·claude stage spec→slice: signed off\n"
    ));
    let unusual = run(
        &command(&words(&format!(
            "issue move {SPEC} stage land skipping ahead"
        ))),
        &home(),
        &context(),
    );
    assert!(!unusual.failed);
    assert!(
        text(&unusual)
            .ends_with("stage spec→land: skipping ahead (unusual move)\n")
    );
    let status = run(
        &command(&words(&format!("issue move {TASK} status ready"))),
        &home(),
        &context(),
    );
    assert!(text(&status).contains("status = \"ready\"\n"));
    assert!(text(&status).ends_with("status open→ready: \n"));
    let unknown = run(
        &command(&words(&format!("issue move {TASK} status started"))),
        &home(),
        &context(),
    );
    assert!(
        unknown.failed
            && unknown.output.contains("no status is called started")
    );
}

#[test]
fn a_stage_or_status_is_moved_not_set() {
    let outcome = run(
        &command(&words(&format!("issue set {TASK} status=done"))),
        &home(),
        &context(),
    );
    assert!(outcome.failed && outcome.writes.is_empty());
}

#[test]
fn a_time_is_written_to_the_second_in_utc() {
    assert_eq!(stamp(NOW), "2026-11-22T12:00:00Z");
    assert_eq!(stamp(0), "1970-01-01T00:00:00Z");
}
