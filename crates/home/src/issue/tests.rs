use std::collections::BTreeMap;

use crate::flow::Flow;
use crate::issue::{
    ChecklistChange, ChecklistItem, Datum, EditId, Entry, Issue, IssueChange,
    Item, LogEvent, NewIssue, Part, Section,
};
use crate::{
    Checkout, CheckoutLocation, Cloning, Home, HomeRefusal, HomeSync, IssueId,
    Origin, ProjectId, UnixSecs,
};

use super::{
    Branch, Edit, EditData, Mode, Move, PendingEdits, Remote, SessionUpdate,
    branch, message, start,
};
use crate::flow::command;
use crate::flow::load;
use crate::flow::snapshot::Snapshot;
use crate::issue::body::log_line;
use crate::issue::storage::Inputs;
use crate::issue::storage::{self, File};

fn id_id() -> IssueId {
    IssueId {
        authority: String::from("local"),
        key: String::from("00000001"),
    }
}
const OPENED: u64 = 1_794_996_720;
const SIGNED: &str = "2026-11-18T10:12:00Z laptop·iPhone";

fn text(value: &str) -> String {
    String::from(value)
}

fn project() -> ProjectId {
    ProjectId::parse("7r5n23").unwrap_or(ProjectId::new(0))
}

fn snapshot_of(files: &[File]) -> Snapshot {
    snapshot_with(files, "")
}

fn snapshot_with(files: &[File], selection: &str) -> Snapshot {
    crate::issue::storage::snapshot(Inputs {
        unavailable: Vec::new(),
        observed_at: None,
        flow: Ok(text(include_str!("../flow/test-flow.toml"))),
        projects: Vec::from([(
            project().to_string(),
            Ok(format!(
                "name = \"conductor\"\ntitle = \"Conductor\"\n\
                 root = \"e22b7f7b2976650400965d4147ba771675f96076\"\n{selection}",
            )),
        )]),
        issues: files.to_vec(),
        unread: Vec::new(),
    })
}

fn filed(id: &IssueId, status: &str, more: &str) -> File {
    File {
        project: project(),
        stem: id.reference(),
        text: written()
            .replace(&id_id().reference(), &id.reference())
            .replace(
                "status = \"active\"\n",
                &format!("status = \"{status}\"\n{more}"),
            ),
    }
}

fn issue_file() -> File {
    File {
        project: project(),
        stem: id_id().reference(),
        text: written(),
    }
}

fn flow() -> Flow {
    load(include_str!("../flow/test-flow.toml")).unwrap_or_else(|_| Flow {
        default_kind: String::new(),
        statuses: Vec::new(),
        fields: Vec::new(),
        kinds: Vec::new(),
        relations: Vec::new(),
        stages: Vec::new(),
        rules: Vec::new(),
    })
}

fn among<T>(files: &[File], within: impl FnOnce(&Snapshot) -> T) -> T {
    within(&snapshot_of(files))
}

fn observed(text: &str) -> super::Observed {
    storage::load(
        &flow(),
        &super::Read {
            project: project(),
            issue: id_id(),
        },
        text,
    )
    .observed
}

fn parse(text: &str) -> Result<Issue, String> {
    observed(text).reading.and_then(|observation| {
        observation.editable().map(|(issue, _)| issue.clone())
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Applied {
    Write { text: String, warnings: Vec<String> },
    AlreadyRecorded,
    Unchanged,
    Gone,
    Unreadable(String),
    Refused(String),
}

fn rendered(
    applied: Result<super::Applied, super::NotApplied>,
    current: Option<&str>,
    edit: &Edit,
) -> Applied {
    match applied {
        Ok(super::Applied { change, warnings }) => {
            let document = current.and_then(|text| {
                storage::load(&flow(), &edit.read(), text).document
            });
            storage::encode(&flow(), &change, document, Some(edit.id))
                .map_or_else(Applied::Refused, |file| Applied::Write {
                    text: file.text,
                    warnings,
                })
        }
        Err(super::NotApplied::AlreadyRecorded) => Applied::AlreadyRecorded,
        Err(super::NotApplied::Unchanged) => Applied::Unchanged,
        Err(super::NotApplied::Gone) => Applied::Gone,
        Err(super::NotApplied::Unreadable(reason)) => {
            Applied::Unreadable(reason)
        }
        Err(super::NotApplied::Refused(reason)) => Applied::Refused(reason),
    }
}

fn apply(current: Option<&str>, edit: &Edit, mode: Mode) -> Applied {
    let input = current.map(observed);
    let applied =
        among(&[], |home| super::apply(home, input.as_ref(), edit, mode));
    rendered(applied, current, edit)
}

fn was(
    current: Option<&str>,
    change: &command::Edit,
) -> Vec<Option<Vec<String>>> {
    let input = current.and_then(|text| {
        observed(text)
            .reading
            .ok()
            .and_then(|observation| observation.issue().cloned())
    });
    super::values(input.as_ref(), change)
}

fn note(line: &str) -> Entry {
    Entry {
        at: None,
        by: None,
        event: LogEvent::Note,
        text: text(line),
    }
}

fn notes(issue: &Issue) -> Vec<String> {
    issue
        .log
        .iter()
        .map(|entry| {
            let line = log_line(entry);
            line.strip_prefix("- ")
                .map_or_else(|| line.clone(), String::from)
        })
        .collect()
}

fn issue() -> Issue {
    Issue {
        created: Some(UnixSecs(OPENED).millis()),
        id: id_id(),
        project: project(),
        title: text("Sign in to a subscription from the app"),
        kind: text("task"),
        status: text("active"),
        stage: None,
        fields: BTreeMap::from([
            (text("priority"), Datum::Text(text("high"))),
            (
                text("labels"),
                Datum::Texts(Vec::from([text("app"), text("credentials")])),
            ),
            (text("branch"), Datum::Text(String::from("issue/00000001"))),
            (text("created"), Datum::Text(text("2026-11-18T10:12:00Z"))),
        ]),
        links: BTreeMap::new(),
        body: text("What to build and why."),
        sections: Vec::from([Section {
            heading: String::new(),
            text: text("What to build and why."),
        }]),
        branch: String::from("issue/00000001"),
        canvases: Vec::new(),
        checklist: Vec::from([
            ChecklistItem {
                done: true,
                text: text("the protocol"),
            },
            ChecklistItem {
                done: false,
                text: text("the browser scenario"),
            },
        ]),
        notes: Vec::new(),
        log: Vec::from([note(
            "2026-11-18 laptop · codex · api: built the protocol; browser scenario\nleft; landed on the issue branch.",
        )]),
    }
}

fn written() -> String {
    String::from(
        "+++\n\
         id = \"00000001\"\n\
         title = \"Sign in to a subscription from the app\"\n\
         status = \"active\"\n\
         priority = \"high\"\n\
         labels = [\"app\", \"credentials\"]\n\
         branch = \"issue/00000001\"\n\
         created = 2026-11-18T10:12:00Z\n\
         +++\n\
         \n\
         What to build and why.\n\
         \n\
         ## Checklist\n\
         - [x] the protocol\n\
         - [ ] the browser scenario\n\
         \n\
         ## Notes\n\
         - 2026-11-18 laptop · codex · api: built the protocol; browser scenario\n  \
         left; landed on the issue branch.\n",
    )
}

fn hand_edited() -> String {
    String::from(
        "+++\n\
         id = \"00000001\"\n\
         # set by hand\n\
         title = \"Sign in\"\n\
         status = \"open\"   # open, ready, active, review, done, dropped\n\
         +++\n\
         \n\
         Why.\n\
         \n\
         ## Checklist\n\
         - [X] the protocol\n\
         \n\
         ## Notes\n\
         - first\n\
         \n\
         ## Design\n\
         Kept as written.\n",
    )
    .replace('\n', "\r\n")
}

fn edit_of(change: &command::Edit) -> EditId {
    EditId(format!("{change:?}").bytes().fold(1_000, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u128::from(byte))
    }))
}

fn edit(change: command::Edit) -> Edit {
    Edit {
        id: edit_of(&change),
        data: EditData {
            project: project(),
            at: UnixSecs(OPENED),
            by: text("laptop·iPhone"),
            edit: change,
            was: Vec::new(),
        },
    }
}

fn creating(new: NewIssue) -> command::Edit {
    command::Edit::New {
        id: id_id(),
        issue: new,
    }
}

fn changing(change: IssueChange) -> command::Edit {
    command::Edit::Change {
        id: id_id(),
        changes: Vec::from([change]),
    }
}

fn changing_to(change: IssueChange) -> Edit {
    edit(changing(change))
}

fn file_of(applied: Applied) -> Result<String, String> {
    match applied {
        Applied::Write { text, .. } => Ok(unmarked(&text)),
        other => Err(format!("expected a file, not {other:?}")),
    }
}

fn unmarked(text: &str) -> String {
    text.lines()
        .filter(|line| !line.starts_with("# conductor-edit: "))
        .map(|line| format!("{line}\n"))
        .collect::<Vec<String>>()
        .concat()
}

fn moving_to(status: &str) -> IssueChange {
    IssueChange::Move {
        part: Part::Status,
        to: text(status),
        reason: String::new(),
        signed_off: false,
    }
}

fn setting(field: &str, value: &str) -> IssueChange {
    IssueChange::Set {
        field: text(field),
        value: Datum::Text(text(value)),
    }
}

fn retitling(title: &str) -> IssueChange {
    IssueChange::Title(text(title))
}

fn listing(field: &str, values: &[&str]) -> IssueChange {
    IssueChange::Set {
        field: text(field),
        value: Datum::Texts(values.iter().map(|value| text(value)).collect()),
    }
}

fn adding(item: &str) -> IssueChange {
    IssueChange::Checklist(ChecklistChange::Add(text(item)))
}

const fn at(index: usize, seen: Option<Vec<ChecklistItem>>) -> Item {
    Item { index, seen }
}

#[test]
fn an_issue_file_reads_as_the_flow_declares_it() {
    assert_eq!(parse(&written()), Ok(issue()));
}

#[test]
fn a_change_rewrites_only_its_own_lines_of_a_hand_edited_file()
-> Result<(), String> {
    let unix = hand_edited().replace("\r\n", "\n");
    let moved = file_of(apply(
        Some(&hand_edited()),
        &changing_to(moving_to("review")),
        Mode::Record,
    ))?;
    let added = file_of(apply(
        Some(&moved),
        &changing_to(adding("docs")),
        Mode::Record,
    ))?;
    let noted = file_of(apply(
        Some(&added),
        &changing_to(IssueChange::Note(text("second"))),
        Mode::Record,
    ))?;
    let labelled = file_of(apply(
        Some(&noted),
        &changing_to(listing("labels", &["app"])),
        Mode::Record,
    ))?;
    assert_eq!(
        labelled,
        unix.replace(
                "status = \"open\"   # open, ready, active, review, done, dropped\n",
                "status = \"review\"   # open, ready, active, review, done, dropped\nlabels = [\"app\"]\n"
            )
            .replace("- [X] the protocol\n", "- [X] the protocol\n- [ ] docs\n")
            .replace(
                "- first\n",
                &format!(
                    "- first\n- {SIGNED} status open→review: \n\
                     - {SIGNED} note: second\n- {SIGNED} note: set labels=app.\n"
                )
            ),
        "the comment, the key, the table, the [X] and the section stay"
    );
    let read = parse(&hand_edited()).map(|issue| {
        (
            issue.status.clone(),
            issue.body.clone(),
            issue.checklist.len(),
            notes(&issue),
        )
    });
    assert_eq!(
        read,
        Ok((text("open"), text("Why."), 1, Vec::from([text("first")])))
    );
    Ok(())
}

#[test]
fn an_item_is_checked_and_removed_by_its_place() -> Result<(), String> {
    let checked = file_of(apply(
        Some(&written()),
        &changing_to(IssueChange::Checklist(ChecklistChange::Check {
            item: at(1, None),
            done: true,
        })),
        Mode::Record,
    ))?;
    let removed = file_of(apply(
        Some(&checked),
        &changing_to(IssueChange::Checklist(ChecklistChange::Remove(at(
            0, None,
        )))),
        Mode::Record,
    ))?;
    assert_eq!(
        parse(&removed).map(|read| read.checklist),
        Ok(Vec::from([ChecklistItem {
            done: true,
            text: text("the browser scenario"),
        }]))
    );
    assert_eq!(
        apply(
            Some(&removed),
            &changing_to(IssueChange::Checklist(ChecklistChange::Remove(at(
                5, None
            )))),
            Mode::Record
        ),
        Applied::Refused(text("the checklist has no item 6"))
    );
    Ok(())
}

#[test]
fn an_issue_id_its_file_and_its_status_must_agree() {
    assert_eq!(id_id().to_string(), "local~00000001");
    assert_eq!(
        crate::layout::issue_file(&project(), &id_id().reference()),
        "projects/7r5n23/issues/00000001.md"
    );
    assert_eq!(
        storage::decode(
            &flow(),
            &File {
                project: project(),
                stem: IssueId {
                    authority: String::from("local"),
                    key: String::from("00000002")
                }
                .to_string(),
                text: written()
            }
        )
        .map(|_| ())
        .map_err(|problem| problem.to_string()),
        Err(text("id: is not its file's name, an issue id"))
    );
    assert_eq!(
        parse(&written().replace("active", "doing")).map(|_| ()),
        Err(text("status: no status is called \"doing\""))
    );
    assert_eq!(
        parse("no front matter").map(|_| ()),
        Err(text("it has no +++ front matter"))
    );
}

fn wanted() -> NewIssue {
    NewIssue {
        project: project(),
        kind: text("task"),
        title: text("  Sign in  "),
        fields: Vec::from([(
            text("labels"),
            Datum::Texts(Vec::from([text("app")])),
        )]),
    }
}

#[test]
fn creating_writes_what_home_issue_new_does_and_again_changes_nothing()
-> Result<(), String> {
    let create = edit(creating(wanted()));
    let opened = file_of(apply(None, &create, Mode::Record))?;
    assert_eq!(
        opened,
        format!(
            "+++\n\
             id = \"00000001\"\n\
             title = \"Sign in\"\n\
             kind = \"task\"\n\
             status = \"open\"\n\
             labels = [\"app\"]\n\
             created = 2026-11-18T10:12:00Z\n\
             +++\n\
             \n\
             ## What\n\
             \n\
             ## Acceptance\n\
             \n\
             ## Notes\n\
             \n\
             - {SIGNED} note: opened.\n"
        ),
        "the kind's sections, its first idle status, a signed log"
    );
    let Applied::Write { text: marked, .. } =
        apply(None, &create, Mode::Record)
    else {
        return Err(text("expected a file"));
    };
    assert_eq!(
        apply(
            Some(&marked),
            &create,
            Mode::Replay {
                changed: UnixSecs(0)
            }
        ),
        Applied::AlreadyRecorded
    );
    Ok(())
}

#[test]
fn a_note_is_signed_kept_twice_when_made_twice_and_replayed_once()
-> Result<(), String> {
    let note = || changing_to(IssueChange::Note(text("done")));
    let first = note();
    let Applied::Write { text: once, .. } =
        apply(Some(&written()), &first, Mode::Record)
    else {
        return Err(text("expected a file"));
    };
    let again = Edit {
        id: EditId(2),
        ..note()
    };
    let twice = file_of(apply(Some(&once), &again, Mode::Record))?;
    let noted = |file: &str| parse(file).map(|read| notes(&read));
    assert_eq!(
        noted(&twice).map(|all| all.into_iter().skip(1).collect::<Vec<_>>()),
        Ok(Vec::from([
            format!("{SIGNED} note: done"),
            format!("{SIGNED} note: done"),
        ]))
    );
    assert_eq!(
        apply(
            Some(&once),
            &first,
            Mode::Replay {
                changed: UnixSecs(OPENED + 9)
            }
        ),
        Applied::AlreadyRecorded,
        "a replay finds the note it made already"
    );
    Ok(())
}

#[test]
fn a_replayed_field_yields_to_a_newer_change_by_someone_else() {
    let done = Edit {
        data: EditData {
            was: was(Some(&written()), &changing(moving_to("done"))),
            ..changing_to(moving_to("done")).data
        },
        ..changing_to(moving_to("done"))
    };
    assert_eq!(done.data.was, [Some(Vec::from([text("active")]))]);
    let reviewed = written().replace("\"active\"", "\"review\"");
    let after = Mode::Replay {
        changed: UnixSecs(OPENED + 60),
    };
    let before = Mode::Replay {
        changed: UnixSecs(OPENED - 60),
    };
    let closed = |file: &str, mode| {
        file_of(apply(Some(file), &done, mode))
            .and_then(|next| parse(&next))
            .map(|read| read.status)
    };
    assert_eq!(
        closed(&reviewed, after),
        Ok(text("review")),
        "someone moved it to review after the edit was made"
    );
    assert_eq!(closed(&reviewed, before), Ok(text("done")));
    assert_eq!(
        closed(&written(), after),
        Ok(text("done")),
        "a later change to another field leaves the status to the edit"
    );
    assert_eq!(
        apply(None, &done, after),
        Applied::Gone,
        "a change to an issue the home no longer has is dropped"
    );
    assert!(
        matches!(
            apply(Some("not an issue"), &done, after),
            Applied::Unreadable(_)
        ),
        "a file that no longer reads keeps the edit waiting"
    );
}

fn context(home: &Snapshot, edit: EditId) -> super::Context<'_> {
    super::Context {
        remote: Some("origin"),
        home,
        host: "stone",
        at: UnixSecs(OPENED),
        edit,
    }
}

#[test]
fn an_issue_opens_in_a_listed_project_on_a_host_with_a_home()
-> Result<(), HomeRefusal> {
    let home = snapshot_of(&[]);
    let listed = context(&home, EditId(1));
    let nowhere = super::Context {
        remote: None,
        ..context(&home, EditId(1))
    };
    assert_eq!(
        nowhere.open(id_id(), wanted(), "iPhone").map(|_| ()),
        Err(HomeRefusal::NoHome)
    );
    assert_eq!(
        listed
            .open(
                id_id(),
                NewIssue {
                    project: ProjectId::new(1),
                    ..wanted()
                },
                "iPhone"
            )
            .map(|_| ()),
        Err(HomeRefusal::UnknownProject)
    );
    let untitled = listed.open(
        id_id(),
        NewIssue {
            title: text(" "),
            ..wanted()
        },
        "iPhone",
    )?;
    assert_eq!(
        among(&[], |home| untitled
            .decide(None, home, &PendingEdits::default())
            .map(|_| ())),
        Err(HomeRefusal::IssueRefused {
            reason: text("an issue needs a title")
        })
    );
    Ok(())
}

#[test]
fn a_change_finds_its_issues_project_on_a_host_with_a_home() {
    let home = snapshot_of(&[issue_file()]);
    let listed = context(&home, EditId(1));
    assert_eq!(
        listed
            .edit(id_id(), IssueChange::Note(text("x")), "iPhone")
            .map(|record| record.project()),
        Ok(project())
    );
    assert_eq!(
        listed
            .edit(
                IssueId {
                    authority: String::from("local"),
                    key: String::from("00000007")
                },
                IssueChange::Note(text("x")),
                "iPhone"
            )
            .map(|_| ()),
        Err(HomeRefusal::UnknownIssue)
    );
    let nowhere = super::Context {
        remote: None,
        ..listed
    };
    assert_eq!(
        nowhere
            .edit(id_id(), IssueChange::Note(text("x")), "iPhone")
            .map(|_| ()),
        Err(HomeRefusal::NoHome)
    );
}

#[test]
fn a_configured_tracker_refuses_local_creates_edits_and_previously_pending_replay()
-> Result<(), HomeRefusal> {
    let tracked = snapshot_with(
        &[issue_file()],
        "[backends.tracker]\nadapter = \"github\"\nresource = \"owner/issues\"\n",
    );
    let access = context(&tracked, EditId(1));
    let change = retitling("Direct edit");
    let (pending, _) =
        recorded(change.clone(), EditId(2), &observed(&written()))?;
    for attempt in [
        access.open(id_id(), wanted(), "device"),
        access.edit(id_id(), change, "device"),
    ] {
        assert!(
            matches!(attempt, Err(HomeRefusal::IssueRefused { reason }) if reason.contains("configured tracker"))
        );
    }
    assert!(
        matches!(pending.replay(&tracked, Ok(Some(&observed(&written()))), None), Err(super::NoCommit::Waiting(reason)) if reason.contains("configured tracker"))
    );
    Ok(())
}

#[test]
fn each_change_is_its_own_commit() {
    assert_eq!(
        [
            message(&creating(wanted())),
            message(&changing(moving_to("review"))),
            message(&changing(setting("priority", "low"))),
            message(&changing(IssueChange::Note(text("x")))),
        ],
        [
            String::from("Open issue local~00000001: Sign in"),
            String::from("Move issue local~00000001 to review"),
            String::from("Set the priority of issue local~00000001 to low"),
            String::from("Note on issue local~00000001"),
        ]
    );
}

fn home(checkouts: Vec<Checkout>, issues: Vec<Issue>) -> Home {
    Home {
        remote: text("file:///home.git"),
        synced_secs: None,
        sync: HomeSync::Synced,
        unpushed: 0,
        flow: Some(Box::new(flow())),
        projects: Vec::from([crate::Project {
            id: project(),
            name: text("conductor"),
            title: text("Conductor"),
            remote: None,
            profiles: Vec::new(),
            provides: Vec::new(),
            checkouts,
            cloning: Cloning::Idle,
            issues: issues
                .into_iter()
                .map(|issue| super::Observation {
                    state: super::IssueState::Valid {
                        issue: Box::new(issue.clone()),
                        body: super::body::Body::of(""),
                    },
                    ..super::Observation::mapped(
                        issue,
                        super::body::Body::of(""),
                    )
                })
                .collect(),
        }]),
    }
}

fn checkout() -> Checkout {
    Checkout {
        path: CheckoutLocation(text("/src/conductor")),
        origin: Origin::Found,
    }
}

#[test]
fn a_session_on_an_issue_starts_in_its_projects_checkout_with_a_brief() {
    let started = start(
        Some(&home(Vec::from([checkout()]), Vec::from([issue()]))),
        id_id(),
    );
    assert_eq!(
        started
            .as_ref()
            .map(|(_, at)| (at.checkout.clone(), at.branch.clone())),
        Ok((
            CheckoutLocation(text("/src/conductor")),
            String::from("refs/heads/issue/00000001"),
        ))
    );
    let brief = started.map_or_else(|_| String::new(), |(_, at)| at.brief);
    for part in [
        "Sign in to a subscription from the app",
        "Conductor",
        "What to build and why.",
        "- [x] the protocol",
        "- [ ] the browser scenario",
        "built the protocol",
        "issue/00000001",
    ] {
        assert!(brief.contains(part), "the brief has {part:?}: {brief}");
    }
    assert_eq!(
        start(Some(&home(Vec::new(), Vec::from([issue()]))), id_id())
            .map(|_| ()),
        Err(HomeRefusal::NotCheckedOut)
    );
    assert_eq!(
        start(Some(&home(Vec::from([checkout()]), Vec::new())), id_id())
            .map(|_| ()),
        Err(HomeRefusal::UnknownIssue)
    );
    assert_eq!(start(None, id_id()).map(|_| ()), Err(HomeRefusal::NoHome));
}

#[test]
fn a_brief_names_the_issue_s_stage_and_the_skills_that_serve_it() {
    let brief_at = |stage: Option<&str>| {
        let at = Issue {
            stage: stage.map(text),
            ..issue()
        };
        start(
            Some(&home(Vec::from([checkout()]), Vec::from([at]))),
            id_id(),
        )
        .map_or_else(|_| String::new(), |(_, started)| started.brief)
    };
    let build = brief_at(Some("build"));
    assert!(
        build.contains(
            "It is at the stage build: follow the build skill, and diagnose where it serves."
        ),
        "{build}"
    );
    let spec = brief_at(Some("spec"));
    assert!(
        spec.contains("It is at the stage spec: follow the spec skill."),
        "{spec}"
    );
    let intake = brief_at(Some("intake"));
    assert!(intake.contains("It is at the stage intake."), "{intake}");
    assert!(!brief_at(None).contains("stage"));
}

fn moved(
    mut update: SessionUpdate,
    status: &str,
    blocker: Option<&str>,
) -> Option<String> {
    let blocking = IssueId {
        authority: String::from("local"),
        key: String::from("00000002"),
    };
    let mut files = Vec::from([filed(
        &id_id(),
        status,
        &format!("blocked_by = [\"{blocking}\"]\n"),
    )]);
    files.extend(blocker.map(|other| filed(&blocking, other, "")));
    match update.next(&snapshot_of(&files), &id_id()) {
        Some(IssueChange::Move {
            part: Part::Status,
            to,
            ..
        }) => Some(to),
        _ => None,
    }
}

#[test]
fn a_session_moves_its_issue_to_the_status_the_flow_gives() {
    let finished = |landed| SessionUpdate::Finished {
        summary: None,
        landed,
    };
    assert_eq!(
        [
            moved(SessionUpdate::Started, "ready", None),
            moved(SessionUpdate::Started, "ready", Some("review")),
            moved(SessionUpdate::Started, "ready", Some("active")),
            moved(SessionUpdate::Started, "active", None),
            moved(finished(2), "active", None),
            moved(finished(0), "active", None),
            moved(finished(2), "done", None),
        ],
        [
            Some(text("active")),
            Some(text("active")),
            None,
            None,
            Some(text("review")),
            None,
            None,
        ]
    );
}

#[test]
fn the_issue_branch_follows_the_remote_and_never_leaves_its_work_out() {
    let facts = |local: Option<&str>, remote: Remote, (behind, ahead)| Branch {
        local: local.map(text),
        remote,
        base: text("base"),
        behind,
        ahead,
    };
    let at = |commit: &str| Remote::At(text(commit));
    let set = |commit: &str, old: Option<&str>| Move::Set {
        commit: text(commit),
        old: old.map(text),
    };
    let away = || Remote::Unreachable(text("offline"));
    assert_eq!(
        [
            facts(None, Remote::Absent, (false, false)),
            facts(None, at("r"), (false, false)),
            facts(Some("l"), Remote::Absent, (false, false)),
            facts(Some("l"), away(), (false, false)),
            facts(Some("r"), at("r"), (true, true)),
            facts(Some("l"), at("r"), (true, false)),
            facts(Some("l"), at("r"), (false, true)),
        ]
        .map(Branch::next),
        [
            set("base", None),
            set("r", None),
            Move::Keep,
            Move::Keep,
            Move::Keep,
            set("r", Some("l")),
            Move::Keep,
        ]
    );
    assert!(matches!(
        facts(None, away(), (false, false)).next(),
        Move::Refuse(reason) if reason.ends_with("offline")
    ));
    assert!(matches!(
        facts(Some("l"), at("r"), (false, false)).next(),
        Move::Refuse(reason) if reason.contains("both moved on")
    ));
}

#[test]
fn an_issue_branch_is_the_homes_or_named_by_its_id() {
    let known = home(Vec::from([checkout()]), Vec::from([issue()]));
    assert_eq!(
        branch(Some(&known), &id_id()),
        String::from("refs/heads/issue/00000001")
    );
    assert_eq!(branch(None, &id_id()), "refs/heads/issue/00000001");
}

fn bare(rest: &str) -> String {
    format!(
        "+++\nid = \"00000001\"\ntitle = \"Sign in\"\nstatus = \"open\"\n+++\n{rest}"
    )
}

#[test]
fn a_new_body_replaces_only_the_body() -> Result<(), String> {
    let new_body = || changing_to(IssueChange::Body(text("New why.")));
    assert_eq!(
        file_of(apply(Some(&written()), &new_body(), Mode::Record))?,
        written().replace("What to build and why.", "New why."),
        "the checklist and the notes stay, a blank line before them"
    );
    assert_eq!(
        file_of(apply(Some(&bare("\nWhy.\n")), &new_body(), Mode::Record))?,
        bare("\nNew why.\n"),
        "a body with nothing after it ends the file"
    );
    let source = format!("{}\n## Design\nKept after the notes.\n", written());
    let original = storage::decode(
        &flow(),
        &File {
            project: project(),
            stem: id_id().reference(),
            text: source.clone(),
        },
    )
    .map_err(|problem| problem.to_string())?
    .0;
    let observed = super::Observed {
        reading: Ok(original.clone()),
        applied: Vec::new(),
    };
    let after = super::apply(
        &snapshot_of(&[]).with(original.clone()),
        Some(&observed),
        &new_body(),
        Mode::Record,
    );
    let Ok(super::Applied { change, .. }) = after else {
        return Err(text("expected a semantic body change"));
    };
    assert_eq!(
        change.observation.editable()?.0.sections,
        [
            Section {
                heading: String::new(),
                text: text("New why.")
            },
            Section {
                heading: text("Design"),
                text: text("Kept after the notes.")
            }
        ]
    );
    assert_eq!(
        change.observation.editable()?.0.checklist,
        original.editable()?.0.checklist
    );
    assert_eq!(
        change.observation.editable()?.0.log,
        original.editable()?.0.log
    );
    let context = command::Context {
        now: UnixSecs(OPENED).millis(),
        by: text("laptop·iPhone"),
    };
    for body in [
        "New why.",
        "New why.\n\n## Checklist\n- [ ] extra\n\n## Notes\n- new note",
        "New why.\n\n```",
    ] {
        let loaded = storage::load(
            &flow(),
            &super::Read {
                project: project(),
                issue: id_id(),
            },
            &source,
        );
        let plan = command::plan(
            &snapshot_of(&[]).with(original.clone()),
            &command::Edit::Change {
                id: id_id(),
                changes: Vec::from([
                    IssueChange::Body(text(body)),
                    IssueChange::Note(text("following note")),
                ]),
            },
            &context,
        )?;
        let file =
            storage::encode(&flow(), &plan.change, loaded.document, None)?;
        let decoded = storage::decode(&flow(), &file)
            .map_err(|problem| problem.to_string())?
            .0;
        assert_eq!(plan.change.observation.body(), decoded.body(), "{body}");
        let logged = &plan.change.observation.editable()?.0.log;
        assert_eq!(logged.len(), original.editable()?.0.log.len() + 1);
        assert_eq!(
            logged.last().map(|entry| entry.text.as_str()),
            Some("following note")
        );
    }
    Ok(())
}

#[test]
fn setting_a_field_to_its_value_changes_nothing_and_to_another_writes_it()
-> Result<(), String> {
    let current = issue();
    for same in [
        retitling(&current.title),
        IssueChange::Body(current.body.clone()),
        listing("labels", &["app", "credentials"]),
        setting("priority", "high"),
        moving_to(&current.status),
    ] {
        assert_eq!(
            apply(Some(&written()), &changing_to(same.clone()), Mode::Record),
            Applied::Unchanged,
            "{same:?}"
        );
    }
    let logged = |line: &str| format!("{}- {SIGNED} note: {line}\n", written());
    for (change, from, to, after) in [
        (
            retitling("Sign in"),
            "title = \"Sign in to a subscription from the app\"",
            "title = \"Sign in\"",
            logged("set title=Sign in."),
        ),
        (
            setting("priority", "low"),
            "priority = \"high\"",
            "priority = \"low\"",
            logged("set priority=low."),
        ),
        (
            IssueChange::Body(text("Why.")),
            "What to build and why.",
            "Why.",
            written(),
        ),
    ] {
        assert_eq!(
            file_of(apply(
                Some(&written()),
                &changing_to(change),
                Mode::Record
            ))?,
            after.replace(from, to)
        );
    }
    Ok(())
}

#[test]
fn a_value_the_flow_doesnt_declare_is_refused() {
    assert_eq!(
        apply(
            Some(&written()),
            &changing_to(setting("priority", "someday")),
            Mode::Record
        ),
        Applied::Refused(String::from(
            "refused conductor/00000001: priority: \"someday\" is not one of low, normal, high, urgent"
        ))
    );
}

#[test]
fn a_replay_made_in_the_second_of_the_remotes_last_change_still_applies()
-> Result<(), String> {
    let done = Edit {
        data: EditData {
            was: Vec::from([Some(Vec::from([text("active")]))]),
            ..changing_to(moving_to("done")).data
        },
        ..changing_to(moving_to("done"))
    };
    let reviewed = written().replace("\"active\"", "\"review\"");
    let replayed = file_of(apply(
        Some(&reviewed),
        &done,
        Mode::Replay {
            changed: UnixSecs(OPENED),
        },
    ))?;
    assert_eq!(parse(&replayed).map(|read| read.status), Ok(text("done")));
    Ok(())
}

#[test]
fn an_item_added_again_is_kept_twice_and_a_new_note_replays_in()
-> Result<(), String> {
    let again = file_of(apply(
        Some(&written()),
        &changing_to(adding("the protocol")),
        Mode::Record,
    ))?;
    assert_eq!(parse(&again).map(|read| read.checklist.len()), Ok(3));
    let noted = file_of(apply(
        Some(&written()),
        &changing_to(IssueChange::Note(text("later"))),
        Mode::Replay {
            changed: UnixSecs(OPENED + 9),
        },
    ))?;
    assert_eq!(
        parse(&noted).map(|read| read.log.len()),
        Ok(2),
        "a note the file doesn't have yet is added on a replay"
    );
    Ok(())
}

#[test]
fn the_last_item_before_the_next_heading_is_found() -> Result<(), String> {
    let listed =
        bare("\nWhy.\n\n## Checklist\n- [ ] a\n- [ ] b\n## Notes\n- n\n");
    assert_eq!(
        file_of(apply(
            Some(&listed),
            &changing_to(IssueChange::Checklist(ChecklistChange::Remove(at(
                1, None
            )))),
            Mode::Record,
        ))?,
        listed.replace("- [ ] b\n", "")
    );
    Ok(())
}

#[test]
fn a_section_of_its_own_is_neither_checklist_nor_notes() {
    let file = bare(
        "\nWhy.\n\n## Checklist\n- [ ] one\n\n## Design\n- [ ] not an item\n\n\
         ## Notes\n- a note\n\n## Later\n- not a note\n",
    );
    assert_eq!(
        parse(&file).map(|read| (read.checklist.len(), notes(&read))),
        Ok((1, Vec::from([text("a note")])))
    );
}

#[test]
fn an_edit_preserves_quoted_keys_comments_and_fenced_examples()
-> Result<(), String> {
    let original = bare("\n```` example\n## Checklist\n```\n## Notes\n````\n\n## Checklist\n- [ ] Real\n\n~~~\n## Notes\n- [ ] Example\n~~~\n\n## Notes\n- Real note\n")
        .replace(
            "title = \"Sign in\"",
            "'title' = \"Sign in\" # Keep title comment\n# Keep labels comment\n\"labels\" = [\n    \"one,two\", # Keep inline note\n    # Keep standalone note\n    \"three\",\n] # Keep array comment",
        );
    let labelled = file_of(apply(
        Some(&original),
        &changing_to(listing("labels", &["new"])),
        Mode::Record,
    ))?;
    assert_eq!(
        labelled,
        format!("{original}- {SIGNED} note: set labels=new.\n").replace(
            "[\n    \"one,two\", # Keep inline note\n    # Keep standalone note\n    \"three\",\n]",
            "[\n    # Keep inline note\n    # Keep standalone note\n    \"new\"\n]",
        ),
    );
    let titled = file_of(apply(
        Some(&labelled),
        &changing_to(retitling("New title")),
        Mode::Record,
    ))?;
    let reread = parse(&titled)?;
    assert_eq!(reread.title, "New title");
    assert_eq!(reread.texts("labels"), [text("new")]);
    assert_eq!(notes(&reread).first(), Some(&text("Real note")));
    assert_eq!(reread.checklist.len(), 1);
    assert!(reread.body.contains("## Checklist"));
    assert!(reread.body.contains("## Notes"));
    assert!(titled.contains("'title' = \"New title\" # Keep title comment"));
    Ok(())
}

#[test]
fn a_newer_label_list_with_a_comma_is_a_distinct_snapshot() {
    let original = written()
        .replace("[\"app\", \"credentials\"]", "[\"app, credentials\"]");
    let change = changing(listing("labels", &["queued"]));
    let original_snapshot = was(Some(&original), &change);
    assert_ne!(original_snapshot, was(Some(&written()), &change));
    let queued = Edit {
        data: EditData {
            was: original_snapshot,
            edit: change,
            ..changing_to(retitling("unused")).data
        },
        ..changing_to(retitling("unused"))
    };
    assert_eq!(
        file_of(apply(
            Some(&written()),
            &queued,
            Mode::Replay {
                changed: UnixSecs(OPENED + 1)
            }
        ))
        .and_then(|kept| parse(&kept))
        .map(|read| read.texts("labels").to_vec()),
        Ok(Vec::from([text("app"), text("credentials")])),
        "the newer labels stay"
    );
}

#[test]
fn checking_and_removing_the_second_identical_row_keeps_the_first()
-> Result<(), String> {
    let original = bare("\n## Checklist\n- [ ] repeat\n- [ ] repeat\n");
    let seen = parse(&original)?.checklist;
    let toggle = changing_to(IssueChange::Checklist(ChecklistChange::Check {
        item: at(1, Some(seen)),
        done: true,
    }));
    let checked = file_of(apply(Some(&original), &toggle, Mode::Record))?;
    assert!(checked.ends_with("- [ ] repeat\n- [x] repeat\n"));
    let remove = changing_to(IssueChange::Checklist(ChecklistChange::Remove(
        at(1, Some(parse(&checked)?.checklist)),
    )));
    let removed = file_of(apply(Some(&checked), &remove, Mode::Record))?;
    assert!(removed.ends_with("## Checklist\n- [ ] repeat\n"));
    assert_eq!(
        apply(Some(&removed), &toggle, Mode::Record),
        Applied::Refused(text("the checklist changed; select the item again")),
    );
    Ok(())
}

#[test]
fn what_a_client_writes_is_signed_by_the_host_and_the_client_as_one_word() {
    assert_eq!(super::signer("laptop", "iPhone"), "laptop·iPhone");
    assert_eq!(
        super::signer("big laptop", "my phone"),
        "big-laptop·my-phone"
    );
}

#[test]
fn an_issues_branch_is_the_one_the_home_lists_for_it() {
    let other = Issue {
        created: None,
        id: IssueId {
            authority: String::from("local"),
            key: String::from("00000009"),
        },
        branch: text("issue/other"),
        ..issue()
    };
    let mine = Issue {
        branch: text("issue/mine"),
        ..issue()
    };
    let listed = home(Vec::new(), Vec::from([other, mine]));
    assert_eq!(branch(Some(&listed), &id_id()), "refs/heads/issue/mine");
}

fn unexpected_effect() -> HomeRefusal {
    HomeRefusal::IssueNotSaved {
        reason: text("unexpected record effect"),
    }
}

fn committed(
    commit: &super::Commit,
    previous: Option<&super::Observed>,
) -> super::Observed {
    let mut applied =
        previous.map_or_else(Vec::new, |observed| observed.applied.clone());
    applied.push(commit.marker);
    super::Observed {
        reading: Ok(commit.change.observation.clone()),
        applied,
    }
}

#[test]
fn an_opening_is_rejected_until_its_edit_can_be_recovered()
-> Result<(), HomeRefusal> {
    use super::{Delivery, NoCommit, RecordEffect};
    let home = snapshot_of(&[]);
    let context = context(&home, EditId(1));
    let request = context.open(id_id(), wanted(), "laptop")?;
    let target = request.read();
    assert_eq!((target.project, target.issue.clone()), (project(), id_id()));
    let mut recorded = request.decide(None, &home, &PendingEdits::default())?;
    let Some(RecordEffect::Persist(edit)) = recorded.next(Ok(()))? else {
        return Err(unexpected_effect());
    };
    let mut failed = recorded.clone();
    let reason = text("queue write failed");
    assert_eq!(
        failed.next(Err(reason.clone())),
        Err(HomeRefusal::IssueNotSaved { reason })
    );
    assert_eq!(failed.next(Ok(()))?, None);
    let Some(RecordEffect::Commit(planned)) = recorded.next(Ok(()))? else {
        return Err(unexpected_effect());
    };
    let commit = planned.clone();
    let mut kept = recorded.clone();
    assert_eq!(
        kept.next(Err(text("commit failed")))?,
        Some(RecordEffect::Scan)
    );
    assert_eq!(kept.next(Ok(()))?, None);
    assert_eq!(edit.replay(&home, Ok(None), None), Ok(commit.clone()));
    assert_eq!(commit.target, target);
    assert_eq!(
        commit
            .change
            .observation
            .issue()
            .ok_or(HomeRefusal::UnknownIssue)?
            .kind,
        "task"
    );
    assert!(commit.message.starts_with("Open issue "));
    assert_eq!(
        (commit.author, commit.email),
        ("Conductor", "conductor@localhost")
    );
    assert_eq!(
        recorded.next(Ok(()))?,
        Some(RecordEffect::Deliver {
            delivery: Delivery::Push { retry: true },
            id: Some(context.edit)
        })
    );
    assert_eq!(
        recorded.next(Err(text("offline")))?,
        Some(RecordEffect::Scan)
    );
    assert_eq!(recorded.next(Ok(()))?, None);
    let observed = committed(&commit, None);
    assert_eq!(
        edit.replay(&home, Ok(Some(&observed)), Some(UnixSecs(OPENED))),
        Err(NoCommit::Unchanged)
    );
    let existing = context.open(id_id(), wanted(), "laptop")?;
    assert_eq!(
        existing
            .decide(Some(&observed), &home, &PendingEdits::default())?
            .next(Ok(()))?,
        None
    );
    Ok(())
}

fn recorded(
    change: IssueChange,
    edit: EditId,
    current: &super::Observed,
) -> Result<(Edit, super::Commit), HomeRefusal> {
    use super::RecordEffect;
    let home = snapshot_of(&[issue_file()]);
    let request = context(&home, edit).edit(id_id(), change, "laptop")?;
    let mut recorded =
        request.decide(Some(current), &home, &PendingEdits::default())?;
    let Some(RecordEffect::Persist(edit)) = recorded.next(Ok(()))? else {
        return Err(unexpected_effect());
    };
    let Some(RecordEffect::Commit(planned)) = recorded.next(Ok(()))? else {
        return Err(unexpected_effect());
    };
    let commit = planned.clone();
    Ok((edit, commit))
}

fn replay(
    edit: &Edit,
    current: Option<&super::Observed>,
    changed: Option<u64>,
) -> Result<super::Commit, super::NoCommit> {
    among(&[], |home| {
        edit.replay(home, Ok(current), changed.map(UnixSecs))
    })
}

#[test]
fn issue_changes_decide_each_commit_and_replay_against_the_observed_issue()
-> Result<(), HomeRefusal> {
    use super::NoCommit;
    let changes = [
        (retitling("New title"), "Retitle issue"),
        (IssueChange::Body(text("New body")), "Describe issue"),
        (listing("labels", &["new"]), "Set the labels of issue"),
        (setting("priority", "low"), "Set the priority of issue"),
        (moving_to("review"), "Move issue"),
        (adding("Test the change"), "Add an item"),
        (
            IssueChange::Checklist(ChecklistChange::Check {
                item: at(2, None),
                done: true,
            }),
            "Check off",
        ),
        (
            IssueChange::Checklist(ChecklistChange::Remove(at(2, None))),
            "Remove an item",
        ),
        (IssueChange::Note(text("Summary")), "Note on issue"),
    ];
    let mut current = observed(&written());
    for (index, (change, message)) in changes.into_iter().enumerate() {
        let (edit, commit) = recorded(change, EditId(index as u128), &current)?;
        assert!(commit.message.starts_with(message), "{}", commit.message);
        assert_eq!(
            replay(&edit, Some(&current), Some(OPENED)),
            Ok(commit.clone())
        );
        current = committed(&commit, Some(&current));
        assert_eq!(
            replay(&edit, Some(&current), Some(OPENED)),
            Err(NoCommit::Unchanged)
        );
        assert_eq!(replay(&edit, None, None), Err(NoCommit::Dropped));
        assert!(matches!(
            replay(&edit, Some(&observed("malformed")), Some(OPENED)),
            Err(NoCommit::Waiting(_))
        ));
    }
    assert!(
        current
            .reading
            .as_ref()
            .ok()
            .and_then(super::Observation::issue)
            .is_some_and(|issue| {
                issue.log.iter().any(|entry| entry.text == "Summary")
            })
    );
    let (edit, _) =
        recorded(retitling("Older edit"), EditId(100), &observed(&written()))?;
    let Ok(resolved) = replay(&edit, Some(&current), Some(OPENED + 1)) else {
        return Err(unexpected_effect());
    };
    assert_eq!(
        resolved
            .change
            .observation
            .issue()
            .ok_or(HomeRefusal::UnknownIssue)?
            .title,
        "New title"
    );
    assert_eq!(resolved.change.mutations, Vec::new());
    let mut reverted = committed(&resolved, Some(&current));
    reverted.reading = reverted.reading.and_then(|observation| {
        let (original, body) = observation.editable()?;
        let mut issue = original.clone();
        issue.title = self::issue().title;
        Ok(crate::issue::storage::observation(issue, body.clone()))
    });
    assert_eq!(
        replay(&edit, Some(&reverted), Some(OPENED + 2)),
        Err(NoCommit::Unchanged)
    );
    assert!(replay(&edit, Some(&current), Some(OPENED)).is_ok());
    Ok(())
}

#[test]
fn an_edit_retried_with_its_edit_id_under_a_new_command_is_applied_once()
-> Result<(), HomeRefusal> {
    use super::NoCommit;
    let retried = EditId(7);
    let change = || IssueChange::Note(text("Retried note"));
    let (kept, first) = recorded(change(), retried, &observed(&written()))?;
    let pending = PendingEdits::loaded(Vec::from([kept.clone()]));
    assert_eq!(pending.issue_for(retried), Some(id_id()));
    assert_eq!(pending.issue_for(EditId(8)), None);
    let home = snapshot_of(&[issue_file()]);
    let again = context(&home, retried).edit(id_id(), change(), "laptop")?;
    let current = committed(&first, None);
    assert_eq!(
        again
            .decide(Some(&current), &home, &PendingEdits::default())?
            .next(Ok(()))?,
        None
    );
    assert_eq!(
        replay(&kept, Some(&current), Some(OPENED)),
        Err(NoCommit::Unchanged)
    );
    assert_eq!(
        first
            .change
            .observation
            .issue()
            .ok_or(HomeRefusal::UnknownIssue)?
            .log
            .iter()
            .filter(|entry| entry.text == "Retried note")
            .count(),
        1
    );
    Ok(())
}

#[test]
fn replay_preserves_separate_notes_with_identical_text()
-> Result<(), HomeRefusal> {
    use super::NoCommit;
    let note = || IssueChange::Note(text("Same note"));
    let original = observed(&written());
    let (first, _) = recorded(note(), EditId(1), &original)?;
    let Ok(first_commit) = replay(&first, Some(&original), Some(OPENED)) else {
        return Err(unexpected_effect());
    };
    let home = snapshot_of(&[issue_file()]);
    let again = context(&home, EditId(1)).edit(id_id(), note(), "laptop")?;
    let first_observed = committed(&first_commit, Some(&original));
    assert_eq!(
        again
            .decide(Some(&first_observed), &home, &PendingEdits::default())?
            .next(Ok(()))?,
        None
    );
    let (second, _) = recorded(note(), EditId(2), &first_observed)?;
    let Ok(second_commit) =
        replay(&second, Some(&first_observed), Some(OPENED))
    else {
        return Err(unexpected_effect());
    };
    assert_eq!(
        second_commit
            .change
            .observation
            .issue()
            .ok_or(HomeRefusal::UnknownIssue)?
            .log
            .iter()
            .filter(|entry| entry.text == "Same note")
            .count(),
        2
    );
    let second_observed = committed(&second_commit, Some(&first_observed));
    assert_eq!(
        replay(&first, Some(&second_observed), Some(OPENED)),
        Err(NoCommit::Unchanged)
    );
    assert_eq!(
        replay(&second, Some(&second_observed), Some(OPENED)),
        Err(NoCommit::Unchanged)
    );
    Ok(())
}

#[test]
fn a_session_updates_its_issue_using_the_listing_after_the_note() {
    let open = snapshot_of(&[filed(&id_id(), "open", "")]);
    let mut start = SessionUpdate::Started;
    assert_eq!(
        start.next(&open, &id_id()),
        Some(IssueChange::Move {
            part: Part::Status,
            to: text("active"),
            reason: text("a session started on it"),
            signed_off: false,
        })
    );
    assert_eq!(start.next(&open, &id_id()), None);

    let mut finish = SessionUpdate::Finished {
        summary: Some(text("Summary")),
        landed: 1,
    };
    assert_eq!(
        finish.next(&open, &id_id()),
        Some(IssueChange::Note(text("Summary")))
    );
    let closed = snapshot_of(&[filed(&id_id(), "done", "")]);
    assert_eq!(finish.next(&closed, &id_id()), None);

    let mut finish = SessionUpdate::Finished {
        summary: None,
        landed: 1,
    };
    assert_eq!(
        finish.next(&open, &id_id()),
        Some(IssueChange::Move {
            part: Part::Status,
            to: text("review"),
            reason: text("a session on it landed work"),
            signed_off: false,
        })
    );
    assert_eq!(finish.next(&open, &id_id()), None);
}

#[test]
fn a_competing_done_commit_supersedes_a_session_finishing_its_issue()
-> Result<(), String> {
    let listing = snapshot_of(&[issue_file()]);
    let mut finish = SessionUpdate::Finished {
        summary: None,
        landed: 1,
    };
    let change = finish
        .next(&listing, &id_id())
        .ok_or_else(|| text("expected a move"))?;
    let finishing = Edit {
        data: EditData {
            was: was(Some(&written()), &changing(change.clone())),
            ..changing_to(change.clone()).data
        },
        ..changing_to(change)
    };
    let done = written().replace("status = \"active\"", "status = \"done\"");
    assert_eq!(
        file_of(apply(
            Some(&done),
            &finishing,
            Mode::Replay {
                changed: UnixSecs(OPENED + 1)
            }
        ))
        .and_then(|kept| parse(&kept))
        .map(|read| read.status),
        Ok(text("done")),
    );
    Ok(())
}

mod as_the_command_line {
    use crate::issue::{EditId, IssueChange};
    use crate::{HomeRefusal, IssueId, ProjectId, UnixSecs};

    use crate::issue::Datum;

    use super::super::{Context, PendingEdits};
    use crate::flow::command::{self, command, run};
    use crate::issue::storage::Inputs;
    use crate::issue::storage::{File, fresh};

    const FLOW: &str = include_str!("../flow/test-flow.toml");
    const FOLDER: &str = "7w85je";
    const SPEC: &str = "0tm7zt01";
    const TASK: &str = "0tm7zx0d";
    const NOW: UnixSecs = UnixSecs(1_795_348_800);

    fn issue(id: &str, front: &str, body: &str) -> File {
        File {
            project: ProjectId::parse(FOLDER).unwrap_or(ProjectId::new(0)),
            stem: String::from(id),
            text: format!("+++\nid = \"{id}\"\n{front}+++\n\n{body}"),
        }
    }

    fn inputs() -> Inputs {
        Inputs {
            unavailable: Vec::new(),
            observed_at: None,
            flow: Ok(String::from(FLOW)),
            projects: Vec::from([(
                String::from(FOLDER),
                Ok(String::from(
                    "name = \"home\"\ntitle = \"Home\"\nroot = \"e22b7f7b2976650400965d4147ba771675f96076\"\n",
                )),
            )]),
            issues: Vec::from([
                issue(
                    SPEC,
                    "title = \"The flow\"\nkind = \"spec\"\nstage = \"spec\"\nstatus = \"open\"\n",
                    "## Problem\n\n## Intended design\n\n## Acceptance\n\n## Notes\n",
                ),
                issue(
                    TASK,
                    &format!(
                        "title = \"Write it\"\nkind = \"task\"\nstatus = \"open\"\nparent = \"{SPEC}\"\n"
                    ),
                    "## What\n\n## Notes\n",
                ),
            ]),
            unread: Vec::new(),
        }
    }

    fn by_command_line(said: &str) -> Result<Vec<String>, String> {
        let words: Vec<String> = said.split(' ').map(String::from).collect();
        let home = crate::issue::storage::snapshot(inputs());
        let outcome = run(
            command(&words, &home, fresh(&home, NOW, &[7; crate::DRAWS])),
            None,
            &home,
            &command::Context {
                now: NOW.millis(),
                by: String::from("laptop·claude"),
            },
        );
        let subject = format!("home/{TASK}");
        if outcome.failed {
            return Err(outcome
                .output
                .trim_end()
                .trim_start_matches("home: ")
                .to_owned());
        }
        Ok(outcome
            .output
            .lines()
            .filter_map(|line| {
                line.strip_prefix(&format!("warn   {subject}  "))
                    .map(String::from)
            })
            .collect())
    }

    fn by_app(change: IssueChange) -> Result<Vec<String>, HomeRefusal> {
        let home = crate::issue::storage::snapshot(inputs());
        let task =
            IssueId::from_reference(TASK).ok_or(HomeRefusal::UnknownIssue)?;
        let current = home.observation(&task).cloned().map(|issue| {
            crate::issue::Observed {
                reading: Ok(issue),
                applied: Vec::new(),
            }
        });
        let record = Context {
            remote: Some("origin"),
            home: &home,
            host: "laptop",
            at: NOW,
            edit: EditId(1),
        }
        .edit(task, change, "iPhone")?;
        let recorded =
            record.decide(current.as_ref(), &home, &PendingEdits::default())?;
        Ok(recorded.warnings().to_vec())
    }

    fn set(field: &str, value: Datum) -> IssueChange {
        IssueChange::Set {
            field: String::from(field),
            value,
        }
    }

    fn word(value: &str) -> Datum {
        Datum::Text(String::from(value))
    }

    fn words(values: &[&str]) -> Datum {
        Datum::Texts(values.iter().map(|value| String::from(*value)).collect())
    }

    #[test]
    fn an_edit_the_command_line_refuses_is_refused_with_its_reason() {
        let dangling = "0tm7zx0a";
        for (said, change) in [
            (
                format!("issue set {TASK} priority=someday"),
                set("priority", word("someday")),
            ),
            (
                format!("issue set {TASK} blocked_by={dangling}"),
                set("blocked_by", words(&[dangling])),
            ),
            (
                format!("issue set {TASK} blocked_by={TASK}"),
                set("blocked_by", words(&[TASK])),
            ),
        ] {
            let refused = by_command_line(&said);
            assert!(
                refused
                    .as_ref()
                    .is_err_and(|reason| reason.starts_with("refused home/")),
                "{refused:?}"
            );
            assert_eq!(
                by_app(change),
                Err(HomeRefusal::IssueRefused {
                    reason: refused.err().unwrap_or_default()
                }),
                "{said}"
            );
        }
    }

    #[test]
    fn an_edit_the_command_line_warns_of_is_written_with_its_warning() {
        let warned = by_command_line(&format!("issue note {TASK} checked"));
        assert_eq!(
            warned,
            Ok(Vec::from([String::from(
                "lacks the section \"Acceptance\""
            )]))
        );
        assert_eq!(
            by_app(IssueChange::Note(String::from("checked"))),
            warned.map_err(|reason| HomeRefusal::IssueRefused { reason })
        );
    }

    #[test]
    fn the_app_and_the_command_line_plan_the_same_semantic_change() {
        let home = crate::issue::storage::snapshot(inputs());
        let said: Vec<String> = format!("issue set {TASK} priority=high")
            .split(' ')
            .map(String::from)
            .collect();
        let by = command::Context {
            now: NOW.millis(),
            by: String::from("laptop·iPhone"),
        };
        let typed = run(
            command(&said, &home, fresh(&home, NOW, &[7; crate::DRAWS])),
            None,
            &home,
            &by,
        );
        let made = IssueId::from_reference(TASK)
            .ok_or_else(String::new)
            .and_then(|id| {
                command::plan(
                    &home,
                    &command::Edit::Change {
                        id,
                        changes: Vec::from([set("priority", word("high"))]),
                    },
                    &by,
                )
            });
        assert_eq!(
            typed.changes.first().cloned(),
            made.map(|planned| planned.change).ok()
        );
    }
}

#[test]
fn the_issue_branch_checks_origin_and_distinguishes_absence_from_failure() {
    use super::RemoteProbe::{self, Fetch, Observed, Reachability, Tracking};

    let branch = RemoteProbe::for_branch("refs/heads/issue/task");
    let absent = Observed(Remote::Absent);
    assert_eq!(branch.clone().after(Err(text("no origin"))), absent);
    let fetch = branch.after(Ok(text("origin")));
    let tracking = text("refs/remotes/origin/issue/task");
    assert_eq!(
        fetch,
        Fetch {
            branch: text("refs/heads/issue/task"),
            refspec: format!("+refs/heads/issue/task:{tracking}"),
            tracking: tracking.clone()
        }
    );
    let query = fetch.clone().after(Err(text("fetch failed")));
    assert_eq!(
        query,
        Reachability {
            branch: text("refs/heads/issue/task"),
            failure: text("fetch failed")
        }
    );
    assert_eq!(query.clone().after(Ok(String::new())), absent);
    assert_eq!(
        query.clone().after(Ok(text("abc\trefs/heads/issue/task"))),
        Observed(Remote::PresentUnfetched(text("fetch failed")))
    );
    assert_eq!(
        query.after(Err(text("offline"))),
        Observed(Remote::Unreachable(text("offline")))
    );
    let read = fetch.after(Ok(String::new()));
    assert_eq!(read, Tracking(tracking));
    assert_eq!(
        read.clone().after(Err(text("unknown ref"))),
        Observed(Remote::PresentUnfetched(text("unknown ref")))
    );
    assert_eq!(
        read.after(Ok(text("abc"))),
        Observed(Remote::At(text("abc")))
    );
}

#[test]
fn every_line_the_app_logs_or_briefs_is_the_one_log_line_of_its_entry()
-> Result<(), String> {
    for change in [
        IssueChange::Note(text("first\nsecond")),
        moving_to("review"),
        setting("priority", "low"),
    ] {
        let edited = file_of(apply(
            Some(&written()),
            &changing_to(change.clone()),
            Mode::Record,
        ))?;
        let last = parse(&edited)?
            .log
            .last()
            .map_or_else(String::new, log_line);
        assert!(
            last.starts_with(&format!("- {SIGNED} "))
                && edited.ends_with(&format!("\n{last}\n")),
            "{change:?}: {last:?} in {edited}"
        );
    }
    let brief = start(
        Some(&home(Vec::from([checkout()]), Vec::from([issue()]))),
        id_id(),
    )
    .map(|(_, at)| at.brief)
    .map_err(|refusal| format!("{refusal:?}"))?;
    for entry in &issue().log {
        assert!(brief.contains(&log_line(entry)), "{brief}");
    }
    Ok(())
}
