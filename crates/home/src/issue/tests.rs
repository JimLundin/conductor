use std::collections::BTreeMap;

use proto::flow::Flow;
use proto::{
    ChecklistAction, ChecklistItem, Checkout, Cloning, CommandId, Datum, Entry,
    Home, HomeSync, HostPath, Issue, IssueChange, IssueFields, IssueId,
    LogEvent, Origin, ProjectId, Rejection, Section,
};

use super::{
    Applied, Branch, Change, Edit, EditData, Mode, Move, PendingEdits, Remote,
    branch, changing, fields, file, finished, listed, message, opening, read,
    start, started,
};
use crate::flow::load;

/// 01JD0000000000000000000001 as its bits.
const ID: IssueId = IssueId(2_093_793_708_204_169_241_204_922_644_758_003_713);
/// 2026-11-18T10:12:00Z.
const OPENED: u64 = 1_794_996_720;

fn text(value: &str) -> String {
    String::from(value)
}

fn project() -> ProjectId {
    ProjectId(text("01m7r5n23kp17wyfj138e0bcnf"))
}

/// The flow Conductor's own home declares.
fn flow() -> Flow {
    load(include_str!("../flow/flow.toml")).unwrap_or_else(|_| Flow {
        default_kind: String::new(),
        statuses: Vec::new(),
        fields: Vec::new(),
        kinds: Vec::new(),
        relations: Vec::new(),
        stages: Vec::new(),
        rules: Vec::new(),
    })
}

/// The issue `text` holds, as the flow reads it.
fn parse(text: &str) -> Result<Issue, String> {
    read(&flow(), &project(), ID, text)
}

fn apply(current: Option<&str>, edit: &Edit, mode: Mode) -> Applied {
    super::apply(&flow(), current, edit, mode)
}

fn was(current: Option<&str>, change: Change) -> Option<String> {
    super::was(
        &flow(),
        current,
        &EditData {
            change,
            ..edit(Change::Issue(IssueChange::Note(text("unused")))).data
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
    issue.log.iter().map(Entry::line).collect()
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

fn edit(change: Change) -> Edit {
    EditData {
        project: project(),
        issue: ID,
        at: OPENED,
        by: text("laptop · iPhone"),
        change,
        was: None,
    }
    .into()
}

fn changing_to(change: IssueChange) -> Edit {
    edit(Change::Issue(change))
}

/// The file an edit writes, or what it did instead.
fn file_of(applied: Applied) -> Result<String, String> {
    match applied {
        Applied::Write(text) => Ok(text),
        other => Err(format!("expected a file, not {other:?}")),
    }
}

#[test]
fn an_issue_file_reads_as_the_flow_declares_it() {
    assert_eq!(parse(&written()), Ok(issue()));
}

#[test]
fn a_change_rewrites_only_its_own_line_of_a_hand_edited_file()
-> Result<(), String> {
    let unix = hand_edited().replace("\r\n", "\n");
    let moved = file_of(apply(
        Some(&hand_edited()),
        &changing_to(IssueChange::Status(text("review"))),
        Mode::Record,
    ))?;
    assert_eq!(
        moved,
        unix.replace("status = \"open\"   #", "status = \"review\"   #")
    );
    let added = file_of(apply(
        Some(&moved),
        &changing_to(IssueChange::AddItem(text("docs"))),
        Mode::Record,
    ))?;
    let noted = file_of(apply(
        Some(&added),
        &changing_to(IssueChange::Note(text("second"))),
        Mode::Record,
    ))?;
    let labelled = file_of(apply(
        Some(&noted),
        &changing_to(IssueChange::Labels(Vec::from([text("app")]))),
        Mode::Record,
    ))?;
    assert_eq!(
        labelled,
        unix.replace("status = \"open\"   #", "status = \"review\"   #")
            .replace(
                "status = \"review\"   # open, ready, active, review, done, dropped\n",
                "status = \"review\"   # open, ready, active, review, done, dropped\nlabels = [\"app\"]\n"
            )
            .replace("- [X] the protocol\n", "- [X] the protocol\n- [ ] docs\n")
            .replace(
                "- first\n",
                "- first\n- 2026-11-18 laptop · iPhone: second\n"
            ),
        "the comment, the key, the table, the [X] and the section stay"
    );
    let read = parse(&hand_edited()).map(|issue| {
        (
            issue.status.clone(),
            issue.body(),
            issue.checklist.len(),
            notes(&issue),
        )
    });
    assert_eq!(
        read,
        Ok((
            text("open"),
            text("Why.\n\n## Design\n\nKept as written."),
            1,
            Vec::from([text("first")])
        ))
    );
    Ok(())
}

#[test]
fn an_item_is_checked_added_and_removed_by_its_text() -> Result<(), String> {
    let checked = file_of(apply(
        Some(&written()),
        &changing_to(IssueChange::Check {
            item: text("the browser scenario"),
            done: true,
        }),
        Mode::Record,
    ))?;
    let removed = file_of(apply(
        Some(&checked),
        &changing_to(IssueChange::RemoveItem(text("the protocol"))),
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
            &changing_to(IssueChange::RemoveItem(text("gone"))),
            Mode::Record
        ),
        Applied::Unchanged
    );
    Ok(())
}

#[test]
fn an_issue_id_its_file_and_its_status_must_agree() {
    assert_eq!(ID.to_string(), "01jd0000000000000000000001");
    assert_eq!(
        file(&project(), ID),
        "projects/01m7r5n23kp17wyfj138e0bcnf/issues/01jd0000000000000000000001.md"
    );
    assert_eq!(
        read(&flow(), &project(), IssueId(2), &written()).map(|_| ()),
        Err(text("id: is not its file's name, a lower-case ULID"))
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

#[test]
fn a_project_lists_its_issues_by_id_and_says_why_one_is_skipped() {
    let (issues, problems) = listed(
        &flow(),
        Vec::from([
            (project(), text("not-an-issue"), Ok(text("+++\n+++\n"))),
            (project(), ID.to_string(), Ok(written())),
            (
                project(),
                text("unreadable"),
                Err(text("permission denied")),
            ),
        ]),
    );
    assert_eq!(issues, BTreeMap::from([(project(), Vec::from([issue()]))]));
    assert_eq!(problems.len(), 2);
}

fn wanted() -> IssueFields {
    IssueFields {
        title: text("  Sign in  "),
        body: text("Why.\n"),
        labels: Vec::from([text(" app "), text(""), text("app")]),
    }
}

#[test]
fn creating_opens_the_issue_on_its_own_branch_and_again_changes_nothing()
-> Result<(), String> {
    let create = edit(Change::Create(wanted()));
    let opened = file_of(apply(None, &create, Mode::Record))?;
    assert_eq!(
        opened,
        format!(
            "+++\n\
             id = \"{ID}\"\n\
             title = \"Sign in\"\n\
             status = \"open\"\n\
             priority = \"normal\"\n\
             labels = [\"app\"]\n\
             branch = \"issue/{ID}\"\n\
             created = 2026-11-18T10:12:00Z\n\
             +++\n\
             \n\
             Why.\n"
        ),
        "the flow's default kind, its first idle status, the defaults of the fields every kind has"
    );
    assert_eq!(
        parse(&opened),
        Ok(Issue {
            title: text("Sign in"),
            status: text("open"),
            fields: BTreeMap::from([
                (text("priority"), Datum::Text(text("normal"))),
                (text("labels"), Datum::Texts(Vec::from([text("app")]))),
                (text("branch"), Datum::Text(format!("issue/{ID}"))),
                (text("created"), Datum::Text(text("2026-11-18T10:12:00Z"))),
            ]),
            sections: Vec::from([Section {
                heading: String::new(),
                text: text("Why."),
            }]),
            checklist: Vec::new(),
            log: Vec::new(),
            ..issue()
        })
    );
    assert_eq!(
        apply(Some(&opened), &create, Mode::Replay { changed: 0 }),
        Applied::AlreadyRecorded
    );
    Ok(())
}

#[test]
fn a_note_is_signed_kept_twice_when_made_twice_and_replayed_once()
-> Result<(), String> {
    let note = changing_to(IssueChange::Note(text("done")));
    let once = file_of(apply(Some(&written()), &note, Mode::Record))?;
    let twice = file_of(apply(Some(&once), &note, Mode::Record))?;
    let noted = |file: &str| parse(file).map(|read| notes(&read));
    assert_eq!(
        noted(&twice).map(|all| all.into_iter().skip(1).collect::<Vec<_>>()),
        Ok(Vec::from([
            text("2026-11-18 laptop · iPhone: done"),
            text("2026-11-18 laptop · iPhone: done"),
        ]))
    );
    assert_eq!(
        apply(
            Some(&once),
            &note,
            Mode::Replay {
                changed: OPENED + 9
            }
        ),
        Applied::Unchanged,
        "a replay finds the note it made already"
    );
    Ok(())
}

#[test]
fn a_replayed_field_yields_to_a_newer_change_by_someone_else() {
    let done: Edit = EditData {
        was: was(
            Some(&written()),
            Change::Issue(IssueChange::Status(text("done"))),
        ),
        ..changing_to(IssueChange::Status(text("done"))).data
    }
    .into();
    assert_eq!(done.data.was, Some(text("active")));
    let reviewed = written().replace("\"active\"", "\"review\"");
    let after = Mode::Replay {
        changed: OPENED + 60,
    };
    let before = Mode::Replay {
        changed: OPENED - 60,
    };
    assert_eq!(
        apply(Some(&reviewed), &done, after),
        Applied::Unchanged,
        "someone moved it to review after the edit was made"
    );
    let closed = |file: &str, mode| {
        file_of(apply(Some(file), &done, mode))
            .and_then(|next| parse(&next))
            .map(|read| read.status)
    };
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

#[test]
fn an_issue_opens_only_in_a_listed_project_on_a_host_with_a_home() {
    let listed = [project()];
    assert_eq!(
        opening(None, &listed, &project(), &wanted()),
        Err(Rejection::NoHome)
    );
    assert_eq!(
        opening(Some("remote"), &listed, &ProjectId(text("gone")), &wanted()),
        Err(Rejection::UnknownProject)
    );
    assert_eq!(
        opening(
            Some("remote"),
            &listed,
            &project(),
            &IssueFields {
                title: text(""),
                ..wanted()
            }
        ),
        Err(Rejection::IssueNotSaved {
            reason: text("an issue needs a title")
        })
    );
    assert_eq!(
        opening(Some("remote"), &listed, &project(), &wanted())
            .map(|made| made.title),
        Ok(text("Sign in"))
    );
}

#[test]
fn a_change_finds_its_issues_project_and_is_kept_as_the_issue_keeps_it() {
    let issues = BTreeMap::from([(project(), Vec::from([issue()]))]);
    let check = |change| changing(Some("remote"), &issues, ID, change);
    assert_eq!(
        check(IssueChange::Note(text(" hi "))),
        Ok((project(), IssueChange::Note(text("hi"))))
    );
    assert_eq!(
        check(IssueChange::Labels(Vec::from([text(" a "), text("a")]))),
        Ok((project(), IssueChange::Labels(Vec::from([text("a")]))))
    );
    assert_eq!(
        changing(
            Some("remote"),
            &issues,
            IssueId(7),
            IssueChange::Note(text("x"))
        ),
        Err(Rejection::UnknownIssue)
    );
    assert_eq!(
        check(IssueChange::Title(text(" "))),
        Err(Rejection::IssueNotSaved {
            reason: text("a title needs text")
        })
    );
    assert_eq!(
        changing(None, &issues, ID, IssueChange::Note(text("x"))),
        Err(Rejection::NoHome)
    );
}

#[test]
fn each_change_is_its_own_commit() {
    assert_eq!(
        [
            message(&edit(Change::Create(wanted())).data),
            message(&changing_to(IssueChange::Status(text("review"))).data),
            message(&changing_to(IssueChange::Note(text("x"))).data),
        ],
        [
            format!("Open issue {ID}: Sign in"),
            format!("Move issue {ID} to review"),
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

#[test]
fn work_on_an_idle_or_startable_issue_makes_it_active() {
    let flow = flow();
    assert_eq!(
        ["open", "ready", "active", "review", "done", "dropped"]
            .map(|status| started(&flow, status)),
        [
            Some(text("active")),
            Some(text("active")),
            None,
            None,
            None,
            None
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
fn an_issue_needs_a_title() {
    assert_eq!(
        fields(&IssueFields {
            title: text("   "),
            ..wanted()
        }),
        Err(text("an issue needs a title"))
    );
}

#[test]
fn a_finished_issue_whose_branch_gained_commits_is_up_for_review() {
    let flow = flow();
    assert_eq!(
        ["open", "ready", "active", "review", "done", "dropped"]
            .map(|status| finished(&flow, status, 2)),
        [
            Some(text("review")),
            Some(text("review")),
            Some(text("review")),
            None,
            None,
            None,
        ]
    );
    assert_eq!(finished(&flow, "active", 0), None);
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
        IssueChange::Title(current.title.clone()),
        IssueChange::Body(current.body()),
        IssueChange::Labels(current.texts("labels").to_vec()),
        IssueChange::Set {
            field: text("priority"),
            value: text("high"),
        },
        IssueChange::Status(current.status.clone()),
    ] {
        assert_eq!(
            apply(Some(&written()), &changing_to(same.clone()), Mode::Record),
            Applied::Unchanged,
            "{same:?}"
        );
    }
    for (change, from, to) in [
        (
            IssueChange::Title(text("Sign in")),
            "title = \"Sign in to a subscription from the app\"",
            "title = \"Sign in\"",
        ),
        (
            IssueChange::Set {
                field: text("priority"),
                value: text("low"),
            },
            "priority = \"high\"",
            "priority = \"low\"",
        ),
        (
            IssueChange::Body(text("Why.")),
            "What to build and why.",
            "Why.",
        ),
    ] {
        assert_eq!(
            file_of(apply(
                Some(&written()),
                &changing_to(change),
                Mode::Record
            ))?,
            written().replace(from, to)
        );
    }
    Ok(())
}

#[test]
fn a_replay_made_in_the_second_of_the_remotes_last_change_still_applies()
-> Result<(), String> {
    let done: Edit = EditData {
        was: Some(text("active")),
        ..changing_to(IssueChange::Status(text("done"))).data
    }
    .into();
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
        &changing_to(IssueChange::AddItem(text("the protocol"))),
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
            &changing_to(IssueChange::RemoveItem(text("b"))),
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
        &changing_to(IssueChange::Labels(vec![text("new")])),
        Mode::Record,
    ))?;
    assert_eq!(
        labelled,
        original.replace(
            "[\n    \"one,two\", # Keep inline note\n    # Keep standalone note\n    \"three\",\n]",
            "[\n    # Keep inline note\n    # Keep standalone note\n    \"new\"\n]",
        ),
    );
    let titled = file_of(apply(
        Some(&labelled),
        &changing_to(IssueChange::Title(text("New title"))),
        Mode::Record,
    ))?;
    let reread = parse(&titled)?;
    assert_eq!(reread.title, "New title");
    assert_eq!(reread.texts("labels"), [text("new")]);
    assert_eq!(notes(&reread), vec![text("Real note")]);
    assert_eq!(reread.checklist.len(), 1);
    assert!(reread.body().contains("## Checklist"));
    assert!(reread.body().contains("## Notes"));
    assert!(titled.contains("'title' = \"New title\" # Keep title comment"));
    Ok(())
}

#[test]
fn a_newer_label_list_with_a_comma_is_a_distinct_snapshot() {
    let original = written()
        .replace("[\"app\", \"credentials\"]", "[\"app, credentials\"]");
    let change = Change::Issue(IssueChange::Labels(vec![text("queued")]));
    let original_snapshot = was(Some(&original), change.clone());
    assert_ne!(original_snapshot, was(Some(&written()), change.clone()));
    let queued = Edit::from(EditData {
        was: original_snapshot,
        change,
        ..changing_to(IssueChange::Title(text("unused"))).data
    });
    assert_eq!(
        apply(
            Some(&written()),
            &queued,
            Mode::Replay {
                changed: OPENED + 1
            }
        ),
        Applied::Unchanged,
    );
}

#[test]
fn checking_and_removing_the_second_identical_row_keeps_the_first()
-> Result<(), String> {
    let original = bare("\n## Checklist\n- [ ] repeat\n- [ ] repeat\n");
    let snapshot = parse(&original)?.checklist;
    let change = |snapshot, action| {
        changing_to(IssueChange::Checklist {
            snapshot,
            index: 1,
            action,
        })
    };
    let toggle = change(snapshot, ChecklistAction::SetDone(true));
    let checked = file_of(apply(Some(&original), &toggle, Mode::Record))?;
    assert!(checked.ends_with("- [ ] repeat\n- [x] repeat\n"));
    let remove = change(parse(&checked)?.checklist, ChecklistAction::Remove);
    let removed = file_of(apply(Some(&checked), &remove, Mode::Record))?;
    assert!(removed.ends_with("## Checklist\n- [ ] repeat\n"));
    assert!(matches!(
        apply(Some(&removed), &toggle, Mode::Record),
        Applied::Unreadable(_),
    ));
    Ok(())
}

#[test]
fn a_note_is_signed_by_the_host_and_the_client() {
    assert_eq!(super::signer("laptop", "iPhone"), "laptop · iPhone");
}

#[test]
fn an_issues_branch_is_the_one_the_home_lists_for_it() {
    let other = Issue {
        id: IssueId(1),
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
    use super::{Context, Delivery, RecordEffect, Write};

    let issues = BTreeMap::new();
    let flow = flow();
    let context = Context {
        remote: Some("origin"),
        flow: &flow,
        issues: &issues,
        host: "stone",
        at: OPENED,
        command: CommandId(1),
    };
    let request = context.open(&[project()], project(), ID, &wanted())?;
    let target = request.read();
    assert_eq!(target.path, file(&project(), ID));
    let mut recorded = request.decide(None, &PendingEdits::default())?;
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
        edit.replay(&flow, Ok(None), None),
        Write::Commit(commit.clone())
    );
    assert_eq!(commit.target, target);
    assert!(commit.text.contains(&format!("branch = \"issue/{ID}\"")));
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
        edit.replay(&flow, Ok(Some(&commit.text)), Some(OPENED)),
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
    let existing = context.open(&[project()], project(), ID, &wanted())?;
    assert_eq!(
        existing
            .decide(Some(&commit.text), &PendingEdits::default())?
            .next(Ok(()))?,
        None
    );
    Ok(())
}

#[test]
fn issue_changes_decide_each_commit_and_replay_against_the_file()
-> Result<(), Rejection> {
    use super::{Context, RecordEffect, Write};

    let issues = BTreeMap::from([(project(), vec![issue()])]);
    let flow = flow();
    let context = Context {
        remote: Some("origin"),
        flow: &flow,
        issues: &issues,
        host: "stone",
        at: OPENED,
        command: CommandId(1),
    };
    let changes = [
        (IssueChange::Title(text("New title")), "Retitle issue"),
        (IssueChange::Body(text("New body")), "Describe issue"),
        (IssueChange::Labels(vec![text("new")]), "Label issue"),
        (
            IssueChange::Set {
                field: text("priority"),
                value: text("low"),
            },
            "Set the priority of issue",
        ),
        (IssueChange::Status(text("review")), "Move issue"),
        (IssueChange::AddItem(text("Test the change")), "Add an item"),
        (
            IssueChange::Check {
                item: text("Test the change"),
                done: true,
            },
            "Check off",
        ),
        (
            IssueChange::RemoveItem(text("Test the change")),
            "Remove an item",
        ),
        (IssueChange::Note(text("Summary")), "Note on issue"),
    ];
    let mut current = written();
    for (index, (change, message)) in changes.into_iter().enumerate() {
        let each = Context {
            command: CommandId(index as u128),
            ..context
        };
        let request = each.edit(ID, change, "laptop")?;
        let mut recorded =
            request.decide(Some(&current), &PendingEdits::default())?;
        let Some(RecordEffect::Persist(edit)) = recorded.next(Ok(()))? else {
            return Err(unexpected_effect());
        };
        let Some(RecordEffect::Commit(commit)) = recorded.next(Ok(()))? else {
            return Err(unexpected_effect());
        };
        assert!(commit.message.starts_with(message));
        assert_eq!(
            edit.replay(&flow, Ok(Some(&current)), Some(OPENED)),
            Write::Commit(commit.clone())
        );
        assert_eq!(
            edit.replay(&flow, Ok(Some(&commit.text)), Some(OPENED)),
            Write::Unchanged
        );
        assert_eq!(edit.replay(&flow, Ok(None), None), Write::Dropped);
        assert!(matches!(
            edit.replay(&flow, Ok(Some("malformed")), Some(OPENED)),
            Write::Waiting(_)
        ));
        current = commit.text;
    }
    assert!(current.contains("stone \u{b7} laptop: Summary"));
    let fresh = Context {
        command: CommandId(100),
        ..context
    };
    let request =
        fresh.edit(ID, IssueChange::Title(text("Older edit")), "laptop")?;
    let mut recorded =
        request.decide(Some(&written()), &PendingEdits::default())?;
    let Some(RecordEffect::Persist(edit)) = recorded.next(Ok(()))? else {
        return Err(unexpected_effect());
    };
    let Write::Commit(resolved) =
        edit.replay(&flow, Ok(Some(&current)), Some(OPENED + 1))
    else {
        return Err(unexpected_effect());
    };
    assert!(resolved.text.contains("title = \"New title\""));
    assert!(!resolved.text.contains("Older edit"));
    let reverted = resolved.text.replace("New title", &issue().title);
    assert_eq!(
        edit.replay(&flow, Ok(Some(&reverted)), Some(OPENED + 2)),
        Write::Unchanged
    );
    assert!(matches!(
        edit.replay(&flow, Ok(Some(&current)), Some(OPENED)),
        Write::Commit(_)
    ));
    Ok(())
}

#[test]
fn replay_preserves_separate_notes_with_identical_text() -> Result<(), Rejection>
{
    use super::{Context, RecordEffect, Write};

    let issues = BTreeMap::from([(project(), vec![issue()])]);
    let flow = flow();
    let context = Context {
        remote: Some("origin"),
        flow: &flow,
        issues: &issues,
        host: "stone",
        at: OPENED,
        command: CommandId(1),
    };
    let note = || IssueChange::Note(text("Same note"));
    let record = |command, current: &str| -> Result<Edit, Rejection> {
        let request_context = Context { command, ..context };
        let mut recorded = request_context
            .edit(ID, note(), "laptop")?
            .decide(Some(current), &PendingEdits::default())?;
        while let Some(effect) = recorded.next(Ok(()))? {
            if let RecordEffect::Persist(edit) = effect {
                return Ok(edit);
            }
        }
        Err(unexpected_effect())
    };
    let original = written();
    let first = record(CommandId(1), &original)?;
    let Write::Commit(first_commit) =
        first.replay(&flow, Ok(Some(&original)), Some(OPENED))
    else {
        return Err(unexpected_effect());
    };
    assert_eq!(
        context
            .edit(ID, note(), "laptop")?
            .decide(Some(&first_commit.text), &PendingEdits::default())?
            .next(Ok(()))?,
        None
    );
    let second = record(CommandId(2), &first_commit.text)?;
    let Write::Commit(second_commit) =
        second.replay(&flow, Ok(Some(&first_commit.text)), Some(OPENED))
    else {
        return Err(unexpected_effect());
    };
    assert_eq!(second_commit.text.matches(": Same note").count(), 2);
    assert_eq!(
        first.replay(&flow, Ok(Some(&second_commit.text)), Some(OPENED)),
        Write::Unchanged
    );
    assert_eq!(
        second.replay(&flow, Ok(Some(&second_commit.text)), Some(OPENED)),
        Write::Unchanged
    );
    Ok(())
}

#[test]
fn a_session_updates_its_issue_using_the_listing_after_the_note() {
    use super::SessionUpdate;

    let flow = flow();
    let mut known = issue();
    known.status = text("open");
    let open = BTreeMap::from([(project(), vec![known.clone()])]);
    let mut start = SessionUpdate::Started;
    assert_eq!(
        start.next(&flow, &open, ID),
        Some(IssueChange::Transition {
            from: text("open"),
            to: text("active")
        })
    );
    assert_eq!(start.next(&flow, &open, ID), None);

    let mut finish = SessionUpdate::Finished {
        summary: Some(text("Summary")),
        landed: 1,
    };
    assert_eq!(
        finish.next(&flow, &open, ID),
        Some(IssueChange::Note(text("Summary")))
    );
    known.status = text("done");
    let closed = BTreeMap::from([(project(), vec![known])]);
    assert_eq!(finish.next(&flow, &closed, ID), None);

    let mut finish = SessionUpdate::Finished {
        summary: None,
        landed: 1,
    };
    assert_eq!(
        finish.next(&flow, &open, ID),
        Some(IssueChange::Transition {
            from: text("open"),
            to: text("review")
        })
    );
    assert_eq!(finish.next(&flow, &open, ID), None);
}

#[test]
fn a_competing_done_commit_supersedes_a_session_finishing_its_issue() {
    use super::SessionUpdate;

    let known = issue();
    let listing = BTreeMap::from([(project(), vec![known])]);
    let mut finish = SessionUpdate::Finished {
        summary: None,
        landed: 1,
    };
    let transition = finish.next(&flow(), &listing, ID).map(changing_to);
    let done = written().replace("status = \"active\"", "status = \"done\"");
    assert_eq!(
        transition.as_ref().map(|edit| apply(
            Some(&done),
            edit,
            Mode::Replay { changed: OPENED }
        )),
        Some(Applied::Unchanged),
    );
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
