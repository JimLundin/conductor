use proto::{
    ChecklistChange, ChecklistItem, Issue, IssueChange, Item, NewIssue, Part,
    ProjectId,
};

use super::{Command, Context, Outcome, command, fresh, run, stamp};
use crate::flow::check::{Inputs, check};
use crate::flow::file::log_line;
use crate::flow::graph::{File, one};
use crate::flow::load;

const FLOW: &str = include_str!("../test-flow.toml");
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
        id: fresh(NOW, 7),
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
        Command::New(NewIssue {
            project: String::from("home"),
            kind: String::from("task"),
            title: String::from("Do-it"),
            fields: Vec::from([String::from("parent=x")]),
        })
    );
    let changing = |change| Command::Change {
        id: String::from("x"),
        changes: Vec::from([change]),
    };
    assert_eq!(
        command(&words("issue move x stage slice signed off")),
        changing(IssueChange::Move {
            part: Part::Stage,
            to: String::from("slice"),
            reason: String::from("signed off"),
        })
    );
    assert_eq!(
        command(&words("issue note x it works")),
        changing(IssueChange::Note(String::from("it works")))
    );
    assert_eq!(
        command(&words("issue set x a=1 b=2")),
        Command::Change {
            id: String::from("x"),
            changes: Vec::from([
                IssueChange::Set {
                    field: String::from("a"),
                    value: String::from("1"),
                },
                IssueChange::Set {
                    field: String::from("b"),
                    value: String::from("2"),
                },
            ]),
        }
    );
    assert_eq!(
        command(&words("issue set x a")),
        Command::Wrong(String::from("\"a\" is not <field>=<value>"))
    );
    assert_eq!(
        command(&words("issue body x Why it matters.")),
        changing(IssueChange::Body(String::from("Why it matters.")))
    );
    assert_eq!(
        command(&words("issue checklist x add the docs")),
        changing(IssueChange::Checklist(ChecklistChange::Add(String::from(
            "the docs"
        ))))
    );
    let first = || Item {
        index: 0,
        seen: None,
    };
    assert_eq!(
        [
            command(&words("issue checklist x check 1")),
            command(&words("issue checklist x uncheck 1")),
            command(&words("issue checklist x remove 1")),
        ],
        [
            changing(IssueChange::Checklist(ChecklistChange::Check {
                item: first(),
                done: true,
            })),
            changing(IssueChange::Checklist(ChecklistChange::Check {
                item: first(),
                done: false,
            })),
            changing(IssueChange::Checklist(ChecklistChange::Remove(first()))),
        ]
    );
    assert_eq!(
        command(&words("issue checklist x check 0")),
        Command::Wrong(String::from(
            "0 is not an item's number, counted from 1"
        ))
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

#[test]
fn a_body_is_replaced_and_a_checklist_item_added_checked_and_removed() {
    let inputs = home();
    let said = |inputs: &Inputs, words_said: &str| {
        run(&command(&words(words_said)), inputs, &context())
    };
    let described = said(&inputs, &format!("issue body {TASK} Wire it up."));
    assert!(!described.failed, "{}", described.output);
    assert!(
        text(&described).contains("+++\n\nWire it up.\n\n## Notes\n"),
        "{}",
        text(&described)
    );
    let added = said(
        &after(&inputs, &described),
        &format!("issue checklist {TASK} add the docs"),
    );
    let more = said(
        &after(&after(&inputs, &described), &added),
        &format!("issue checklist {TASK} add the tests"),
    );
    let home_now = after(&after(&after(&inputs, &described), &added), &more);
    assert!(
        text(&more).contains(
            "## Checklist\n- [ ] the docs\n- [ ] the tests\n\n## Notes"
        ),
        "{}",
        text(&more)
    );
    let checked = said(&home_now, &format!("issue checklist {TASK} check 2"));
    assert!(text(&checked).contains("- [ ] the docs\n- [x] the tests\n"));
    let removed = said(
        &after(&home_now, &checked),
        &format!("issue checklist {TASK} remove 1"),
    );
    assert!(text(&removed).contains("## Checklist\n- [x] the tests\n\n"));
    let missing = said(&home_now, &format!("issue checklist {TASK} check 3"));
    assert_eq!(
        (missing.failed, missing.output),
        (true, String::from("home: the checklist has no item 3\n"))
    );
}

#[test]
fn a_note_of_several_lines_is_indented_under_its_first() {
    let outcome = run(
        &Command::Change {
            id: String::from(TASK),
            changes: Vec::from([IssueChange::Note(String::from(
                "first\nsecond",
            ))]),
        },
        &home(),
        &context(),
    );
    assert!(text(&outcome).ends_with(
        "- 2026-11-22T12:00:00Z laptop·claude note: first\n  second\n"
    ));
}

/// The home with the task's file holding `text`.
fn with_task(text: &str) -> Inputs {
    let mut inputs = home();
    for file in &mut inputs.issues {
        if file.stem == TASK {
            file.text = String::from(text);
        }
    }
    inputs
}

fn task_text() -> String {
    home()
        .issues
        .iter()
        .find(|file| file.stem == TASK)
        .map_or_else(String::new, |file| file.text.clone())
}

/// The issue `text` holds as the graph reads it, filed as `stem`.
fn graph_reads(stem: &str, text: &str) -> Result<Issue, String> {
    let flow = load(FLOW)?;
    one(
        &flow,
        &File {
            project: ProjectId(String::from(FOLDER)),
            stem: String::from(stem),
            text: String::from(text),
        },
    )
    .map_err(|problem| problem.message)
}

/// What `home check` says of the home with the task's file holding `text`,
/// what the graph reads of the task, and what a note on it writes.
fn read_and_written(
    text_of_task: &str,
) -> (String, Result<Issue, String>, String) {
    let inputs = with_task(text_of_task);
    let noted = run(
        &command(&words(&format!("issue note {TASK} seen"))),
        &inputs,
        &context(),
    );
    (
        check(&inputs).text(),
        graph_reads(TASK, text_of_task),
        text(&noted),
    )
}

#[test]
fn a_byte_order_mark_windows_line_ends_and_a_last_closing_line_read_alike_for_check_and_write()
 {
    let plain = task_text();
    let bare = plain
        .split_once("+++\n\n")
        .map_or_else(String::new, |(front, _)| format!("{front}+++\n"));
    let crlf = |text: &str| text.replace('\n', "\r\n");
    for (base, variants) in [
        (
            plain.clone(),
            [
                format!("\u{feff}{plain}"),
                crlf(&plain),
                format!("\u{feff}{}", crlf(&plain)),
            ],
        ),
        (
            bare.clone(),
            [
                String::from(bare.trim_end()),
                crlf(bare.trim_end()),
                format!("\u{feff}{}", crlf(bare.trim_end())),
            ],
        ),
    ] {
        let expected = read_and_written(&base);
        assert!(expected.1.is_ok() && !expected.2.is_empty(), "{expected:?}");
        for variant in variants {
            assert_eq!(read_and_written(&variant), expected, "{variant:?}");
        }
    }
}

#[test]
fn a_checklist_edit_changes_the_item_the_graph_read_at_its_place() {
    let listed = task_text().replace(
        "## What\n\n",
        "## What\n\n```\n## Checklist\n- [ ] an example\n```\n\n\
         ##  Checklist \n- [ ] first\n~~~\n- [ ] fenced\n~~~\n- [ ] second\n\n",
    );
    let inputs = with_task(&listed);
    let reread =
        |text: &str| graph_reads(TASK, text).map(|issue| issue.checklist);
    let item = |done, said: &str| ChecklistItem {
        done,
        text: String::from(said),
    };
    assert_eq!(
        reread(&listed),
        Ok(Vec::from([item(false, "first"), item(false, "second")]))
    );
    let checked = run(
        &command(&words(&format!("issue checklist {TASK} check 2"))),
        &inputs,
        &context(),
    );
    assert!(!checked.failed, "{}", checked.output);
    assert_eq!(
        reread(&text(&checked)),
        Ok(Vec::from([item(false, "first"), item(true, "second")]))
    );
    assert!(text(&checked).contains("- [ ] an example\n"));
    assert!(text(&checked).contains("~~~\n- [ ] fenced\n~~~\n- [x] second\n"));
    let removed = run(
        &command(&words(&format!("issue checklist {TASK} remove 1"))),
        &inputs,
        &context(),
    );
    assert_eq!(
        reread(&text(&removed)),
        Ok(Vec::from([item(false, "second")]))
    );
    assert!(text(&removed).contains("- [ ] fenced\n"));
}

#[test]
fn every_line_the_command_line_logs_is_the_one_log_line_of_its_entry()
-> Result<(), String> {
    let inputs = home();
    for said in [
        String::from("issue new home task Wire-it"),
        format!("issue note {TASK} seen"),
        format!("issue move {SPEC} stage slice signed off"),
        format!("issue move {TASK} status ready"),
        format!("issue set {TASK} title=Write-it-well"),
    ] {
        let outcome = run(&command(&words(&said)), &inputs, &context());
        let stem = outcome
            .writes
            .first()
            .map_or_else(String::new, |write| write.stem.clone());
        let log = graph_reads(&stem, &text(&outcome))?.log;
        let last = log.last().map_or_else(String::new, log_line);
        assert!(
            last.starts_with("- 2026-11-22T12:00:00Z laptop·claude ")
                && text(&outcome).ends_with(&format!("\n{last}\n")),
            "{said}: {last:?} in {}",
            text(&outcome)
        );
    }
    Ok(())
}
