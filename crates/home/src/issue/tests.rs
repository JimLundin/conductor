use std::collections::BTreeMap;

use proto::flow::Flow;
use proto::{
    ChecklistChange, ChecklistItem, Checkout, Cloning, CommandId, Datum, Entry,
    Home, HomeSync, HostPath, Issue, IssueChange, IssueId, Item, LogEvent,
    NewIssue, Origin, Part, ProjectId, Rejection, Section,
};

use super::{
    Applied, Branch, Edit, EditData, Mode, Move, PendingEdits, Remote,
    SessionUpdate, branch, message, read, start,
};
use crate::flow::command;
use crate::flow::file::log_line;
use crate::flow::graph::File;
use crate::flow::load;
use crate::flow::snapshot::{Inputs, Snapshot};
use crate::layout;

/// 00000001.
const ID: IssueId = IssueId::new(0, 1);
/// 2026-11-18T10:12:00Z.
const OPENED: u64 = 1_794_996_720;
/// How the log signs and dates what the edits here write.
const SIGNED: &str = "2026-11-18T10:12:00Z laptop·iPhone";

fn text(value: &str) -> String {
    String::from(value)
}

fn project() -> ProjectId {
    ProjectId::parse("7r5n23").unwrap_or(ProjectId::new(0))
}

/// The home with the project `project()` and the issue files `files`.
fn snapshot_of(files: &[File]) -> Snapshot {
    Snapshot::of(Inputs {
        flow: Ok(text(include_str!("../flow/test-flow.toml"))),
        projects: Vec::from([(
            project().to_string(),
            Ok(text(
                "name = \"conductor\"\ntitle = \"Conductor\"\n\
                 root = \"e22b7f7b2976650400965d4147ba771675f96076\"\n",
            )),
        )]),
        issues: files.to_vec(),
        unread: Vec::new(),
    })
}

/// The issue's file as `id`, in `status`, with `more` front matter.
fn filed(id: IssueId, status: &str, more: &str) -> File {
    File {
        project: project(),
        stem: id.to_string(),
        text: written().replace(&ID.to_string(), &id.to_string()).replace(
            "status = \"active\"\n",
            &format!("status = \"{status}\"\n{more}"),
        ),
    }
}

/// The issue's file, holding [`written`].
fn issue_file() -> File {
    File {
        project: project(),
        stem: ID.to_string(),
        text: written(),
    }
}

/// The flow Conductor's own home declares.
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

/// What `within` makes of the home whose issue files are `files`.
fn among<T>(files: &[File], within: impl FnOnce(&Snapshot) -> T) -> T {
    within(&snapshot_of(files))
}

/// The issue `text` holds, as the flow reads it.
fn parse(text: &str) -> Result<Issue, String> {
    read(&flow(), project(), ID, text).map(|(issue, _)| issue)
}

fn apply(current: Option<&str>, edit: &Edit, mode: Mode) -> Applied {
    among(&[], |home| super::apply(home, current, edit, mode))
}

fn was(
    current: Option<&str>,
    change: command::Edit,
) -> Vec<Option<Vec<String>>> {
    super::was(
        Some(&flow()),
        current,
        &EditData {
            edit: change,
            ..edit(changing(IssueChange::Note(text("unused")))).data
        },
    )
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
        id: ID,
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
            (text("branch"), Datum::Text(format!("issue/{ID}"))),
            (text("created"), Datum::Text(text("2026-11-18T10:12:00Z"))),
        ]),
        links: BTreeMap::new(),
        body: text("What to build and why."),
        sections: Vec::from([Section {
            heading: String::new(),
            text: text("What to build and why."),
        }]),
        branch: format!("issue/{ID}"),
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
        log: Vec::from([note(
            "2026-11-18 laptop · codex · api: built the protocol; browser scenario\nleft; landed on the issue branch.",
        )]),
    }
}

fn written() -> String {
    format!(
        "+++\n\
         id = \"{ID}\"\n\
         title = \"Sign in to a subscription from the app\"\n\
         status = \"active\"\n\
         priority = \"high\"\n\
         labels = [\"app\", \"credentials\"]\n\
         branch = \"issue/{ID}\"\n\
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
         left; landed on the issue branch.\n"
    )
}

/// The file as a person left it: a comment on the status, a key and a
/// table Conductor doesn't read, a section of their own, Windows line
/// ends.
fn hand_edited() -> String {
    format!(
        "+++\n\
         id = \"{ID}\"\n\
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
         Kept as written.\n"
    )
    .replace('\n', "\r\n")
}

/// A command id of its own for each change, so no edit here finds
/// another's mark.
fn command_of(change: &command::Edit) -> CommandId {
    CommandId(format!("{change:?}").bytes().fold(1_000, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u128::from(byte))
    }))
}

fn edit(change: command::Edit) -> Edit {
    Edit {
        command: command_of(&change),
        data: EditData {
            project: project(),
            at: OPENED,
            by: text("laptop·iPhone"),
            edit: change,
            was: Vec::new(),
        },
    }
}

const fn creating(new: NewIssue) -> command::Edit {
    command::Edit::New { id: ID, issue: new }
}

fn changing(change: IssueChange) -> command::Edit {
    command::Edit::Change {
        id: ID,
        changes: Vec::from([change]),
    }
}

fn changing_to(change: IssueChange) -> Edit {
    edit(changing(change))
}

/// The file an edit writes, without the line that marks it written, or
/// what it did instead.
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

/// The checklist's item at `index`, picked from `seen`.
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
    assert_eq!(ID.to_string(), "00000001");
    assert_eq!(
        layout::issue_file(&project(), &ID.to_string()),
        "projects/7r5n23/issues/00000001.md"
    );
    assert_eq!(
        read(&flow(), project(), IssueId::new(0, 2), &written()).map(|_| ()),
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
             id = \"{ID}\"\n\
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
        apply(Some(&marked), &create, Mode::Replay { changed: 0 }),
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
        command: CommandId(2),
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
                changed: OPENED + 9
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
            was: was(Some(&written()), changing(moving_to("done"))),
            ..changing_to(moving_to("done")).data
        },
        ..changing_to(moving_to("done"))
    };
    assert_eq!(done.data.was, [Some(Vec::from([text("active")]))]);
    let reviewed = written().replace("\"active\"", "\"review\"");
    let after = Mode::Replay {
        changed: OPENED + 60,
    };
    let before = Mode::Replay {
        changed: OPENED - 60,
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

fn context(home: &Snapshot, command: CommandId) -> super::Context<'_> {
    super::Context {
        remote: Some("origin"),
        home,
        host: "stone",
        at: OPENED,
        command,
    }
}

#[test]
fn an_issue_opens_in_a_listed_project_on_a_host_with_a_home()
-> Result<(), Rejection> {
    let home = snapshot_of(&[]);
    let listed = context(&home, CommandId(1));
    let nowhere = super::Context {
        remote: None,
        ..context(&home, CommandId(1))
    };
    assert_eq!(
        nowhere.open(ID, wanted(), "iPhone").map(|_| ()),
        Err(Rejection::NoHome)
    );
    assert_eq!(
        listed
            .open(
                ID,
                NewIssue {
                    project: ProjectId::new(1),
                    ..wanted()
                },
                "iPhone"
            )
            .map(|_| ()),
        Err(Rejection::UnknownProject)
    );
    let untitled = listed.open(
        ID,
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
        Err(Rejection::IssueRefused {
            reason: text("an issue needs a title")
        })
    );
    Ok(())
}

#[test]
fn a_change_finds_its_issues_project_on_a_host_with_a_home() {
    let home = snapshot_of(&[issue_file()]);
    let listed = context(&home, CommandId(1));
    assert_eq!(
        listed
            .edit(ID, IssueChange::Note(text("x")), "iPhone")
            .map(|record| record.project()),
        Ok(project())
    );
    assert_eq!(
        listed
            .edit(IssueId::new(0, 7), IssueChange::Note(text("x")), "iPhone")
            .map(|_| ()),
        Err(Rejection::UnknownIssue)
    );
    let nowhere = super::Context {
        remote: None,
        ..listed
    };
    assert_eq!(
        nowhere
            .edit(ID, IssueChange::Note(text("x")), "iPhone")
            .map(|_| ()),
        Err(Rejection::NoHome)
    );
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
            format!("Open issue {ID}: Sign in"),
            format!("Move issue {ID} to review"),
            format!("Set the priority of issue {ID} to low"),
            format!("Note on issue {ID}"),
        ]
    );
}

fn home(checkouts: Vec<Checkout>, issues: Vec<Issue>) -> Home {
    Home {
        remote: text("file:///home.git"),
        synced: None,
        sync: HomeSync::Synced,
        unpushed: 0,
        flow: Some(Box::new(flow())),
        projects: Vec::from([proto::Project {
            id: project(),
            name: text("conductor"),
            title: text("Conductor"),
            remote: None,
            profiles: Vec::new(),
            provides: Vec::new(),
            checkouts,
            cloning: Cloning::Idle,
            issues,
        }]),
    }
}

fn checkout() -> Checkout {
    Checkout {
        path: HostPath(text("/src/conductor")),
        origin: Origin::Found,
    }
}

#[test]
fn a_session_on_an_issue_starts_in_its_projects_checkout_with_a_brief() {
    let started = start(
        Some(&home(Vec::from([checkout()]), Vec::from([issue()]))),
        ID,
    );
    assert_eq!(
        started
            .as_ref()
            .map(|at| (at.checkout.clone(), at.branch.clone())),
        Ok((
            HostPath(text("/src/conductor")),
            format!("refs/heads/issue/{ID}"),
        ))
    );
    let brief = started.map_or_else(|_| String::new(), |at| at.brief);
    for part in [
        "Sign in to a subscription from the app",
        "Conductor",
        "What to build and why.",
        "- [x] the protocol",
        "- [ ] the browser scenario",
        "built the protocol",
        format!("issue/{ID}").as_str(),
    ] {
        assert!(brief.contains(part), "the brief has {part:?}: {brief}");
    }
    assert_eq!(
        start(Some(&home(Vec::new(), Vec::from([issue()]))), ID).map(|_| ()),
        Err(Rejection::NotCheckedOut)
    );
    assert_eq!(
        start(Some(&home(Vec::from([checkout()]), Vec::new())), ID).map(|_| ()),
        Err(Rejection::UnknownIssue)
    );
    assert_eq!(start(None, ID).map(|_| ()), Err(Rejection::NoHome));
}

/// What a session's update moves the issue to from `status`, when it is
/// blocked by an issue whose status is `blocker`, if the home has one.
fn moved(
    mut update: SessionUpdate,
    status: &str,
    blocker: Option<&str>,
) -> Option<String> {
    let blocking = IssueId::new(0, 2);
    let mut files = Vec::from([filed(
        ID,
        status,
        &format!("blocked_by = [\"{blocking}\"]\n"),
    )]);
    files.extend(blocker.map(|other| filed(blocking, other, "")));
    match update.next(&snapshot_of(&files), ID) {
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
    assert_eq!(branch(Some(&known), ID), format!("refs/heads/issue/{ID}"));
    assert_eq!(branch(None, ID), format!("refs/heads/issue/{ID}"));
}

/// A hand-written issue file with the front matter Conductor needs and
/// `rest` after it.
fn bare(rest: &str) -> String {
    format!(
        "+++\nid = \"{ID}\"\ntitle = \"Sign in\"\nstatus = \"open\"\n+++\n{rest}"
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
        Applied::Refused(format!(
            "refused conductor/{ID}: priority: \"someday\" is not one of low, normal, high, urgent"
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
        Mode::Replay { changed: OPENED },
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
            changed: OPENED + 9,
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
    let original_snapshot = was(Some(&original), change.clone());
    assert_ne!(original_snapshot, was(Some(&written()), change.clone()));
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
                changed: OPENED + 1
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
        id: IssueId::new(0, 9),
        branch: text("issue/other"),
        ..issue()
    };
    let mine = Issue {
        branch: text("issue/mine"),
        ..issue()
    };
    let listed = home(Vec::new(), Vec::from([other, mine]));
    assert_eq!(branch(Some(&listed), ID), "refs/heads/issue/mine");
}

fn unexpected_effect() -> Rejection {
    Rejection::IssueNotSaved {
        reason: text("unexpected record effect"),
    }
}

#[test]
fn an_opening_is_rejected_until_its_edit_can_be_recovered()
-> Result<(), Rejection> {
    use super::{Delivery, RecordEffect, Write};

    let home = snapshot_of(&[]);
    let context = context(&home, CommandId(1));
    let request = context.open(ID, wanted(), "laptop")?;
    let target = request.read();
    assert_eq!(target.path, layout::issue_file(&project(), &ID.to_string()));
    let mut recorded = among(&[], |home| {
        request.decide(None, home, &PendingEdits::default())
    })?;
    let Some(RecordEffect::Persist(edit)) = recorded.next(Ok(()))? else {
        return Err(unexpected_effect());
    };
    let mut failed = recorded.clone();
    let reason = text("queue write failed");
    assert_eq!(
        failed.next(Err(reason.clone())),
        Err(Rejection::IssueNotSaved { reason })
    );
    assert_eq!(failed.next(Ok(()))?, None);

    let Some(RecordEffect::Commit(commit)) = recorded.next(Ok(()))? else {
        return Err(unexpected_effect());
    };
    let mut kept = recorded.clone();
    assert_eq!(
        kept.next(Err(text("commit failed")))?,
        Some(RecordEffect::Scan)
    );
    assert_eq!(kept.next(Ok(()))?, None);
    assert_eq!(
        among(&[], |home| edit.replay(home, Ok(None), None)),
        Write::Commit(commit.clone())
    );
    assert_eq!(commit.target, target);
    assert!(commit.text.contains("kind = \"task\""));
    assert!(commit.message.starts_with("Open issue "));
    assert_eq!(commit.author, "Conductor");
    assert_eq!(commit.email, "conductor@localhost");
    assert_eq!(
        recorded.next(Ok(()))?,
        Some(RecordEffect::Deliver {
            delivery: Delivery::Push { retry: true },
            id: Some(context.command),
        })
    );
    assert_eq!(
        recorded.next(Err(text("offline")))?,
        Some(RecordEffect::Scan)
    );
    assert_eq!(recorded.next(Ok(()))?, None);
    assert_eq!(
        among(&[], |home| edit.replay(
            home,
            Ok(Some(&commit.text)),
            Some(OPENED)
        )),
        Write::Unchanged
    );
    assert_eq!(
        super::Listing::created_from_path(&commit.target.path),
        Some(ID)
    );
    assert!(
        commit.text.contains(
            super::Listing::creation_pattern(context.command)
                .trim_matches(['^', '$'])
        )
    );
    let existing = context.open(ID, wanted(), "laptop")?;
    assert_eq!(
        among(&[], |home| existing.decide(
            Some(&commit.text),
            home,
            &PendingEdits::default()
        ))?
        .next(Ok(()))?,
        None
    );
    Ok(())
}

/// What recording `change` as command `command` on the file `current`
/// keeps, and the commit it makes.
fn recorded(
    change: IssueChange,
    command: CommandId,
    current: &str,
) -> Result<(Edit, super::Commit), Rejection> {
    use super::RecordEffect;

    let home = snapshot_of(&[issue_file()]);
    let request = context(&home, command).edit(ID, change, "laptop")?;
    let mut recorded = among(&[], |home| {
        request.decide(Some(current), home, &PendingEdits::default())
    })?;
    let Some(RecordEffect::Persist(edit)) = recorded.next(Ok(()))? else {
        return Err(unexpected_effect());
    };
    let Some(RecordEffect::Commit(commit)) = recorded.next(Ok(()))? else {
        return Err(unexpected_effect());
    };
    Ok((edit, commit))
}

fn replay(
    edit: &Edit,
    current: Option<&str>,
    changed: Option<u64>,
) -> super::Write {
    among(&[], |home| edit.replay(home, Ok(current), changed))
}

#[test]
fn issue_changes_decide_each_commit_and_replay_against_the_file()
-> Result<(), Rejection> {
    use super::Write;

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
    let mut current = written();
    for (index, (change, message)) in changes.into_iter().enumerate() {
        let (edit, commit) =
            recorded(change, CommandId(index as u128), &current)?;
        assert!(commit.message.starts_with(message), "{}", commit.message);
        assert_eq!(
            replay(&edit, Some(&current), Some(OPENED)),
            Write::Commit(commit.clone())
        );
        assert_eq!(
            replay(&edit, Some(&commit.text), Some(OPENED)),
            Write::Unchanged
        );
        assert_eq!(replay(&edit, None, None), Write::Dropped);
        assert!(matches!(
            replay(&edit, Some("malformed"), Some(OPENED)),
            Write::Waiting(_)
        ));
        current = commit.text;
    }
    assert!(current.contains("stone·laptop note: Summary"));
    let (edit, _) =
        recorded(retitling("Older edit"), CommandId(100), &written())?;
    let Write::Commit(resolved) =
        replay(&edit, Some(&current), Some(OPENED + 1))
    else {
        return Err(unexpected_effect());
    };
    assert!(resolved.text.contains("title = \"New title\""));
    assert!(!resolved.text.contains("Older edit"));
    let reverted = resolved.text.replace("New title", &issue().title);
    assert_eq!(
        replay(&edit, Some(&reverted), Some(OPENED + 2)),
        Write::Unchanged
    );
    assert!(matches!(
        replay(&edit, Some(&current), Some(OPENED)),
        Write::Commit(_)
    ));
    Ok(())
}

#[test]
fn replay_preserves_separate_notes_with_identical_text() -> Result<(), Rejection>
{
    use super::Write;

    let note = || IssueChange::Note(text("Same note"));
    let original = written();
    let (first, _) = recorded(note(), CommandId(1), &original)?;
    let Write::Commit(first_commit) =
        replay(&first, Some(&original), Some(OPENED))
    else {
        return Err(unexpected_effect());
    };
    let home = snapshot_of(&[issue_file()]);
    let again = context(&home, CommandId(1)).edit(ID, note(), "laptop")?;
    assert_eq!(
        among(&[], |home| again.decide(
            Some(&first_commit.text),
            home,
            &PendingEdits::default()
        ))?
        .next(Ok(()))?,
        None
    );
    let (second, _) = recorded(note(), CommandId(2), &first_commit.text)?;
    let Write::Commit(second_commit) =
        replay(&second, Some(&first_commit.text), Some(OPENED))
    else {
        return Err(unexpected_effect());
    };
    assert_eq!(second_commit.text.matches(": Same note").count(), 2);
    assert_eq!(
        replay(&first, Some(&second_commit.text), Some(OPENED)),
        Write::Unchanged
    );
    assert_eq!(
        replay(&second, Some(&second_commit.text), Some(OPENED)),
        Write::Unchanged
    );
    Ok(())
}

#[test]
fn a_session_updates_its_issue_using_the_listing_after_the_note() {
    let open = snapshot_of(&[filed(ID, "open", "")]);
    let mut start = SessionUpdate::Started;
    assert_eq!(
        start.next(&open, ID),
        Some(IssueChange::Move {
            part: Part::Status,
            to: text("active"),
            reason: text("a session started on it"),
        })
    );
    assert_eq!(start.next(&open, ID), None);

    let mut finish = SessionUpdate::Finished {
        summary: Some(text("Summary")),
        landed: 1,
    };
    assert_eq!(
        finish.next(&open, ID),
        Some(IssueChange::Note(text("Summary")))
    );
    let closed = snapshot_of(&[filed(ID, "done", "")]);
    assert_eq!(finish.next(&closed, ID), None);

    let mut finish = SessionUpdate::Finished {
        summary: None,
        landed: 1,
    };
    assert_eq!(
        finish.next(&open, ID),
        Some(IssueChange::Move {
            part: Part::Status,
            to: text("review"),
            reason: text("a session on it landed work"),
        })
    );
    assert_eq!(finish.next(&open, ID), None);
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
        .next(&listing, ID)
        .ok_or_else(|| text("expected a move"))?;
    let finishing = Edit {
        data: EditData {
            was: was(Some(&written()), changing(change.clone())),
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
                changed: OPENED + 1
            }
        ))
        .and_then(|kept| parse(&kept))
        .map(|read| read.status),
        Ok(text("done")),
    );
    Ok(())
}

/// The home the `home` command line's own tests use: a spec, and a task
/// under it that lacks a section.
mod as_the_command_line {
    use proto::{CommandId, IssueChange, IssueId, ProjectId, Rejection};

    use proto::Datum;

    use super::super::{Context, PendingEdits};
    use crate::flow::command::{self, command, fresh, run};
    use crate::flow::graph::File;
    use crate::flow::snapshot::{Inputs, Snapshot};

    const FLOW: &str = include_str!("../flow/test-flow.toml");
    const FOLDER: &str = "7w85je";
    const SPEC: &str = "0tm7zt01";
    const TASK: &str = "0tm7zx0d";
    /// 2026-11-22T12:00:00Z, in seconds.
    const NOW: u64 = 1_795_348_800;

    fn issue(id: &str, front: &str, body: &str) -> File {
        File {
            project: ProjectId::parse(FOLDER).unwrap_or(ProjectId::new(0)),
            stem: String::from(id),
            text: format!("+++\nid = \"{id}\"\n{front}+++\n\n{body}"),
        }
    }

    fn inputs() -> Inputs {
        Inputs {
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

    /// What the command line says to `said`: its refusal or its
    /// warnings, as the app would be told them.
    fn by_command_line(said: &str) -> Result<Vec<String>, String> {
        let words: Vec<String> = said.split(' ').map(String::from).collect();
        let home = Snapshot::of(inputs());
        let outcome = run(
            command(&words, &home, fresh(&home, NOW, &[7; proto::DRAWS])),
            None,
            &home,
            &command::Context {
                now: NOW * 1000,
                by: String::from("laptop·claude"),
                mark: None,
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

    /// What the app is told of `change` to the task.
    fn by_app(change: IssueChange) -> Result<Vec<String>, Rejection> {
        let inputs = inputs();
        let home = Snapshot::of(inputs.clone());
        let task = IssueId::parse(TASK).ok_or(Rejection::UnknownIssue)?;
        let current = inputs
            .issues
            .iter()
            .find(|file| file.stem == TASK)
            .map(|file| file.text.clone());
        let record = Context {
            remote: Some("origin"),
            home: &home,
            host: "laptop",
            at: NOW,
            command: CommandId(1),
        }
        .edit(task, change, "iPhone")?;
        let recorded = record.decide(
            current.as_deref(),
            &home,
            &PendingEdits::default(),
        )?;
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
                Err(Rejection::IssueRefused {
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
            warned.map_err(|reason| Rejection::IssueRefused { reason })
        );
    }

    #[test]
    fn the_app_and_the_command_line_write_the_same_file() {
        let home = Snapshot::of(inputs());
        let said: Vec<String> = format!("issue set {TASK} priority=high")
            .split(' ')
            .map(String::from)
            .collect();
        let by = command::Context {
            now: NOW * 1000,
            by: String::from("laptop·iPhone"),
            mark: None,
        };
        let typed = run(
            command(&said, &home, fresh(&home, NOW, &[7; proto::DRAWS])),
            None,
            &home,
            &by,
        );
        let made =
            IssueId::parse(TASK).ok_or_else(String::new).and_then(|id| {
                command::write(
                    &home,
                    &command::Edit::Change {
                        id,
                        changes: Vec::from([set("priority", word("high"))]),
                    },
                    &by,
                )
            });
        assert_eq!(
            typed.writes.first().map(|write| write.text.clone()),
            made.map(|written| written.write.text).ok()
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
        ID,
    )
    .map(|at| at.brief)
    .map_err(|rejection| format!("{rejection:?}"))?;
    for entry in &issue().log {
        assert!(brief.contains(&log_line(entry)), "{brief}");
    }
    Ok(())
}
