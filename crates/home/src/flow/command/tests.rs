use crate::issue::{
    ChecklistChange, ChecklistItem, Datum, Issue, IssueChange, Item, NewIssue,
    Part,
};
use crate::{DRAWS, Draws, IssueId, ProjectId};

use super::{
    Asked, Command, Context, Edit, Local, Outcome, asked as dispatched,
    command, fresh, homeless, run, stamp, write,
};
use crate::flow::check::{Report, check as checked};
use crate::flow::file::log_line;
use crate::flow::graph::{File, one};
use crate::flow::load;
use crate::flow::snapshot::{Inputs, Snapshot};

const FLOW: &str = include_str!("../test-flow.toml");
const ROOT: &str = "e22b7f7b2976650400965d4147ba771675f96076";
const FOLDER: &str = "7w85je";
const SPEC: &str = "0tm7zt01";
const TASK: &str = "0tm7zx0d";
/// 2026-11-22T12:00:00Z.
const NOW: u64 = 1_795_348_800_000;
const SECONDS: u64 = NOW / 1000;
const SEVEN: Draws = [7; DRAWS];

fn words(text: &str) -> Vec<String> {
    text.split(' ').map(String::from).collect()
}

fn issue(id: &str, front: &str, body: &str) -> File {
    File {
        project: ProjectId::parse(FOLDER).unwrap_or(ProjectId::new(0)),
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
        unread: Vec::new(),
    }
}

/// The draws `given`, the last of them repeated to the end.
fn draws(given: &[u64]) -> Draws {
    core::array::from_fn(|at| {
        given.get(at).or_else(|| given.last()).copied().unwrap_or(0)
    })
}

fn id(stem: &str) -> IssueId {
    IssueId::parse(stem).unwrap_or(IssueId::new(0, 0))
}

/// What the command line `line` asks of the home `inputs` holds.
fn asked(inputs: &Inputs, line: &str) -> Command {
    let home = Snapshot::of(inputs.clone());
    command(&words(line), &home, fresh(&home, SECONDS, &SEVEN))
}

/// What saying `line` to the home `inputs` holds writes and says.
fn ran(inputs: &Inputs, line: &str) -> Outcome {
    let home = Snapshot::of(inputs.clone());
    run(
        command(&words(line), &home, fresh(&home, SECONDS, &SEVEN)),
        None,
        &home,
        &context(),
    )
}

fn check(inputs: &Inputs) -> Report {
    checked(&Snapshot::of(inputs.clone()))
}

fn context() -> Context {
    Context {
        now: NOW,
        by: String::from("laptop·claude"),
        mark: None,
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
        project: write.project,
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
fn a_new_issue_takes_the_fresh_id() {
    assert_eq!(
        asked(&home(), &format!("issue new home task Do-it parent={SPEC}")),
        Command::Edit(Edit::New {
            id: id("0tt82007"),
            issue: NewIssue {
                project: ProjectId::parse(FOLDER).unwrap_or(ProjectId::new(0)),
                kind: String::from("task"),
                title: String::from("Do-it"),
                fields: Vec::from([(
                    String::from("parent"),
                    Datum::Text(String::from(SPEC)),
                )]),
            },
        })
    );
}

#[test]
fn a_new_issue_s_id_sorts_after_every_issue_made_before_it() {
    let home = Snapshot::of(home());
    let fresh = fresh(&home, SECONDS, &SEVEN);
    assert!(fresh.is_some());
    assert!(
        home.graph()
            .issues
            .iter()
            .all(|issue| Some(issue.id) < fresh)
    );
    assert!(fresh < super::fresh(&home, SECONDS + 1, &[0; DRAWS]));
}

#[test]
fn a_new_issue_draws_again_for_an_id_the_home_has() {
    let mut held = home();
    held.issues
        .push(issue("0tt82007", "title = \"Taken\"\n", ""));
    let home = Snapshot::of(held);
    assert_eq!(
        fresh(&home, SECONDS, &draws(&[7, 7, 9])),
        Some(id("0tt82009"))
    );
    assert_eq!(fresh(&home, SECONDS, &SEVEN), None);
    assert!(matches!(
        command(&words("issue new home task T"), &home, None),
        Command::Wrong(reason) if reason.starts_with("no issue id is free")
    ));
}

#[test]
fn the_words_name_a_command() {
    let inputs = home();
    let asked = |line: &str| asked(&inputs, line);
    assert_eq!(asked("check"), Command::Check);
    let changing = |change| {
        Command::Edit(Edit::Change {
            id: id(TASK),
            changes: Vec::from([change]),
        })
    };
    assert_eq!(
        asked(&format!(
            "issue move {TASK} stage slice --signed-off by the owner"
        )),
        changing(IssueChange::Move {
            part: Part::Stage,
            to: String::from("slice"),
            reason: String::from("by the owner"),
            signed_off: true,
        })
    );
    assert_eq!(
        asked(&format!("issue move {TASK} status ready to go")),
        changing(IssueChange::Move {
            part: Part::Status,
            to: String::from("ready"),
            reason: String::from("to go"),
            signed_off: false,
        })
    );
    assert_eq!(
        asked(&format!("issue note {TASK} it works")),
        changing(IssueChange::Note(String::from("it works")))
    );
    assert_eq!(
        asked("issue note x it works"),
        Command::Wrong(String::from("no issue is called x"))
    );
    assert_eq!(
        asked(&format!("issue set {TASK} a=1 b=2")),
        Command::Edit(Edit::Change {
            id: id(TASK),
            changes: Vec::from([
                IssueChange::Set {
                    field: String::from("a"),
                    value: Datum::Text(String::from("1")),
                },
                IssueChange::Set {
                    field: String::from("b"),
                    value: Datum::Text(String::from("2")),
                },
            ]),
        })
    );
    assert_eq!(
        asked(&format!("issue set {TASK} a")),
        Command::Wrong(String::from("\"a\" is not <field>=<value>"))
    );
    assert_eq!(
        asked(&format!("issue body {TASK} Why it matters.")),
        changing(IssueChange::Body(String::from("Why it matters.")))
    );
    assert_eq!(
        asked(&format!("issue checklist {TASK} add the docs")),
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
            asked(&format!("issue checklist {TASK} check 1")),
            asked(&format!("issue checklist {TASK} uncheck 1")),
            asked(&format!("issue checklist {TASK} remove 1")),
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
        asked(&format!("issue checklist {TASK} check 0")),
        Command::Wrong(String::from(
            "0 is not an item's number, counted from 1"
        ))
    );
    assert_eq!(asked(&format!("issue set {TASK}")), Command::Usage);
    assert_eq!(
        asked(&format!("issue move {TASK} phase build")),
        Command::Usage
    );
}

#[test]
fn a_project_is_named_by_its_name_or_its_id() {
    let inputs = home();
    assert_eq!(
        asked(&inputs, &format!("issue new {FOLDER} task Do-it")),
        asked(&inputs, "issue new home task Do-it")
    );
    assert_eq!(
        asked(&inputs, "issue new elsewhere task Do-it"),
        Command::Wrong(String::from("no project is called elsewhere"))
    );
}

#[test]
fn a_body_from_a_file_asks_the_shell_for_the_file() {
    let inputs = home();
    let from_file = asked(&inputs, &format!("issue body {TASK} --file why.md"));
    assert_eq!(from_file.file(), Some("why.md"));
    assert_eq!(
        from_file,
        Command::BodyFrom {
            id: id(TASK),
            path: String::from("why.md"),
        }
    );
    assert_eq!(asked(&inputs, "check").file(), None);
}

#[test]
fn a_value_is_read_as_its_field_takes_it() {
    let inputs = home();
    let set = |pair: &str| match asked(
        &inputs,
        &format!("issue set {TASK} {pair}"),
    ) {
        Command::Edit(Edit::Change { changes, .. }) => changes,
        other => Vec::from([IssueChange::Note(format!("{other:?}"))]),
    };
    let value = |field: &str, value| {
        Vec::from([IssueChange::Set {
            field: String::from(field),
            value,
        }])
    };
    let texts = |items: &[&str]| {
        Datum::Texts(items.iter().map(|item| String::from(*item)).collect())
    };
    assert_eq!(
        set("labels=app,,ui"),
        value("labels", texts(&["app", "ui"]))
    );
    assert_eq!(
        set(&format!("blocked_by={SPEC},{TASK}")),
        value("blocked_by", texts(&[SPEC, TASK]))
    );
    assert_eq!(
        set("title=Read,-then-write"),
        [IssueChange::Title(String::from("Read,-then-write"))]
    );
    assert_eq!(
        set(&format!("parent={SPEC}")),
        value("parent", Datum::Text(String::from(SPEC)))
    );
}

#[test]
fn a_body_from_a_file_is_the_text_the_shell_read() {
    let inputs = home();
    let home = Snapshot::of(inputs.clone());
    let from_file =
        || asked(&inputs, &format!("issue body {TASK} --file why.md"));
    let written = run(
        from_file(),
        Some(Ok(String::from("Read from a file.\n"))),
        &home,
        &context(),
    );
    assert!(
        text(&written).contains("+++\n\nRead from a file.\n\n## Notes\n"),
        "{}",
        written.output
    );
    let unread = run(
        from_file(),
        Some(Err(String::from("why.md: not found"))),
        &home,
        &context(),
    );
    assert_eq!(
        (unread.failed, unread.output, unread.writes),
        (true, String::from("home: why.md: not found\n"), Vec::new())
    );
}

#[test]
fn the_command_line_says_what_it_did_and_exits_by_how_it_went() {
    let wrote = ran(&home(), &format!("issue note {TASK} seen"));
    assert_eq!(wrote.ended(Ok(())).1, 0);
    assert_eq!(
        wrote.ended(Err(String::from("disk full"))),
        (String::from("home: disk full\n"), 1)
    );
    assert_eq!(ran(&home(), "issue").ended(Ok(())).1, 1);
    assert_eq!(
        homeless(),
        (
            String::from(
                "home: no home here: no folder above holds docs/agents/flow.toml\n"
            ),
            2
        )
    );
}

#[test]
fn a_new_issue_gets_a_fresh_id_its_sections_and_a_log_that_check_accepts() {
    let inputs = home();
    let outcome = ran(
        &inputs,
        &format!("issue new home task Wire-it parent={SPEC} blocked_by={TASK}"),
    );
    assert!(!outcome.failed, "{}", outcome.output);
    let id = outcome
        .writes
        .first()
        .map_or_else(String::new, |write| write.stem.clone());
    assert_eq!(id, "0tt82007");
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
        let outcome = ran(&home(), said);
        (outcome.failed && outcome.writes.is_empty(), outcome.output)
    };
    let (dangling, output) = refused("issue new home task T parent=0tm7zx0a");
    assert!(dangling);
    assert!(output.contains("parent: 0tm7zx0a is no issue"), "{output}");
    let (undeclared, said) = refused("issue new home task T severity=high");
    assert!(undeclared);
    assert!(
        said.contains("severity: is not a field or relation of \"task\""),
        "{said}"
    );
    let inputs = home();
    let blocking = ran(&inputs, &format!("issue set {SPEC} blocked_by={TASK}"));
    assert!(!blocking.failed, "{}", blocking.output);
    let cycle = ran(
        &after(&inputs, &blocking),
        &format!("issue set {TASK} blocked_by={SPEC}"),
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
    let outcome = ran(&home(), &format!("issue note {TASK} checked"));
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
    let usual = ran(
        &home(),
        &format!("issue move {SPEC} stage slice --signed-off by the owner"),
    );
    assert!(text(&usual).contains("stage = \"slice\"\n"));
    assert!(text(&usual).ends_with(
        "- 2026-11-22T12:00:00Z laptop·claude stage spec→slice: signed off: by the owner\n"
    ));
    let unusual = ran(
        &home(),
        &format!("issue move {SPEC} stage land --signed-off skipping ahead"),
    );
    assert!(!unusual.failed);
    assert!(text(&unusual).ends_with(
        "stage spec→land: signed off: skipping ahead (unusual move)\n"
    ));
    let status = ran(&home(), &format!("issue move {TASK} status ready"));
    assert!(text(&status).contains("status = \"ready\"\n"));
    assert!(text(&status).ends_with("status open→ready: \n"));
    let unknown = ran(&home(), &format!("issue move {TASK} status started"));
    assert!(
        unknown.failed
            && unknown.output.contains("no status is called started")
    );
}

#[test]
fn a_spec_enters_a_sign_off_stage_only_signed_off() {
    let at = |stage: &str| {
        let inputs = home();
        Inputs {
            issues: inputs
                .issues
                .into_iter()
                .map(|file| File {
                    text: file.text.replace(
                        "stage = \"spec\"",
                        &format!("stage = \"{stage}\""),
                    ),
                    ..file
                })
                .collect(),
            ..inputs
        }
    };
    for (from, to) in [("spec", "slice"), ("review", "land")] {
        let inputs = at(from);
        let unsigned =
            ran(&inputs, &format!("issue move {SPEC} stage {to} ready"));
        assert!(unsigned.failed && unsigned.writes.is_empty());
        assert_eq!(
            unsigned.output,
            format!(
                "home: entering the stage {to} needs its sign-off
"
            )
        );
        let signed = ran(
            &inputs,
            &format!("issue move {SPEC} stage {to} --signed-off"),
        );
        assert!(!signed.failed, "{}", signed.output);
        assert!(
            text(&signed).contains(&format!("stage {from}→{to}: signed off")),
            "{}",
            text(&signed)
        );
    }
    for from in ["review", "land"] {
        let rework = ran(&at(from), &format!("issue move {SPEC} stage build"));
        assert!(!rework.failed, "{}", rework.output);
    }
}

#[test]
fn a_stage_or_status_is_moved_not_set() {
    let outcome = ran(&home(), &format!("issue set {TASK} status=done"));
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
    let said = |inputs: &Inputs, words_said: &str| ran(inputs, words_said);
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
        Command::Edit(Edit::Change {
            id: id(TASK),
            changes: Vec::from([IssueChange::Note(String::from(
                "first\nsecond",
            ))]),
        }),
        None,
        &Snapshot::of(home()),
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
            project: ProjectId::parse(FOLDER).unwrap_or(ProjectId::new(0)),
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
    let noted = ran(&inputs, &format!("issue note {TASK} seen"));
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
    let checked = ran(&inputs, &format!("issue checklist {TASK} check 2"));
    assert!(!checked.failed, "{}", checked.output);
    assert_eq!(
        reread(&text(&checked)),
        Ok(Vec::from([item(false, "first"), item(true, "second")]))
    );
    assert!(text(&checked).contains("- [ ] an example\n"));
    assert!(text(&checked).contains("~~~\n- [ ] fenced\n~~~\n- [x] second\n"));
    let removed = ran(&inputs, &format!("issue checklist {TASK} remove 1"));
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
        format!("issue move {SPEC} stage slice --signed-off"),
        format!("issue move {TASK} status ready"),
        format!("issue set {TASK} title=Write-it-well"),
    ] {
        let outcome = ran(&inputs, &said);
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

/// The tests' home with `from` in its flow replaced by `to`.
fn home_with(from: &str, to: &str) -> Inputs {
    assert!(FLOW.contains(from), "the flow has no {from:?}");
    Inputs {
        flow: Ok(FLOW.replacen(from, to, 1)),
        ..home()
    }
}

#[test]
fn a_new_issue_writes_when_it_opened_in_its_created_field_whatever_it_is_called()
 {
    let renamed = ran(
        &home_with("name = \"created\"", "name = \"opened_at\""),
        "issue new home task Wire-it",
    );
    assert!(!renamed.failed, "{}", renamed.output);
    assert!(
        text(&renamed).contains("\nopened_at = 2026-11-22T12:00:00Z\n"),
        "{}",
        text(&renamed)
    );
    let roleless = ran(
        &home_with("role = \"created\"\n", ""),
        "issue new home task Wire-it",
    );
    assert!(!roleless.failed, "{}", roleless.output);
    assert!(
        !text(&roleless).contains("created ="),
        "{}",
        text(&roleless)
    );
}

#[test]
fn an_issue_s_own_words_are_refused_as_fields_in_the_flow_s_terms() {
    let refused = |field: &str| {
        let edit = Edit::Change {
            id: id(TASK),
            changes: Vec::from([IssueChange::Set {
                field: String::from(field),
                value: Datum::Text(String::from("x")),
            }]),
        };
        write(&Snapshot::of(home()), &edit, &context()).map(|_| ())
    };
    assert_eq!(
        ["status", "stage", "kind", "id", "title"].map(refused),
        [
            Err(String::from(
                "a status changes by a move, not by setting a field"
            )),
            Err(String::from(
                "a stage changes by a move, not by setting a field"
            )),
            Err(String::from("an issue's kind doesn't change")),
            Err(String::from("an issue's id doesn't change")),
            Err(String::from(
                "the title changes by retitling, not by setting a field"
            )),
        ]
    );
}

#[test]
fn an_edit_to_an_issue_that_doesn_t_read_says_what_is_wrong_with_it() {
    let inputs =
        with_task(&format!("+++\nid = \"{TASK}\"\nstatus = \"open\"\n+++\n"));
    let outcome = ran(&inputs, &format!("issue note {TASK} hello"));
    assert_eq!(
        (outcome.failed, outcome.output),
        (
            true,
            format!(
                "home: {TASK} doesn't read as the flow declares: title: is missing\n"
            )
        )
    );
}

#[test]
fn repos_and_skills_are_done_in_the_home_s_folder_and_the_rest_on_its_issues() {
    let home = Snapshot::of(home());
    let local = |line: &str| dispatched(&words(line), &home, None);
    assert_eq!(local("repos"), Asked::Local(Local::Status));
    assert_eq!(local("repos sync"), Asked::Local(Local::Sync));
    assert_eq!(local("repos scan"), Asked::Local(Local::Scan));
    assert_eq!(local("skills"), Asked::Local(Local::Skills));
    assert_eq!(local("repos wipe"), Asked::Flow(Command::Usage));
    assert_eq!(local("check"), Asked::Flow(Command::Check));
}
