use crate::issue::storage::fresh;
use crate::issue::{
    ChecklistChange, ChecklistItem, Datum, Issue, IssueChange, Item, NewIssue,
    Part,
};
use crate::{DRAWS, Draws, IssueId, ProjectId};

use super::{
    Asked, CanvasWrite, Command, Context, Edit, Fetched, Given, Local, Need,
    Source, Wanted, asked as dispatched, command, homeless, plan, stamp,
};
use crate::canvas::{
    BadName, Block, Canvas, CanvasError, CanvasFile, CanvasKey, CanvasName,
    Held, Stray, StrayReason,
};
use crate::flow::check::{Report, check as checked};
use crate::flow::load;
use crate::flow::snapshot::Snapshot;
use crate::issue::body::log_line;
use crate::issue::storage::Inputs;
use crate::issue::storage::{File, one};

const FLOW: &str = include_str!("../test-flow.toml");
const ROOT: &str = "e22b7f7b2976650400965d4147ba771675f96076";
const FOLDER: &str = "7w85je";
const SPEC: &str = "0tm7zt01";
const TASK: &str = "0tm7zx0d";
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
        unavailable: Vec::new(),
        observed_at: None,
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

fn draws(given: &[u64]) -> Draws {
    core::array::from_fn(|at| {
        given.get(at).or_else(|| given.last()).copied().unwrap_or(0)
    })
}

fn id(stem: &str) -> IssueId {
    IssueId::from_reference(stem).unwrap_or_else(|| IssueId {
        authority: String::from("local"),
        key: String::from("00000000"),
    })
}

fn asked(inputs: &Inputs, line: &str) -> Command {
    let home = crate::issue::storage::snapshot(inputs.clone());
    command(&words(line), &home, fresh(&home, SECONDS, &SEVEN))
}

#[derive(Debug)]
struct Outcome {
    writes: Vec<File>,
    output: String,
    failed: bool,
    canvases: Vec<CanvasWrite>,
}

impl Outcome {
    fn ended(&self, saved: Result<(), String>) -> (String, u8) {
        super::Outcome {
            changes: Vec::new(),
            canvases: self.canvases.clone(),
            output: self.output.clone(),
            failed: self.failed,
        }
        .ended(saved)
    }
}

fn saved(outcome: super::Outcome, inputs: &Inputs) -> Outcome {
    let loaded = inputs.clone().read();
    let writes = outcome
        .changes
        .iter()
        .map(|change| loaded.encode(change))
        .collect::<Result<Vec<_>, _>>();
    assert!(writes.is_ok(), "{writes:?}");
    Outcome {
        writes: writes.unwrap_or_default(),
        output: outcome.output,
        failed: outcome.failed,
        canvases: outcome.canvases,
    }
}

fn run(
    command: Command,
    given: Option<Given>,
    snapshot: &Snapshot,
    context: &Context,
) -> Outcome {
    saved(super::run(command, given, snapshot, context), &home())
}

fn ran(inputs: &Inputs, line: &str) -> Outcome {
    let home = inputs.clone().read().snapshot;
    saved(
        super::run(
            command(&words(line), &home, fresh(&home, SECONDS, &SEVEN)),
            None,
            &home,
            &context(),
        ),
        inputs,
    )
}

fn check(inputs: &Inputs) -> Report {
    checked(&crate::issue::storage::snapshot(inputs.clone()))
}

fn context() -> Context {
    Context {
        now_ms: NOW,
        by: String::from("laptop·claude"),
    }
}

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
        unavailable: Vec::new(),
        observed_at: None,
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
fn a_new_issue_draws_again_for_an_id_the_home_has() {
    let mut held = home();
    held.issues
        .push(issue("0tt82007", "title = \"Taken\"\n", ""));
    let home = crate::issue::storage::snapshot(held);
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
    assert_eq!(
        from_file.needs(),
        Some(Need::Read(Source::File(String::from("why.md"))))
    );
    assert_eq!(
        from_file,
        Command::BodyFrom {
            id: id(TASK),
            path: String::from("why.md"),
        }
    );
    assert_eq!(asked(&inputs, "check").needs(), Some(Need::EveryCanvas));
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
    let dotted = Inputs {
        unavailable: Vec::new(),
        observed_at: None,
        flow: Ok(FLOW.replace("\"priority\"", "\"triage.level\"")),
        ..home()
    };
    for words in [
        String::from("issue new home task T triage.level=high"),
        format!("issue set {TASK} triage.level=low"),
    ] {
        let result = ran(&dotted, &words);
        assert!(!result.failed, "{}", result.output);
        let report = check(&after(&dotted, &result));
        assert!(!report.failed(), "{}", report.text());
    }
}

#[test]
fn a_body_from_a_file_is_the_text_the_shell_read() {
    let inputs = home();
    let home = crate::issue::storage::snapshot(inputs.clone());
    let from_file =
        || asked(&inputs, &format!("issue body {TASK} --file why.md"));
    let written = run(
        from_file(),
        Some(Given::Text(Ok(String::from("Read from a file.\n")))),
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
        Some(Given::Text(Err(String::from("why.md: not found")))),
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
    let staged = ran(&inputs, "issue new home spec Stage-it stage=shape");
    assert!(!staged.failed, "{}", staged.output);
    assert_eq!(
        graph_reads("0tt82007", &text(&staged))
            .ok()
            .and_then(|issue| issue.stage),
        Some(String::from("shape"))
    );
    for fields in ["priority=high priority=low", "created=2026-11-21T00:00:00Z"]
    {
        let duplicate =
            ran(&inputs, &format!("issue new home task T {fields}"));
        assert!(
            duplicate.failed && duplicate.writes.is_empty(),
            "{}",
            duplicate.output
        );
    }
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
            unavailable: Vec::new(),
            observed_at: None,
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
                "home: entering the stage {to} needs its sign-off; fix: add `--signed-off` once the owner has signed off
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
        &crate::issue::storage::snapshot(home()),
        &context(),
    );
    assert!(text(&outcome).ends_with(
        "- 2026-11-22T12:00:00Z laptop·claude note: first\n  second\n"
    ));
}

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

fn home_with(from: &str, to: &str) -> Inputs {
    assert!(FLOW.contains(from), "the flow has no {from:?}");
    Inputs {
        unavailable: Vec::new(),
        observed_at: None,
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
        plan(&crate::issue::storage::snapshot(home()), &edit, &context())
            .map(|_| ())
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
    let home = crate::issue::storage::snapshot(home());
    let local = |line: &str| dispatched(&words(line), &home, None);
    assert_eq!(local("repos"), Asked::Local(Local::Status));
    assert_eq!(local("repos --verify"), Asked::Local(Local::Verify));
    assert_eq!(local("repos sync"), Asked::Local(Local::Sync));
    assert_eq!(local("repos scan"), Asked::Local(Local::Scan));
    assert_eq!(local("skills"), Asked::Local(Local::Skills));
    assert_eq!(local("repos wipe"), Asked::Flow(Command::Usage));
    assert_eq!(local("repos sync --verify"), Asked::Flow(Command::Usage));
    assert_eq!(local("check"), Asked::Flow(Command::Check));
}

const PROSE: &str = r#"{"title": "Why a store", "blocks": [{"prose": {"text": "The core decides; the store **keeps**."}}]}"#;

fn canvas_key(name: &str) -> Result<CanvasKey, BadName> {
    Ok(CanvasKey {
        project: ProjectId::parse(FOLDER).unwrap_or(ProjectId::new(0)),
        issue: id(TASK),
        name: CanvasName::parse(name)?,
    })
}

fn why_a_store(title: &str) -> Result<Canvas, CanvasError> {
    Canvas::new(
        String::from(title),
        Vec::from([Block::Prose {
            text: String::from("The core decides; the store **keeps**."),
        }]),
    )
}

fn given(line: &str, supplied: Given) -> Outcome {
    let inputs = home();
    run(
        asked(&inputs, line),
        Some(supplied),
        &crate::issue::storage::snapshot(inputs.clone()),
        &context(),
    )
}

#[test]
fn a_canvas_put_reads_its_json_from_the_file_or_else_from_stdin()
-> Result<(), BadName> {
    let inputs = home();
    assert_eq!(
        asked(
            &inputs,
            &format!("canvas put {TASK} why-a-store --file why.json")
        ),
        Command::CanvasPut {
            key: canvas_key("why-a-store")?,
            from: Source::File(String::from("why.json")),
        }
    );
    assert_eq!(
        asked(&inputs, &format!("canvas put {TASK} why-a-store")).needs(),
        Some(Need::Read(Source::Stdin))
    );
    Ok(())
}

#[test]
fn a_canvas_put_on_an_issue_writes_the_canvas_under_its_key_stamped_with_when_and_by_whom()
-> Result<(), Box<dyn core::error::Error>> {
    let outcome = given(
        &format!("canvas put {TASK} why-a-store"),
        Given::Text(Ok(String::from(PROSE))),
    );
    assert_eq!(
        (outcome.failed, outcome.writes, outcome.canvases),
        (
            false,
            Vec::new(),
            Vec::from([CanvasWrite {
                key: canvas_key("why-a-store")?,
                file: CanvasFile {
                    written: NOW,
                    by: String::from("laptop·claude"),
                    canvas: why_a_store("Why a store")?,
                },
            }])
        )
    );
    assert_eq!(
        outcome.output,
        format!("wrote canvas why-a-store of home/{TASK}\n")
    );
    Ok(())
}

#[test]
fn a_canvas_put_on_an_unknown_issue_with_a_bad_name_or_json_that_isnt_a_canvas_writes_nothing_and_says_why()
 {
    let home = crate::issue::storage::snapshot(home());
    let put = |words: &[&str], text: &str| {
        let args: Vec<String> =
            words.iter().copied().map(String::from).collect();
        run(
            command(&args, &home, None),
            Some(Given::Text(Ok(String::from(text)))),
            &home,
            &context(),
        )
    };
    let cases = [
        (
            put(&["canvas", "put", "0tm7zzzz", "why"], PROSE),
            String::from("home: no issue is called 0tm7zzzz\n"),
        ),
        (
            put(&["canvas", "put", TASK, "Design Tree"], PROSE),
            String::from(
                "home: `Design Tree` isn't a canvas name: name it with lowercase words joined by `-`, such as `design-tree`\n",
            ),
        ),
        (
            put(&["canvas", "put", TASK, "why"], r#"{"title": "t"}"#),
            format!(
                "home: canvas why of home/{TASK}: not a canvas: missing field `blocks` at line 1 column 14\n"
            ),
        ),
    ];
    for (outcome, said) in cases {
        assert_eq!(
            (
                outcome.failed,
                outcome.output,
                outcome.writes,
                outcome.canvases
            ),
            (true, said, Vec::new(), Vec::new())
        );
    }
    let unread = given(
        &format!("canvas put {TASK} why --file why.json"),
        Given::Text(Err(String::from("why.json: not found"))),
    );
    assert_eq!(
        (unread.failed, unread.output, unread.canvases),
        (
            true,
            String::from("home: why.json: not found\n"),
            Vec::new()
        )
    );
}

#[test]
fn a_canvas_show_asks_the_store_for_the_named_canvas_or_every_canvas_of_the_issue()
-> Result<(), BadName> {
    let inputs = home();
    let wanted = |name| Wanted {
        project: ProjectId::parse(FOLDER).unwrap_or(ProjectId::new(0)),
        issue: id(TASK),
        name,
    };
    assert_eq!(
        asked(&inputs, &format!("canvas show {TASK} why-a-store")).needs(),
        Some(Need::Canvases(wanted(Some(CanvasName::parse(
            "why-a-store"
        )?))))
    );
    assert_eq!(
        asked(&inputs, &format!("canvas show {TASK}")).needs(),
        Some(Need::Canvases(wanted(None)))
    );
    assert_eq!(
        asked(&inputs, "canvas show 0tm7zzzz"),
        Command::Wrong(String::from("no issue is called 0tm7zzzz"))
    );
    Ok(())
}

#[test]
fn a_canvas_show_prints_the_canvas_s_title_and_prose_as_markdown()
-> Result<(), Box<dyn core::error::Error>> {
    let kept = |name: &str, title: &str| {
        Ok::<_, Box<dyn core::error::Error>>(fetched(
            TASK,
            name,
            Ok(Some(CanvasFile {
                written: NOW,
                by: String::from("laptop·claude"),
                canvas: why_a_store(title)?,
            })),
        )?)
    };
    let one = given(
        &format!("canvas show {TASK} why-a-store"),
        Given::Canvases(Ok(held(Vec::from([kept(
            "why-a-store",
            "Why a store",
        )?])))),
    );
    assert_eq!(
        (one.failed, one.output),
        (
            false,
            String::from(
                "# Why a store\n\nThe core decides; the store **keeps**.\n"
            )
        )
    );
    let every = given(
        &format!("canvas show {TASK}"),
        Given::Canvases(Ok(held(Vec::from([
            kept("first", "First")?,
            kept("second", "Second")?,
        ])))),
    );
    assert_eq!(
        every.output,
        "# First\n\nThe core decides; the store **keeps**.\n\n\
         # Second\n\nThe core decides; the store **keeps**.\n"
    );
    Ok(())
}

#[test]
fn a_canvas_show_of_a_name_the_store_lacks_or_a_store_that_fails_says_so() {
    let missing = given(
        &format!("canvas show {TASK} gone"),
        Given::Canvases(Ok(held(Vec::new()))),
    );
    assert_eq!(
        (missing.failed, missing.output),
        (
            true,
            format!("home: no canvas is called gone on home/{TASK}\n")
        )
    );
    let none = given(
        &format!("canvas show {TASK}"),
        Given::Canvases(Ok(held(Vec::new()))),
    );
    assert_eq!(
        (none.failed, none.output),
        (false, format!("home/{TASK} has no canvases\n"))
    );
    let failing = given(
        &format!("canvas show {TASK}"),
        Given::Canvases(Err(String::from("projects: denied"))),
    );
    assert_eq!(
        (failing.failed, failing.output),
        (true, String::from("home: projects: denied\n"))
    );
}

fn statechart(
    states: &str,
    start: &str,
    transitions: &str,
    path: &str,
) -> String {
    format!(
        r#"{{"title": "A task's turn", "blocks": [{{"statechart": {{"states": [{states}], "start": "{start}", "transitions": [{transitions}], "path": [{path}]}}}}]}}"#
    )
}

const STATES: &str = r#"{"id": "open", "label": "Open"}, {"id": "review", "label": "In review", "marked": "the owner reads it here"}, {"id": "done", "label": "Done"}"#;
const TRANSITIONS: &str = r#"{"from": "open", "to": "review", "event": "submit"}, {"from": "review", "to": "open", "event": "send back"}, {"from": "review", "to": "done", "event": "land"}"#;

#[test]
fn a_canvas_put_of_a_statechart_that_breaks_a_drawing_rule_writes_nothing_and_says_what_to_change()
 {
    let cases = [
        (
            statechart(STATES, "open", TRANSITIONS, r#""open", "done""#),
            "the path goes from `open` to `done`, but no transition does: add that transition or change the path",
        ),
        (
            statechart(
                STATES,
                "open",
                r#"{"from": "open", "to": "gone", "event": "submit"}"#,
                "",
            ),
            "a transition goes to `gone`, which isn't a state: add the state or change the transition",
        ),
        (
            statechart(
                STATES,
                "open",
                r#"{"from": "gone", "to": "open", "event": "submit"}"#,
                "",
            ),
            "a transition comes from `gone`, which isn't a state: add the state or change the transition",
        ),
        (
            statechart(STATES, "gone", TRANSITIONS, ""),
            "the statechart starts at `gone`, which isn't a state: add the state or change `start`",
        ),
        (
            statechart(STATES, "open", TRANSITIONS, r#""review", "done""#),
            "the path begins at `review`, but the statechart starts at `open`: begin the path with `open`",
        ),
        (
            statechart(
                r#"{"id": "open", "label": "Open and waiting for someone to pick it up today"}"#,
                "open",
                "",
                "",
            ),
            "the label `Open and waiting for someone to pick it up today` is over 8 words: put the detail in prose",
        ),
        (
            statechart(
                STATES,
                "open",
                r#"{"from": "open", "to": "review", "event": "the worker pushes the branch and asks for a review"}"#,
                "",
            ),
            "the label `the worker pushes the branch and asks for a review` is over 8 words: put the detail in prose",
        ),
        (
            statechart(
                r#"{"id": "open", "label": "Open"}, {"id": "open", "label": "Reopened"}"#,
                "open",
                "",
                "",
            ),
            "two states are called `open`: give each state its own id",
        ),
    ];
    for (text, said) in cases {
        let outcome =
            given(&format!("canvas put {TASK} turn"), Given::Text(Ok(text)));
        assert_eq!(
            (
                outcome.failed,
                outcome.output,
                outcome.writes,
                outcome.canvases
            ),
            (
                true,
                format!("home: canvas turn of home/{TASK}: {said}\n"),
                Vec::new(),
                Vec::new()
            )
        );
    }
}

#[test]
fn a_canvas_show_prints_a_statechart_as_a_mermaid_state_diagram_with_marked_states_and_the_path_styled()
-> Result<(), String> {
    let text =
        statechart(STATES, "open", TRANSITIONS, r#""open", "review", "done""#);
    let canvas = Canvas::parse(&text).map_err(|error| error.to_string())?;
    let shown = given(
        &format!("canvas show {TASK} turn"),
        Given::Canvases(Ok(held(Vec::from([fetched(
            TASK,
            "turn",
            Ok(Some(CanvasFile {
                written: NOW,
                by: String::from("laptop·claude"),
                canvas,
            })),
        )?])))),
    );
    assert_eq!(
        (shown.failed, shown.output),
        (
            false,
            String::from(
                "# A task's turn\n\n\
                 ```mermaid\n\
                 stateDiagram-v2\n    \
                 state \"Open\" as s0\n    \
                 state \"In review\" as s1\n    \
                 state \"Done\" as s2\n    \
                 [*] --> s0\n    \
                 s0 --> s1 : submit\n    \
                 s1 --> s0 : send back\n    \
                 s1 --> s2 : land\n    \
                 note right of s1 : the owner reads it here\n    \
                 classDef marked fill:#fde68a,stroke:#b45309\n    \
                 class s1 marked\n    \
                 classDef path stroke:#2563eb,stroke-width:3px\n    \
                 class s0,s1,s2 path\n\
                 ```\n"
            )
        )
    );
    Ok(())
}

#[test]
fn a_canvas_show_prints_a_state_by_its_index_with_its_label_quoted() {
    let text = statechart(
        r#"{"id": "open", "label": "Open"}, {"id": "in review", "label": "Say \"hi\""}"#,
        "open",
        r#"{"from": "open", "to": "in review", "event": "submit"}"#,
        "",
    );
    let put = given(&format!("canvas put {TASK} turn"), Given::Text(Ok(text)));
    let shown = given(
        &format!("canvas show {TASK} turn"),
        Given::Canvases(Ok(held(
            put.canvases
                .into_iter()
                .map(|write| Fetched {
                    key: write.key,
                    got: Ok(Some(write.file)),
                })
                .collect(),
        ))),
    );
    assert_eq!(
        (shown.failed, shown.output.as_str()),
        (
            false,
            "# A task's turn\n\n\
             ```mermaid\n\
             stateDiagram-v2\n    \
             state \"Open\" as s0\n    \
             state \"Say #quot;hi#quot;\" as s1\n    \
             [*] --> s0\n    \
             s0 --> s1 : submit\n\
             ```\n"
        )
    );
}

#[test]
fn a_canvas_show_keeps_a_statechart_s_events_and_marked_notes_on_one_line_with_colons_escaped()
-> Result<(), String> {
    let text = statechart(
        r#"{"id": "open", "label": "Open", "marked": "the owner:\nreads it"}, {"id": "done", "label": "Done"}"#,
        "open",
        r#"{"from": "open", "to": "done", "event": "land:\nnow"}"#,
        "",
    );
    let canvas = Canvas::parse(&text).map_err(|error| error.to_string())?;
    let shown = given(
        &format!("canvas show {TASK} turn"),
        Given::Canvases(Ok(held(Vec::from([fetched(
            TASK,
            "turn",
            Ok(Some(CanvasFile {
                written: NOW,
                by: String::from("laptop·claude"),
                canvas,
            })),
        )?])))),
    );
    assert_eq!(
        (shown.failed, shown.output.as_str()),
        (
            false,
            "# A task's turn\n\n\
             ```mermaid\n\
             stateDiagram-v2\n    \
             state \"Open\" as s0\n    \
             state \"Done\" as s1\n    \
             [*] --> s0\n    \
             s0 --> s1 : land#58; now\n    \
             note right of s0 : the owner#58; reads it\n    \
             classDef marked fill:#fde68a,stroke:#b45309\n    \
             class s0 marked\n\
             ```\n"
        )
    );
    Ok(())
}

#[test]
fn a_canvas_put_of_a_path_that_begins_at_start_and_follows_transitions_writes_it()
 {
    let outcome = given(
        &format!("canvas put {TASK} turn"),
        Given::Text(Ok(statechart(
            r#"{"id": "a", "label": "A"}, {"id": "b", "label": "B"}"#,
            "a",
            r#"{"from": "a", "to": "b", "event": "go"}"#,
            r#""a", "b""#,
        ))),
    );
    assert_eq!(
        (outcome.failed, outcome.canvases.len()),
        (false, 1),
        "{}",
        outcome.output
    );
}

fn put_canvas(text: &str) -> Outcome {
    given(
        &format!("canvas put {TASK} design"),
        Given::Text(Ok(String::from(text))),
    )
}

fn diagram(nodes: &str, edges: &str) -> String {
    format!(
        r#"{{"title": "Design", "blocks": [{{"diagram": {{"nodes": [{nodes}], "edges": [{edges}], "groups": [{{"id": "shell", "label": "Shell"}}]}}}}]}}"#
    )
}

fn node(id: &str, label: &str, step: u8) -> String {
    format!(r#"{{"id": "{id}", "label": "{label}", "step": {step}}}"#)
}

#[test]
fn a_canvas_put_of_a_diagram_that_breaks_a_drawing_rule_writes_nothing_and_says_what_to_change()
 {
    let seven = (1..=7)
        .map(|at| node(&format!("n{at}"), "Part", if at < 4 { 1 } else { 2 }))
        .collect::<Vec<String>>()
        .join(", ");
    let cases = [
        (
            diagram(&seven, ""),
            format!(
                "home: canvas design of home/{TASK}: step 2 of a diagram shows 7 nodes, over 6: split it into steps or diagrams\n"
            ),
        ),
        (
            diagram(
                &node(
                    "core",
                    "the core decides what every store keeps and how",
                    1,
                ),
                "",
            ),
            format!(
                "home: canvas design of home/{TASK}: the label `the core decides what every store keeps and how` is over 8 words: put the detail in prose\n"
            ),
        ),
        (
            diagram(
                &[node("core", "Core", 1), node("store", "Store", 1)]
                    .join(", "),
                r#"{"from": "core", "to": "store", "label": "this edge label runs on for nine whole words", "step": 1}"#,
            ),
            format!(
                "home: canvas design of home/{TASK}: the label `this edge label runs on for nine whole words` is over 8 words: put the detail in prose\n"
            ),
        ),
        (
            diagram(
                &node("core", "Core", 1),
                r#"{"from": "core", "to": "store", "step": 1}"#,
            ),
            format!(
                "home: canvas design of home/{TASK}: an edge runs from `core` to `store`, but the diagram has no node `store`: add the node or drop the edge\n"
            ),
        ),
        (
            diagram(
                r#"{"id": "folder", "label": "Folder", "group": "core", "step": 1}"#,
                "",
            ),
            format!(
                "home: canvas design of home/{TASK}: node `folder` is in group `core`, but the diagram has no group `core`: add the group or take the node out of it\n"
            ),
        ),
        (
            diagram(
                &[node("core", "Core", 1), node("store", "Store", 2)]
                    .join(", "),
                r#"{"from": "core", "to": "store", "step": 1}"#,
            ),
            format!(
                "home: canvas design of home/{TASK}: the edge from `core` to `store` comes at step 1, before node `store` at step 2: move the edge to step 2 or later\n"
            ),
        ),
        (
            diagram(
                &[node("core", "Core", 1), node("core", "Store", 2)].join(", "),
                "",
            ),
            format!(
                "home: canvas design of home/{TASK}: two nodes are called `core`: give each node its own id\n"
            ),
        ),
        (
            String::from(
                r#"{"title": "Design", "blocks": [{"diagram": {"nodes": [], "edges": [], "groups": [{"id": "shell", "label": "the shell does every piece of input and output"}]}}]}"#,
            ),
            format!(
                "home: canvas design of home/{TASK}: the label `the shell does every piece of input and output` is over 8 words: put the detail in prose\n"
            ),
        ),
    ];
    for (text, said) in cases {
        let outcome = put_canvas(&text);
        assert_eq!(
            (
                outcome.failed,
                outcome.output,
                outcome.writes,
                outcome.canvases
            ),
            (true, said, Vec::new(), Vec::new()),
            "{text}"
        );
    }
}

#[test]
fn a_canvas_show_prints_a_diagram_as_one_flowchart_per_step_each_holding_the_steps_before()
 {
    let text = diagram(
        &[
            node("core", "Core", 1),
            node("store", "Store", 1),
            String::from(
                r#"{"id": "folder", "label": "Home \"folder\"", "group": "shell", "step": 2}"#,
            ),
        ]
        .join(", "),
        r#"{"from": "core", "to": "store", "label": "puts", "step": 1},
           {"from": "store", "to": "folder", "step": 2}"#,
    );
    let put = put_canvas(&text);
    let shown = given(
        &format!("canvas show {TASK} design"),
        Given::Canvases(Ok(held(
            put.canvases
                .into_iter()
                .map(|write| Fetched {
                    key: write.key,
                    got: Ok(Some(write.file)),
                })
                .collect(),
        ))),
    );
    assert_eq!(
        (shown.failed, shown.output.as_str()),
        (
            false,
            "# Design\n\n\
             ```mermaid\nflowchart LR\n    \
             n0[\"Core\"]\n    n1[\"Store\"]\n    \
             n0 -->|\"puts\"| n1\n```\n\n\
             ```mermaid\nflowchart LR\n    \
             n0[\"Core\"]\n    n1[\"Store\"]\n    \
             subgraph g0[\"Shell\"]\n        n2[\"Home #quot;folder#quot;\"]\n    end\n    \
             n0 -->|\"puts\"| n1\n    n1 --> n2\n```\n"
        )
    );
}

#[test]
fn a_canvas_show_keeps_a_label_with_a_newline_on_one_mermaid_line() {
    let shown = |text: &str| {
        let put = put_canvas(text);
        given(
            &format!("canvas show {TASK} design"),
            Given::Canvases(Ok(held(
                put.canvases
                    .into_iter()
                    .map(|write| Fetched {
                        key: write.key,
                        got: Ok(Some(write.file)),
                    })
                    .collect(),
            ))),
        )
        .output
    };
    let drawn = shown(
        &diagram(
            r#"{"id": "core", "label": "Core\nlogic", "step": 1},
               {"id": "folder", "label": "Folder", "group": "shell", "step": 1}"#,
            r#"{"from": "core", "to": "folder", "label": "puts\nit", "step": 1}"#,
        )
        .replace(r#""label": "Shell""#, r#""label": "Home\nshell""#),
    );
    assert_eq!(
        drawn,
        "# Design\n\n\
         ```mermaid\nflowchart LR\n    \
         n0[\"Core logic\"]\n    \
         subgraph g0[\"Home shell\"]\n        n1[\"Folder\"]\n    end\n    \
         n0 -->|\"puts it\"| n1\n```\n"
    );
    let charted = shown(&statechart(
        r#"{"id": "open", "label": "Open\nnow"}, {"id": "done", "label": "Done"}"#,
        "open",
        r#"{"from": "open", "to": "done", "event": "land"}"#,
        "",
    ));
    assert!(
        charted.contains("    state \"Open now\" as s0\n"),
        "{charted}"
    );
}

fn blocks(blocks: &str) -> String {
    format!(r#"{{"title": "Where it stands", "blocks": [{blocks}]}}"#)
}

const CHOICES: &str = r#"[{"label": "Home folder", "gets": "history in git", "costs": "a sync per write"}, {"label": "Database", "gets": "queries", "costs": "a server to run"}]"#;

#[test]
fn a_canvas_put_with_more_done_than_total_a_pick_past_the_options_or_groups_out_of_order_writes_nothing_and_says_what_to_change()
 {
    let cases = [
        (
            blocks(
                r#"{"progress": {"label": "Build", "bar": {"count": {"done": 6, "total": 5}}}}"#,
            ),
            "`Build` has 6 done of 5: make `done` at most `total`",
        ),
        (
            blocks(
                r#"{"progress": {"label": "Movie", "bar": {"stretches": [{"label": "Opening", "done": 3, "total": 3}, {"label": "Middle", "done": 5, "total": 4}]}}}"#,
            ),
            "`Middle` has 5 done of 4: make `done` at most `total`",
        ),
        (
            blocks(&format!(
                r#"{{"options": {{"question": "Where?", "options": {CHOICES}, "pick": 2, "why": "it syncs"}}}}"#
            )),
            "`pick` is 2, past the last of 2 options: pick one from 0 to 1",
        ),
        (
            blocks(
                r#"{"options": {"question": "Where?", "options": [], "pick": 0, "why": "it syncs"}}"#,
            ),
            "`Where?` has no options: add the choices the pick is made from",
        ),
        (
            blocks(
                r#"{"changes": {"groups": [{"kind": "wiring", "entries": []}, {"kind": "core", "entries": []}]}}"#,
            ),
            "the `core` group comes after `wiring`: order the groups core, wiring, mechanical",
        ),
        (
            blocks(
                r#"{"changes": {"groups": [{"kind": "core", "entries": []}, {"kind": "core", "entries": []}]}}"#,
            ),
            "the `core` group comes twice: merge them into one",
        ),
    ];
    for (text, said) in cases {
        let outcome =
            given(&format!("canvas put {TASK} stands"), Given::Text(Ok(text)));
        assert_eq!(
            (
                outcome.failed,
                outcome.output,
                outcome.writes,
                outcome.canvases
            ),
            (
                true,
                format!("home: canvas stands of home/{TASK}: {said}\n"),
                Vec::new(),
                Vec::new()
            )
        );
    }
}

fn shown(text: &str) -> Result<Outcome, String> {
    let canvas = Canvas::parse(text).map_err(|error| error.to_string())?;
    Ok(given(
        &format!("canvas show {TASK} stands"),
        Given::Canvases(Ok(held(Vec::from([fetched(
            TASK,
            "stands",
            Ok(Some(CanvasFile {
                written: NOW,
                by: String::from("laptop·claude"),
                canvas,
            })),
        )?])))),
    ))
}

#[test]
fn a_canvas_show_prints_progress_as_text_bars_with_one_line_per_stretch()
-> Result<(), String> {
    let outcome = shown(&blocks(
        r#"{"progress": {"label": "Build", "bar": {"count": {"done": 4, "total": 10}}}}, {"progress": {"label": "Movie", "bar": {"stretches": [{"label": "Opening", "done": 3, "total": 3}, {"label": "Middle", "done": 2, "total": 4}, {"label": "End", "done": 0, "total": 5}]}}}"#,
    ))?;
    assert_eq!(
        (outcome.failed, outcome.output),
        (
            false,
            String::from(
                "# Where it stands\n\n\
                 **Build** `████░░░░░░` 4/10\n\n\
                 **Movie**\n\n\
                 - Opening `██████████` 3/3\n\
                 - Middle `█████░░░░░` 2/4\n\
                 - End `░░░░░░░░░░` 0/5\n"
            )
        )
    );
    Ok(())
}

#[test]
fn a_canvas_show_prints_options_as_a_list_with_the_pick_marked_and_why()
-> Result<(), String> {
    let outcome = shown(&blocks(&format!(
        r#"{{"options": {{"question": "Where do canvases live?", "options": {CHOICES}, "pick": 0, "why": "it syncs with the issues"}}}}"#
    )))?;
    assert_eq!(
        (outcome.failed, outcome.output),
        (
            false,
            String::from(
                "# Where it stands\n\n\
                 **Where do canvases live?**\n\n\
                 - **Home folder** (picked): gets history in git; costs a sync per write\n\
                 - **Database**: gets queries; costs a server to run\n\n\
                 Picked **Home folder**: it syncs with the issues\n"
            )
        )
    );
    Ok(())
}

#[test]
fn a_canvas_show_prints_changes_as_grouped_lists_with_their_tags_and_code_as_links()
-> Result<(), String> {
    let outcome = shown(&blocks(
        r#"{"changes": {"groups": [{"kind": "core", "entries": [{"code": {"path": "crates/home/src/canvas.rs", "lines": [10, 20]}, "note": "the block type", "tag": "breaking"}, {"code": {"path": "crates/home/src/canvas/progress.rs"}, "note": "the bar"}]}, {"kind": "mechanical", "entries": [{"code": {"path": "crates/home/src/lib.rs", "lines": [3, 3], "commit": "d4a1e852"}, "note": "the export", "tag": "subtle"}]}]}}, {"code": {"path": "crates/home/src/canvas.rs", "lines": [30, 41]}}"#,
    ))?;
    assert_eq!(
        (outcome.failed, outcome.output),
        (
            false,
            String::from(
                "# Where it stands\n\n\
                 **Core**\n\n\
                 - [crates/home/src/canvas.rs:10-20](crates/home/src/canvas.rs#L10-L20): the block type (breaking)\n\
                 - [crates/home/src/canvas/progress.rs](crates/home/src/canvas/progress.rs): the bar\n\n\
                 **Mechanical**\n\n\
                 - [crates/home/src/lib.rs:3 at d4a1e852](crates/home/src/lib.rs#L3): the export (subtle)\n\n\
                 [crates/home/src/canvas.rs:30-41](crates/home/src/canvas.rs#L30-L41)\n"
            )
        )
    );
    Ok(())
}

fn fetched(
    issue: &str,
    name: &str,
    got: Result<Option<CanvasFile>, String>,
) -> Result<Fetched, String> {
    Ok(Fetched {
        key: CanvasKey {
            project: ProjectId::parse(FOLDER).ok_or("project")?,
            issue: IssueId::from_reference(issue).ok_or("issue")?,
            name: CanvasName::parse(name).map_err(|error| error.to_string())?,
        },
        got,
    })
}

fn held<T>(canvases: Vec<T>) -> Held<T> {
    Held {
        canvases,
        strays: Vec::new(),
    }
}

fn prose_file() -> Result<CanvasFile, String> {
    Ok(CanvasFile {
        written: NOW,
        by: String::from("laptop·claude"),
        canvas: Canvas::new(
            String::from("Why a store"),
            Vec::from([Block::Prose {
                text: String::from("The core decides."),
            }]),
        )
        .map_err(|error| error.to_string())?,
    })
}

#[test]
fn check_reports_each_canvas_the_store_cant_return_or_that_no_issue_holds()
-> Result<(), String> {
    let checked = given(
        "check",
        Given::EveryCanvas(Ok(held(Vec::from([
            fetched(TASK, "fine", Ok(Some(prose_file()?)))?,
            fetched(TASK, "denied", Err(String::from("permission denied")))?,
            fetched(
                TASK,
                "garbled",
                Err(String::from("not a canvas: missing field `title`")),
            )?,
            fetched(TASK, "vanished", Ok(None))?,
            fetched("0v29xvc9", "orphan", Ok(Some(prose_file()?)))?,
        ])))),
    );
    assert_eq!(
        (checked.failed, checked.output),
        (
            true,
            format!(
                "warn   home/{TASK}  lacks the section \"Acceptance\"\n\
                 error  canvas denied of home/{TASK}  permission denied\n\
                 error  canvas garbled of home/{TASK}  not a canvas: missing field `title`\n\
                 error  canvas vanished of home/{TASK}  the store lists it but returns nothing\n\
                 error  canvas orphan of home/0v29xvc9  the issue doesn't exist: write the canvas under one that does, or remove it\n\
                 4 errors, 1 warnings\n"
            )
        )
    );
    Ok(())
}

#[test]
fn check_of_a_home_whose_canvases_are_all_valid_reports_nothing_new()
-> Result<(), String> {
    let checked = given(
        "check",
        Given::EveryCanvas(Ok(held(Vec::from([
            fetched(TASK, "why-a-store", Ok(Some(prose_file()?)))?,
            fetched(SPEC, "design", Ok(Some(prose_file()?)))?,
        ])))),
    );
    assert_eq!(
        (checked.failed, checked.output),
        (
            false,
            format!(
                "warn   home/{TASK}  lacks the section \"Acceptance\"\n\
                 0 errors, 1 warnings\n"
            )
        )
    );
    Ok(())
}

#[test]
fn check_reports_a_store_that_cant_list_its_canvases() {
    let checked = given(
        "check",
        Given::EveryCanvas(Err(String::from("projects: denied"))),
    );
    assert_eq!(
        (checked.failed, checked.output),
        (
            true,
            format!(
                "warn   home/{TASK}  lacks the section \"Acceptance\"\n\
                 error  canvases: projects: denied\n\
                 1 errors, 1 warnings\n"
            )
        )
    );
}

#[test]
fn a_canvas_show_prints_an_issue_s_valid_canvases_then_names_what_it_skipped()
-> Result<(), String> {
    let shown = given(
        &format!("canvas show {TASK}"),
        Given::Canvases(Ok(Held {
            canvases: Vec::from([
                fetched(TASK, "first", Ok(Some(prose_file()?)))?,
                fetched(
                    TASK,
                    "garbled",
                    Err(String::from("not a canvas: missing field `title`")),
                )?,
                fetched(TASK, "second", Ok(Some(prose_file()?)))?,
            ]),
            strays: Vec::from([Stray {
                place: String::from("over/there/notes"),
                reason: StrayReason::Folder,
            }]),
        })),
    );
    assert_eq!(
        (shown.failed, shown.output),
        (
            true,
            format!(
                "# Why a store\n\nThe core decides.\n\n\
                 # Why a store\n\nThe core decides.\n\
                 home: canvas garbled of home/{TASK}: not a canvas: missing field `title`\n\
                 home: over/there/notes: is a folder, and an issue's folder holds only canvases: move it out of the issue\n"
            )
        )
    );
    Ok(())
}

#[test]
fn check_and_canvas_show_name_a_broken_canvas_the_same_way()
-> Result<(), String> {
    let broken = || {
        fetched(
            TASK,
            "garbled",
            Err(String::from("not a canvas: missing field `title`")),
        )
    };
    let named = format!("canvas garbled of home/{TASK}");
    let checked = given(
        "check",
        Given::EveryCanvas(Ok(held(Vec::from([broken()?])))),
    );
    let shown = given(
        &format!("canvas show {TASK} garbled"),
        Given::Canvases(Ok(held(Vec::from([broken()?])))),
    );
    assert!(
        checked.output.contains(&format!(
            "error  {named}  not a canvas: missing field `title`\n"
        )),
        "{}",
        checked.output
    );
    assert_eq!(
        (shown.failed, shown.output),
        (
            true,
            format!("home: {named}: not a canvas: missing field `title`\n")
        )
    );
    Ok(())
}

#[test]
fn check_reports_each_stray_the_store_holds_by_its_place() -> Result<(), String>
{
    let checked = given(
        "check",
        Given::EveryCanvas(Ok(Held {
            canvases: Vec::from([fetched(
                TASK,
                "why-a-store",
                Ok(Some(prose_file()?)),
            )?]),
            strays: Vec::from([
                Stray {
                    place: String::from("over/there/Bad Name.json"),
                    reason: StrayReason::Unnamed,
                },
                Stray {
                    place: String::from("over/not-an-issue"),
                    reason: StrayReason::NotAnIssue,
                },
            ]),
        })),
    );
    assert!(checked.failed);
    assert!(
        checked.output.contains(
            "error  over/there/Bad Name.json  isn't a canvas: name it in lowercase words joined by `-`, or move it out of the issue\n\
             error  over/not-an-issue  isn't an issue: keep canvases under an issue's id, or remove it\n"
        ),
        "{}",
        checked.output
    );
    Ok(())
}
