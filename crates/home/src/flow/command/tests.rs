use crate::issue::storage::fresh;
use crate::issue::{
    ChecklistChange, ChecklistItem, Datum, Freshness, Issue, IssueChange,
    IssueReading, IssueState, Item, NewIssue, Observation, Part, ReadFailure,
    ReadStatus,
};
use crate::{DRAWS, Draws, IssueId, IssueKey, ProjectId, UnixMillis, UnixSecs};

use super::{
    Asked, CanvasWrite, Command, Context, Edit, Fetched, Given, Local, Need,
    Resolved, Source, Wanted, asked as dispatched, command, homeless, plan,
    stamp,
};
use crate::canvas::{
    BadName, Block, Canvas, CanvasError, CanvasFile, CanvasKey, CanvasName,
    Held, Stray, StrayReason,
};
use crate::flow::check::{Report, check as checked};
use crate::flow::load;
use crate::flow::resolve::in_folder;
use crate::flow::resolve::{Missing, Resolver};
use crate::flow::snapshot::Snapshot;
use crate::issue::body::log_line;
use crate::issue::storage::Inputs;
use crate::issue::storage::{File, one};

const FLOW: &str = include_str!("../test-flow.toml");
const ROOT: &str = "e22b7f7b2976650400965d4147ba771675f96076";
const FOLDER: &str = "7w85je";
const SPEC: &str = "0tm7zt01";
const TASK: &str = "0tm7zx0d";
const NOW: UnixMillis = UnixMillis(1_795_348_800_000);
const SECONDS: UnixSecs = NOW.secs();
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
    IssueId(String::from(stem))
}

fn folder() -> ProjectId {
    ProjectId::parse(FOLDER).unwrap_or(ProjectId::new(0))
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
            message: None,
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
    let resolved = in_folder(snapshot, &command, given.as_ref());
    saved(
        super::run(command, given, &resolved, snapshot, context),
        &home(),
    )
}

fn ran(inputs: &Inputs, line: &str) -> Outcome {
    let home = inputs.clone().read().snapshot;
    let asked = command(&words(line), &home, fresh(&home, SECONDS, &SEVEN));
    let resolved = in_folder(&home, &asked, None);
    saved(
        super::run(asked, None, &resolved, &home, &context()),
        inputs,
    )
}

fn check(inputs: &Inputs) -> Report {
    checked(&crate::issue::storage::snapshot(inputs.clone()))
}

fn context() -> Context {
    Context {
        now: NOW,
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
            project: folder(),
            id: id(TASK),
            changes: Vec::from([change]),
        })
    };
    assert_eq!(
        asked(&format!(
            "issue move home {TASK} stage slice --signed-off by the owner"
        )),
        changing(IssueChange::Move {
            part: Part::Stage,
            to: String::from("slice"),
            reason: String::from("by the owner"),
            signed_off: true,
        })
    );
    assert_eq!(
        asked(&format!("issue move home {TASK} status ready to go")),
        changing(IssueChange::Move {
            part: Part::Status,
            to: String::from("ready"),
            reason: String::from("to go"),
            signed_off: false,
        })
    );
    assert_eq!(
        asked(&format!("issue note home {TASK} it works")),
        changing(IssueChange::Note(String::from("it works")))
    );
    assert_eq!(
        asked("issue note elsewhere x it works"),
        Command::Wrong(String::from("no project is called elsewhere"))
    );
    assert_eq!(
        asked(&format!("issue body home {TASK} Why it matters.")),
        changing(IssueChange::Body(String::from("Why it matters.")))
    );
    assert_eq!(
        asked(&format!("issue checklist home {TASK} add the docs")),
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
            asked(&format!("issue checklist home {TASK} check 1")),
            asked(&format!("issue checklist home {TASK} uncheck 1")),
            asked(&format!("issue checklist home {TASK} remove 1")),
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
        asked(&format!("issue checklist home {TASK} check 0")),
        Command::Wrong(String::from(
            "0 is not an item's number, counted from 1"
        ))
    );
    assert_eq!(asked(&format!("issue set home {TASK}")), Command::Usage);
    assert_eq!(
        asked(&format!("issue move home {TASK} phase build")),
        Command::Usage
    );
}

#[test]
fn issue_set_names_a_change_for_each_pair() {
    let inputs = home();
    let asked = |line: &str| asked(&inputs, line);
    assert_eq!(
        asked(&format!("issue set home {TASK} a=1 b=2")),
        Command::Edit(Edit::Change {
            project: folder(),
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
        asked(&format!("issue set home {TASK} a")),
        Command::Wrong(String::from("\"a\" is not <field>=<value>"))
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
    let from_file =
        asked(&inputs, &format!("issue body home {TASK} --file why.md"));
    assert_eq!(
        from_file.needs(),
        Some(Need::Read(Source::File(String::from("why.md"))))
    );
    assert_eq!(
        from_file,
        Command::BodyFrom {
            issue: crate::IssueKey {
                project: folder(),
                id: id(TASK),
            },
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
        &format!("issue set home {TASK} {pair}"),
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
        format!("issue set home {TASK} triage.level=low"),
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
        || asked(&inputs, &format!("issue body home {TASK} --file why.md"));
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
    let wrote = ran(&home(), &format!("issue note home {TASK} seen"));
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
    let blocking =
        ran(&inputs, &format!("issue set home {SPEC} blocked_by={TASK}"));
    assert!(!blocking.failed, "{}", blocking.output);
    let cycle = ran(
        &after(&inputs, &blocking),
        &format!("issue set home {TASK} blocked_by={SPEC}"),
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
    let outcome = ran(&home(), &format!("issue note home {TASK} checked"));
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
        &format!(
            "issue move home {SPEC} stage slice --signed-off by the owner"
        ),
    );
    assert!(text(&usual).contains("stage = \"slice\"\n"));
    assert!(text(&usual).ends_with(
        "- 2026-11-22T12:00:00Z laptop·claude stage spec→slice: signed off: by the owner\n"
    ));
    let unusual = ran(
        &home(),
        &format!(
            "issue move home {SPEC} stage land --signed-off skipping ahead"
        ),
    );
    assert!(!unusual.failed);
    assert!(text(&unusual).ends_with(
        "stage spec→land: signed off: skipping ahead (unusual move)\n"
    ));
    let status = ran(&home(), &format!("issue move home {TASK} status ready"));
    assert!(text(&status).contains("status = \"ready\"\n"));
    assert!(text(&status).ends_with("status open→ready: \n"));
    let unknown =
        ran(&home(), &format!("issue move home {TASK} status started"));
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
            ran(&inputs, &format!("issue move home {SPEC} stage {to} ready"));
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
            &format!("issue move home {SPEC} stage {to} --signed-off"),
        );
        assert!(!signed.failed, "{}", signed.output);
        assert!(
            text(&signed).contains(&format!("stage {from}→{to}: signed off")),
            "{}",
            text(&signed)
        );
    }
    for from in ["review", "land"] {
        let rework =
            ran(&at(from), &format!("issue move home {SPEC} stage build"));
        assert!(!rework.failed, "{}", rework.output);
    }
}

#[test]
fn a_stage_or_status_is_moved_not_set() {
    let outcome = ran(&home(), &format!("issue set home {TASK} status=done"));
    assert!(outcome.failed && outcome.writes.is_empty());
}

#[test]
fn a_time_is_written_to_the_second_in_utc() {
    assert_eq!(stamp(SECONDS), "2026-11-22T12:00:00Z");
    assert_eq!(stamp(UnixSecs(0)), "1970-01-01T00:00:00Z");
}

#[test]
fn a_body_is_replaced_and_a_checklist_item_added_checked_and_removed() {
    let inputs = home();
    let said = |inputs: &Inputs, words_said: &str| ran(inputs, words_said);
    let described =
        said(&inputs, &format!("issue body home {TASK} Wire it up."));
    assert!(!described.failed, "{}", described.output);
    assert!(
        text(&described).contains("+++\n\nWire it up.\n\n## Notes\n"),
        "{}",
        text(&described)
    );
    let added = said(
        &after(&inputs, &described),
        &format!("issue checklist home {TASK} add the docs"),
    );
    let more = said(
        &after(&after(&inputs, &described), &added),
        &format!("issue checklist home {TASK} add the tests"),
    );
    let home_now = after(&after(&after(&inputs, &described), &added), &more);
    assert!(
        text(&more).contains(
            "## Checklist\n- [ ] the docs\n- [ ] the tests\n\n## Notes"
        ),
        "{}",
        text(&more)
    );
    let checked =
        said(&home_now, &format!("issue checklist home {TASK} check 2"));
    assert!(text(&checked).contains("- [ ] the docs\n- [x] the tests\n"));
    let removed = said(
        &after(&home_now, &checked),
        &format!("issue checklist home {TASK} remove 1"),
    );
    assert!(text(&removed).contains("## Checklist\n- [x] the tests\n\n"));
    let missing =
        said(&home_now, &format!("issue checklist home {TASK} check 3"));
    assert_eq!(
        (missing.failed, missing.output),
        (true, String::from("home: the checklist has no item 3\n"))
    );
}

#[test]
fn a_note_of_several_lines_is_indented_under_its_first() {
    let outcome = run(
        Command::Edit(Edit::Change {
            project: folder(),
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
    let noted = ran(&inputs, &format!("issue note home {TASK} seen"));
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
    let checked = ran(&inputs, &format!("issue checklist home {TASK} check 2"));
    assert!(!checked.failed, "{}", checked.output);
    assert_eq!(
        reread(&text(&checked)),
        Ok(Vec::from([item(false, "first"), item(true, "second")]))
    );
    assert!(text(&checked).contains("- [ ] an example\n"));
    assert!(text(&checked).contains("~~~\n- [ ] fenced\n~~~\n- [x] second\n"));
    let removed =
        ran(&inputs, &format!("issue checklist home {TASK} remove 1"));
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
        format!("issue note home {TASK} seen"),
        format!("issue move home {SPEC} stage slice --signed-off"),
        format!("issue move home {TASK} status ready"),
        format!("issue set home {TASK} title=Write-it-well"),
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
            project: folder(),
            id: id(TASK),
            changes: Vec::from([IssueChange::Set {
                field: String::from(field),
                value: Datum::Text(String::from("x")),
            }]),
        };
        let home = crate::issue::storage::snapshot(home());
        let observed = home.observation(folder(), &id(TASK));
        plan(&home, &edit, observed, &context()).map(|_| ())
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
    let outcome = ran(&inputs, &format!("issue note home {TASK} hello"));
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
            &format!("canvas put home {TASK} why-a-store --file why.json")
        ),
        Command::CanvasPut {
            key: canvas_key("why-a-store")?,
            from: Source::File(String::from("why.json")),
        }
    );
    assert_eq!(
        asked(&inputs, &format!("canvas put home {TASK} why-a-store")).needs(),
        Some(Need::Read(Source::Stdin))
    );
    Ok(())
}

#[test]
fn a_canvas_put_on_an_issue_writes_the_canvas_under_its_key_stamped_with_when_and_by_whom()
-> Result<(), Box<dyn core::error::Error>> {
    let outcome = given(
        &format!("canvas put home {TASK} why-a-store"),
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
            put(&["canvas", "put", "home", "0tm7zzzz", "why"], PROSE),
            String::from("home: no issue 0tm7zzzz in home\n"),
        ),
        (
            put(&["canvas", "put", "home", TASK, "Design Tree"], PROSE),
            String::from(
                "home: `Design Tree` isn't a canvas name: name it with lowercase words joined by `-`, such as `design-tree`\n",
            ),
        ),
        (
            put(&["canvas", "put", "home", TASK, "why"], r#"{"title": "t"}"#),
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
        &format!("canvas put home {TASK} why --file why.json"),
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
        asked(&inputs, &format!("canvas show home {TASK} why-a-store")).needs(),
        Some(Need::Canvases(wanted(Some(CanvasName::parse(
            "why-a-store"
        )?))))
    );
    assert_eq!(
        asked(&inputs, &format!("canvas show home {TASK}")).needs(),
        Some(Need::Canvases(wanted(None)))
    );
    assert_eq!(
        ran(&inputs, "canvas show home 0tm7zzzz").output,
        "home: no issue 0tm7zzzz in home\n"
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
        &format!("canvas show home {TASK} why-a-store"),
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
        &format!("canvas show home {TASK}"),
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
        &format!("canvas show home {TASK} gone"),
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
        &format!("canvas show home {TASK}"),
        Given::Canvases(Ok(held(Vec::new()))),
    );
    assert_eq!(
        (none.failed, none.output),
        (false, format!("home/{TASK} has no canvases\n"))
    );
    let failing = given(
        &format!("canvas show home {TASK}"),
        Given::Canvases(Err(String::from("projects: denied"))),
    );
    assert_eq!(
        (failing.failed, failing.output),
        (true, String::from("home: projects: denied\n"))
    );
}

const BLOCKS: &str = include_str!("blocks.json");

const KINDS: [&str; 7] = [
    "changes",
    "code",
    "diagram",
    "options",
    "progress",
    "prose",
    "statechart",
];

fn schema() -> Result<serde_json::Value, String> {
    let outcome = ran(&home(), "canvas schema");
    if outcome.failed {
        return Err(outcome.output);
    }
    serde_json::from_str(&outcome.output).map_err(|error| error.to_string())
}

fn examples() -> Result<Vec<serde_json::Value>, String> {
    let examples: Vec<serde_json::Value> =
        serde_json::from_str(BLOCKS).map_err(|error| error.to_string())?;
    Ok(examples
        .into_iter()
        .map(|example| {
            if example.get("title").is_some() {
                example
            } else {
                serde_json::json!({"title": "A block", "blocks": [example]})
            }
        })
        .collect())
}

#[test]
fn a_canvas_schema_prints_a_json_schema_with_a_definition_of_each_block_kind()
-> Result<(), String> {
    let schema = schema()?;
    let kinds: Vec<&str> = schema["$defs"]["Block"]["oneOf"]
        .as_array()
        .ok_or("no Block definition")?
        .iter()
        .filter_map(|variant| variant["required"][0].as_str())
        .collect::<alloc::collections::BTreeSet<&str>>()
        .into_iter()
        .collect();
    assert_eq!(kinds, KINDS, "{schema:#}");
    assert_eq!(schema["required"], serde_json::json!(["title", "blocks"]));
    Ok(())
}

#[test]
fn the_canvas_schema_and_canvas_put_agree_on_every_example_of_the_explain_skill_s_blocks()
-> Result<(), String> {
    let validator = jsonschema::validator_for(&schema()?)
        .map_err(|error| error.to_string())?;
    for example in examples()? {
        let unfit: Vec<String> = validator
            .iter_errors(&example)
            .map(|error| error.to_string())
            .collect();
        assert_eq!(unfit, Vec::<String>::new(), "{example}");
        let put = put_canvas(&example.to_string());
        assert_eq!(
            (put.failed, put.canvases.len()),
            (false, 1),
            "{example}: {}",
            put.output
        );
    }
    Ok(())
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
        (
            statechart(
                r#"{"id": "open", "label": "Open", "marked": "the owner reads it here before the worker starts"}"#,
                "open",
                "",
                "",
            ),
            "the label `the owner reads it here before the worker starts` is over 8 words: put the detail in prose",
        ),
    ];
    for (text, said) in cases {
        let outcome = given(
            &format!("canvas put home {TASK} turn"),
            Given::Text(Ok(text)),
        );
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
        &format!("canvas show home {TASK} turn"),
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
    let put = given(
        &format!("canvas put home {TASK} turn"),
        Given::Text(Ok(text)),
    );
    let shown = given(
        &format!("canvas show home {TASK} turn"),
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
        &format!("canvas show home {TASK} turn"),
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
        &format!("canvas put home {TASK} turn"),
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
        &format!("canvas put home {TASK} design"),
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
    refuses_each(cases);
}

#[test]
fn a_canvas_put_of_a_diagram_with_a_lone_node_misnumbered_steps_or_a_repeated_group_writes_nothing_and_says_what_to_change()
 {
    let cases = [
        (
            diagram(
                &[
                    node("core", "Core", 1),
                    node("store", "Store", 1),
                    node("cache", "Cache", 1),
                ]
                .join(", "),
                r#"{"from": "core", "to": "store", "step": 1}, {"from": "store", "to": "cache", "step": 2}"#,
            ),
            format!(
                "home: canvas design of home/{TASK}: node `cache` stands alone at step 1: bring the node in with the edge that connects it\n"
            ),
        ),
        (
            diagram(&node("core", "Core", 1), ""),
            format!(
                "home: canvas design of home/{TASK}: node `core` stands alone at step 1: bring the node in with the edge that connects it\n"
            ),
        ),
        (
            diagram(
                &[
                    node("core", "Core", 1),
                    node("store", "Store", 1),
                    node("cache", "Cache", 3),
                ]
                .join(", "),
                r#"{"from": "core", "to": "store", "step": 1}, {"from": "store", "to": "cache", "step": 3}"#,
            ),
            format!(
                "home: canvas design of home/{TASK}: a diagram skips step 2: number its steps from 1 with no gap\n"
            ),
        ),
        (
            diagram(
                &[node("core", "Core", 1), node("store", "Store", 1)]
                    .join(", "),
                r#"{"from": "core", "to": "store", "step": 1}"#,
            )
            .replace(
                r#"{"id": "shell", "label": "Shell"}"#,
                r#"{"id": "shell", "label": "Shell"}, {"id": "shell", "label": "Host"}"#,
            ),
            format!(
                "home: canvas design of home/{TASK}: two groups are called `shell`: give each group its own id\n"
            ),
        ),
    ];
    refuses_each(cases);
}

#[test]
fn a_canvas_put_of_a_diagram_step_of_0_is_refused_when_decoding_and_says_to_number_steps_from_1()
 {
    let text = diagram(
        &[node("core", "Core", 0), node("store", "Store", 1)].join(", "),
        r#"{"from": "core", "to": "store", "step": 1}"#,
    );
    refuses_each([(
        text,
        format!(
            "home: canvas design of home/{TASK}: not a canvas: a diagram has a step 0: number its steps from 1 at line 1 column 96\n"
        ),
    )]);
}

#[test]
fn a_canvas_schema_gives_a_diagram_step_a_minimum_of_1() -> Result<(), String> {
    let schema = schema()?;
    let steps: Vec<&serde_json::Value> = ["Node", "Edge"]
        .iter()
        .map(|part| &schema["$defs"][part]["properties"]["step"]["minimum"])
        .collect();
    assert_eq!(
        steps,
        [&serde_json::json!(1), &serde_json::json!(1)],
        "{schema:#}"
    );
    Ok(())
}

fn refuses_each<const N: usize>(cases: [(String, String); N]) {
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
fn a_canvas_put_of_a_diagram_without_groups_writes_it() {
    let outcome = put_canvas(
        r#"{"title": "Design", "blocks": [{"diagram": {"nodes": [{"id": "core", "label": "Core", "step": 1}, {"id": "store", "label": "Store", "step": 1}], "edges": [{"from": "core", "to": "store", "step": 1}]}}]}"#,
    );
    assert_eq!(
        (outcome.failed, outcome.canvases.len()),
        (false, 1),
        "{}",
        outcome.output
    );
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
        &format!("canvas show home {TASK} design"),
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
            &format!("canvas show home {TASK} design"),
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
fn a_canvas_put_with_more_done_than_total_fewer_than_two_options_a_pick_past_them_or_groups_out_of_order_writes_nothing_and_says_what_to_change()
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
            "`Where?` has fewer than two options: add the choices the pick is made from",
        ),
        (
            blocks(
                r#"{"options": {"question": "Where?", "options": [{"label": "Home folder", "gets": "history in git", "costs": "a sync per write"}], "pick": 0, "why": "it syncs"}}"#,
            ),
            "`Where?` has fewer than two options: add the choices the pick is made from",
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
        let outcome = given(
            &format!("canvas put home {TASK} stands"),
            Given::Text(Ok(text)),
        );
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
        &format!("canvas show home {TASK} stands"),
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
fn a_canvas_show_prints_options_as_a_list_with_the_pick_recommended_and_why()
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
                 - **Home folder** (recommended): gets history in git; costs a sync per write\n\
                 - **Database**: gets queries; costs a server to run\n\n\
                 Recommended **Home folder**: it syncs with the issues\n"
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
            issue: IssueId::new(issue).ok_or("issue")?,
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
                 error  canvas orphan of home/0v29xvc9  no issue 0v29xvc9 in home: write the canvas under an issue that exists, or remove it\n\
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
        &format!("canvas show home {TASK}"),
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
        &format!("canvas show home {TASK} garbled"),
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

const TRACKED: &str = "hyn162";
const MARKDOWN_ID: &str = "0tm7aaaa";
const TRACKER_ID: &str = "146";

fn project_file(name: &str, tracker: bool) -> String {
    let backends = if tracker {
        "[backends.tracker]\nadapter = \"github\"\nresource = \"owner/standards\"\n"
    } else {
        ""
    };
    format!(
        "name = \"{name}\"\ntitle = \"{name}\"\nroot = \"{ROOT}\"\n{backends}"
    )
}

fn filed(project: &str, id: &str) -> File {
    File {
        project: ProjectId::parse(project).unwrap_or(ProjectId::new(0)),
        stem: String::from(id),
        text: format!(
            "+++\nid = \"{id}\"\ntitle = \"Keep it\"\nkind = \"task\"\nstatus = \"open\"\n+++\n\n## What\n\n## Notes\n"
        ),
    }
}

fn two_trackers() -> Inputs {
    let mut inputs = home();
    inputs
        .projects
        .push((String::from(TRACKED), Ok(project_file("standards", true))));
    inputs.issues.push(filed(FOLDER, MARKDOWN_ID));
    inputs
}

fn kept_by_tracker(number: &str) -> Result<Observation, String> {
    let mut inputs = home();
    inputs.projects = Vec::from([(
        String::from(TRACKED),
        Ok(project_file("standards", false)),
    )]);
    inputs.issues = Vec::from([filed(TRACKED, MARKDOWN_ID)]);
    let project = ProjectId::parse(TRACKED).ok_or("project")?;
    let mut observation = crate::issue::storage::snapshot(inputs)
        .observation(project, &id(MARKDOWN_ID))
        .cloned()
        .ok_or("the tracker's issue doesn't read")?;
    match &mut observation.state {
        IssueState::Valid { issue, .. } => issue.id = id(number),
        _ => return Err(String::from("the tracker's issue isn't valid")),
    }
    Ok(observation)
}

struct Tracker {
    cached: Vec<Observation>,
    remote: Result<Vec<Observation>, ReadFailure>,
    refreshes: usize,
    reads: usize,
}

impl Tracker {
    fn holding(remote: Result<Vec<Observation>, ReadFailure>) -> Self {
        Self {
            cached: Vec::new(),
            remote,
            refreshes: 0,
            reads: 0,
        }
    }

    fn look(&self, key: &IssueKey) -> IssueReading {
        let record = self
            .cached
            .iter()
            .find(|observation| {
                observation.project() == key.project
                    && observation.id() == &key.id
            })
            .cloned();
        IssueReading {
            status: ReadStatus {
                freshness: Freshness::Stale,
                complete: record.is_some(),
                observed_at: None,
                failure: None,
            },
            record,
        }
    }

    fn read(&mut self, key: &IssueKey) -> IssueReading {
        self.reads += 1;
        let read = self.remote.clone().map(|remote| {
            remote.into_iter().find(|observation| {
                observation.project() == key.project
                    && observation.id() == &key.id
            })
        });
        match read {
            Ok(Some(found)) => {
                self.cached.retain(|observation| {
                    observation.project() != key.project
                        || observation.id() != &key.id
                });
                self.cached.push(found);
                self.look(key)
            }
            Ok(None) => self.look(key),
            Err(failure) => {
                let mut looked = self.look(key);
                looked.status.failure = Some(failure);
                looked
            }
        }
    }

    fn refresh(&mut self) -> Result<(), ReadFailure> {
        self.refreshes += 1;
        self.cached = self.remote.clone()?;
        Ok(())
    }
}

fn resolved_by(
    home: &Snapshot,
    tracker: &mut Tracker,
    asked: &Command,
    given: Option<&Given>,
) -> Resolved {
    let on_tracker = |project: ProjectId| {
        matches!(
            crate::tracker::select(home, project),
            Some(crate::tracker::Tracker::Configured(_))
        )
    };
    let mut steps = Resolver::new(asked.mode());
    asked
        .issues(given)
        .into_iter()
        .map(|key| {
            loop {
                let looked = if !on_tracker(key.project) {
                    home.reading(&key)
                } else if steps.fresh(key.project) {
                    tracker.read(&key)
                } else {
                    tracker.look(&key)
                };
                match steps.step(&super::named(home, key.project), &key, looked)
                {
                    Ok(reading) => break (key, Ok(reading)),
                    Err(Missing::Unknown(reason)) => break (key, Err(reason)),
                    Err(Missing::Refresh(project)) => {
                        let outcome = if on_tracker(project) {
                            tracker.refresh()
                        } else {
                            Ok(())
                        };
                        steps.refreshed(project, outcome);
                    }
                }
            }
        })
        .collect()
}

fn tracked_run(
    tracker: &mut Tracker,
    line: &str,
    given: Option<Given>,
) -> Outcome {
    let inputs = two_trackers();
    let home = crate::issue::storage::snapshot(inputs.clone());
    let asked = command(&words(line), &home, None);
    let resolved = resolved_by(&home, tracker, &asked, given.as_ref());
    saved(
        super::run(asked, given, &resolved, &home, &context()),
        &inputs,
    )
}

fn key_of(project: &str, id: &str, name: &str) -> Result<CanvasKey, String> {
    Ok(CanvasKey {
        project: ProjectId::parse(project).ok_or("project")?,
        issue: self::id(id),
        name: CanvasName::parse(name).map_err(|error| error.to_string())?,
    })
}

#[test]
fn a_canvas_put_and_show_name_a_markdown_issue_and_a_tracker_issue_alike()
-> Result<(), Box<dyn core::error::Error>> {
    for (project, folder, id) in [
        ("home", FOLDER, MARKDOWN_ID),
        ("standards", TRACKED, TRACKER_ID),
    ] {
        let mut tracker =
            Tracker::holding(Ok(Vec::from([kept_by_tracker(TRACKER_ID)?])));
        tracker.refresh()?;
        let put = tracked_run(
            &mut tracker,
            &format!("canvas put {project} {id} why-a-store"),
            Some(Given::Text(Ok(String::from(PROSE)))),
        );
        let key = key_of(folder, id, "why-a-store")?;
        assert_eq!(
            (put.failed, put.output, put.canvases),
            (
                false,
                format!("wrote canvas why-a-store of {project}/{id}\n"),
                Vec::from([CanvasWrite {
                    key: key.clone(),
                    file: CanvasFile {
                        written: NOW,
                        by: String::from("laptop·claude"),
                        canvas: why_a_store("Why a store")?,
                    },
                }])
            )
        );
        let shown = tracked_run(
            &mut tracker,
            &format!("canvas show {project} {id}"),
            Some(Given::Canvases(Ok(held(Vec::from([Fetched {
                key,
                got: Ok(Some(CanvasFile {
                    written: NOW,
                    by: String::from("laptop·claude"),
                    canvas: why_a_store("Why a store")?,
                })),
            }]))))),
        );
        assert_eq!(
            (shown.failed, shown.output, tracker.refreshes),
            (
                false,
                String::from(
                    "# Why a store\n\nThe core decides; the store **keeps**.\n"
                ),
                1
            )
        );
    }
    Ok(())
}

#[test]
fn an_id_the_cache_lacks_refreshes_its_tracker_once_and_then_resolves()
-> Result<(), String> {
    let mut tracker =
        Tracker::holding(Ok(Vec::from([kept_by_tracker(TRACKER_ID)?])));
    let put = tracked_run(
        &mut tracker,
        &format!("canvas put standards {TRACKER_ID} why-a-store"),
        Some(Given::Text(Ok(String::from(PROSE)))),
    );
    assert_eq!(
        (
            put.failed,
            put.output,
            put.canvases.len(),
            tracker.refreshes
        ),
        (
            false,
            format!("wrote canvas why-a-store of standards/{TRACKER_ID}\n"),
            1,
            1
        )
    );
    Ok(())
}

#[test]
fn an_id_still_missing_after_one_refresh_is_no_issue_in_its_project()
-> Result<(), String> {
    let mut tracker =
        Tracker::holding(Ok(Vec::from([kept_by_tracker(TRACKER_ID)?])));
    for (line, said) in [
        (
            "canvas put standards 147 why-a-store",
            "home: no issue 147 in standards\n",
        ),
        (
            "canvas put home 0tm7bbbb why-a-store",
            "home: no issue 0tm7bbbb in home\n",
        ),
        (
            "issue show standards 147",
            "home: no issue 147 in standards\n",
        ),
        (
            "issue note standards 147 seen",
            "home: no issue 147 in standards\n",
        ),
    ] {
        tracker.refreshes = 0;
        let put = tracked_run(
            &mut tracker,
            line,
            Some(Given::Text(Ok(String::from(PROSE)))),
        );
        assert_eq!(
            (put.failed, put.output.as_str(), put.canvases.len()),
            (true, said, 0),
            "{line}"
        );
        assert!(tracker.refreshes <= 1, "{line}");
    }
    Ok(())
}

#[test]
fn offline_an_id_the_cache_lacks_fails_naming_home_issue_list() {
    let mut unreachable = Tracker::holding(Err(ReadFailure::Unavailable));
    let put = tracked_run(
        &mut unreachable,
        "canvas put standards 146 why-a-store",
        Some(Given::Text(Ok(String::from(PROSE)))),
    );
    assert_eq!(
        (put.failed, put.output, unreachable.refreshes),
        (
            true,
            String::from(
                "home: no issue 146 in standards is cached, and its tracker didn't refresh: issue or tracker unavailable or inaccessible; run `home issue list standards` once it's reachable\n"
            ),
            1
        )
    );
    let mut reachable = Tracker::holding(Ok(Vec::new()));
    let offline =
        tracked_run(&mut reachable, "issue show standards 146 --offline", None);
    assert_eq!(
        (offline.failed, offline.output, reachable.refreshes),
        (
            true,
            String::from(
                "home: no issue 146 in standards is cached; `home issue list standards` refreshes the cache\n"
            ),
            0
        )
    );
}

#[test]
fn issue_show_reads_a_markdown_issue_and_a_tracker_issue_by_the_same_form()
-> Result<(), Box<dyn core::error::Error>> {
    for (project, folder, id) in [
        ("home", FOLDER, MARKDOWN_ID),
        ("standards", TRACKED, TRACKER_ID),
    ] {
        let mut tracker =
            Tracker::holding(Ok(Vec::from([kept_by_tracker(TRACKER_ID)?])));
        let shown = tracked_run(
            &mut tracker,
            &format!("issue show {project} {id}"),
            None,
        );
        let reading: serde_json::Value = serde_json::from_str(&shown.output)?;
        let record = &reading["record"]["state"]["Valid"]["issue"];
        assert_eq!(
            (
                shown.failed,
                record["id"].as_str(),
                record["project"].as_str(),
                record["title"].as_str(),
            ),
            (false, Some(id), Some(folder), Some("Keep it")),
            "{}",
            shown.output
        );
    }
    Ok(())
}

#[test]
fn issue_show_of_a_cached_issue_its_tracker_can_t_read_reports_the_failure()
-> Result<(), Box<dyn core::error::Error>> {
    let mut tracker = Tracker::holding(Err(ReadFailure::RateLimited));
    tracker.cached = Vec::from([kept_by_tracker(TRACKER_ID)?]);
    let shown = tracked_run(&mut tracker, "issue show standards 146", None);
    let reading: serde_json::Value = serde_json::from_str(&shown.output)?;
    assert_eq!(
        (
            shown.failed,
            &reading["status"]["failure"],
            tracker.reads,
            tracker.refreshes
        ),
        (true, &serde_json::json!("RateLimited"), 1, 0)
    );
    Ok(())
}

#[test]
fn a_fresh_issue_show_of_a_cached_issue_reads_it_and_never_lists_its_tracker()
-> Result<(), Box<dyn core::error::Error>> {
    let mut retitled = kept_by_tracker(TRACKER_ID)?;
    if let IssueState::Valid { issue, .. } = &mut retitled.state {
        issue.title = String::from("Read just now");
    }
    let mut tracker = Tracker::holding(Ok(Vec::from([retitled])));
    tracker.cached = Vec::from([kept_by_tracker(TRACKER_ID)?]);
    let shown = tracked_run(&mut tracker, "issue show standards 146", None);
    let reading: serde_json::Value = serde_json::from_str(&shown.output)?;
    assert_eq!(
        (
            shown.failed,
            reading["record"]["state"]["Valid"]["issue"]["title"].as_str(),
            tracker.reads,
            tracker.refreshes
        ),
        (false, Some("Read just now"), 1, 0)
    );
    Ok(())
}

#[test]
fn a_fresh_issue_show_the_tracker_lacks_lists_its_tracker_once()
-> Result<(), String> {
    let mut tracker =
        Tracker::holding(Ok(Vec::from([kept_by_tracker(TRACKER_ID)?])));
    let shown = tracked_run(&mut tracker, "issue show standards 147", None);
    assert_eq!(
        (shown.failed, shown.output, tracker.reads, tracker.refreshes),
        (
            true,
            String::from("home: no issue 147 in standards\n"),
            1,
            1
        )
    );
    Ok(())
}

#[test]
fn a_write_commits_only_when_it_touches_a_home_file() {
    let outcome = super::Outcome {
        changes: Vec::new(),
        canvases: Vec::new(),
        message: Some(String::from("Write canvas why")),
        output: String::new(),
        failed: false,
    };
    assert_eq!(outcome.commit_message(&[]), None);
    assert_eq!(
        outcome.commit_message(&[String::from("why.json")]),
        Some("Write canvas why")
    );
}
