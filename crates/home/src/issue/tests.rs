use std::collections::BTreeMap;

use proto::{
    ChecklistItem, Checkout, Cloning, Home, HomeSync, HostPath, Issue,
    IssueChange, IssueFields, IssueId, IssueStatus, Origin, Priority,
    ProjectId, Rejection,
};

use super::{
    Applied, Branch, Change, Edit, Mode, Move, Remote, apply, branch, changing,
    fields, file, finished, listed, message, name, opening, parse, render,
    start, started, was,
};

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

fn issue() -> Issue {
    Issue {
        id: ID,
        title: text("Sign in to a subscription from the app"),
        body: text("What to build and why."),
        status: IssueStatus::InProgress,
        priority: Priority::High,
        labels: Vec::from([text("app"), text("credentials")]),
        branch: format!("issue/{}", name(ID)),
        created: text("2026-11-18T10:12:00Z"),
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
        notes: Vec::from([text(
            "2026-11-18 laptop · codex · api: built the protocol; browser scenario\nleft; landed on the issue branch.",
        )]),
    }
}

fn written() -> String {
    format!(
        "+++\n\
         id = \"{id}\"\n\
         title = \"Sign in to a subscription from the app\"\n\
         status = \"in-progress\"\n\
         priority = \"high\"\n\
         labels = [\"app\", \"credentials\"]\n\
         branch = \"issue/{id}\"\n\
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
        id = name(ID)
    )
}

/// The file as a person left it: a comment on the status, a key and a
/// table Conductor doesn't read, a section of their own, Windows line
/// ends.
fn hand_edited() -> String {
    format!(
        "+++\n\
         id = \"{id}\"\n\
         # set by hand\n\
         title = \"Sign in\"\n\
         status = \"open\"   # open, ready, in-progress, review, done, dropped\n\
         estimate = 3\n\
         [links]\n\
         pr = \"https://example.com/1\"\n\
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
        id = name(ID)
    )
    .replace('\n', "\r\n")
}

fn edit(change: Change) -> Edit {
    Edit {
        project: project(),
        issue: ID,
        at: OPENED,
        by: text("laptop · iPhone"),
        change,
        was: None,
    }
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
fn an_issue_file_reads_and_a_new_one_writes_in_the_same_form() {
    assert_eq!(parse(&name(ID), &written()), Ok(issue()));
    assert_eq!(render(&issue()), written());
}

#[test]
fn a_change_rewrites_only_its_own_line_of_a_hand_edited_file()
-> Result<(), String> {
    let unix = hand_edited().replace("\r\n", "\n");
    let moved = file_of(apply(
        Some(&hand_edited()),
        &changing_to(IssueChange::Status(IssueStatus::Review)),
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
            .replace("estimate = 3\n", "estimate = 3\nlabels = [\"app\"]\n")
            .replace("- [X] the protocol\n", "- [X] the protocol\n- [ ] docs\n")
            .replace(
                "- first\n",
                "- first\n- 2026-11-18 laptop · iPhone: second\n"
            ),
        "the comment, the key, the table, the [X] and the section stay"
    );
    let read = parse(&name(ID), &hand_edited()).map(|issue| {
        (issue.status, issue.body, issue.checklist.len(), issue.notes)
    });
    assert_eq!(
        read,
        Ok((
            IssueStatus::Open,
            text("Why."),
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
        parse(&name(ID), &removed).map(|read| read.checklist),
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
    assert_eq!(name(ID), "01jd0000000000000000000001");
    assert_eq!(
        file(&project(), ID),
        "projects/01m7r5n23kp17wyfj138e0bcnf/issues/01jd0000000000000000000001.md"
    );
    assert_eq!(
        parse("01jd0000000000000000000002", &written()).map(|_| ()),
        Err(text("its id is not its file's name"))
    );
    assert_eq!(
        parse(&name(ID), &written().replace("in-progress", "doing"))
            .map(|_| ()),
        Err(text(
            "status \"doing\" is not one of open, ready, in-progress, review, done, dropped"
        ))
    );
    assert_eq!(
        parse(&name(ID), "no front matter").map(|_| ()),
        Err(text("it does not start with +++ front matter"))
    );
}

#[test]
fn a_project_lists_its_issues_by_id_and_says_why_one_is_skipped() {
    let (issues, problems) = listed(
        &project(),
        Vec::from([
            (text("not-an-issue"), Ok(text("+++\n+++\n"))),
            (name(ID), Ok(written())),
            (text("unreadable"), Err(text("permission denied"))),
        ]),
    );
    assert_eq!(issues, Vec::from([issue()]));
    assert_eq!(problems.len(), 2);
}

fn wanted() -> IssueFields {
    IssueFields {
        title: text("  Sign in  "),
        body: text("Why.\n"),
        priority: Priority::Normal,
        labels: Vec::from([text(" app "), text(""), text("app")]),
    }
}

#[test]
fn creating_opens_the_issue_on_its_own_branch_and_again_changes_nothing()
-> Result<(), String> {
    let create = edit(Change::Create(wanted()));
    let opened = file_of(apply(None, &create, Mode::Record))?;
    assert_eq!(
        parse(&name(ID), &opened),
        Ok(Issue {
            title: text("Sign in"),
            body: text("Why."),
            status: IssueStatus::Open,
            priority: Priority::Normal,
            labels: Vec::from([text("app")]),
            checklist: Vec::new(),
            notes: Vec::new(),
            ..issue()
        })
    );
    assert_eq!(
        apply(Some(&opened), &create, Mode::Replay { changed: 0 }),
        Applied::Unchanged
    );
    Ok(())
}

#[test]
fn a_note_is_signed_kept_twice_when_made_twice_and_replayed_once()
-> Result<(), String> {
    let note = changing_to(IssueChange::Note(text("done")));
    let once = file_of(apply(Some(&written()), &note, Mode::Record))?;
    let twice = file_of(apply(Some(&once), &note, Mode::Record))?;
    let notes = |file: &str| parse(&name(ID), file).map(|read| read.notes);
    assert_eq!(
        notes(&twice).map(|all| all.into_iter().skip(1).collect::<Vec<_>>()),
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
    let done = Edit {
        was: was(
            Some(&written()),
            ID,
            &Change::Issue(IssueChange::Status(IssueStatus::Done)),
        ),
        ..changing_to(IssueChange::Status(IssueStatus::Done))
    };
    assert_eq!(done.was, Some(text("in-progress")));
    let reviewed = written().replace("in-progress", "review");
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
            .and_then(|next| parse(&name(ID), &next))
            .map(|read| read.status)
    };
    assert_eq!(closed(&reviewed, before), Ok(IssueStatus::Done));
    assert_eq!(
        closed(&written(), after),
        Ok(IssueStatus::Done),
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
            message(&edit(Change::Create(wanted()))),
            message(&changing_to(IssueChange::Status(IssueStatus::Review))),
            message(&changing_to(IssueChange::Note(text("x")))),
        ],
        [
            format!("Open issue {}: Sign in", name(ID)),
            format!("Move issue {} to review", name(ID)),
            format!("Note on issue {}", name(ID)),
        ]
    );
}

fn home(checkouts: Vec<Checkout>, issues: Vec<Issue>) -> Home {
    Home {
        remote: text("file:///home.git"),
        synced: None,
        sync: HomeSync::Synced,
        unpushed: 0,
        projects: Vec::from([proto::Project {
            id: project(),
            name: text("conductor"),
            title: text("Conductor"),
            remote: None,
            profiles: Vec::new(),
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
            format!("refs/heads/issue/{}", name(ID)),
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
        format!("issue/{}", name(ID)).as_str(),
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
fn work_on_an_open_or_ready_issue_puts_it_in_progress() {
    assert_eq!(
        IssueStatus::ALL.map(started),
        [
            Some(IssueStatus::InProgress),
            Some(IssueStatus::InProgress),
            None,
            None,
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
    assert_eq!(
        IssueStatus::ALL.map(|status| finished(status, 2)),
        [
            Some(IssueStatus::Review),
            Some(IssueStatus::Review),
            Some(IssueStatus::Review),
            None,
            None,
            None,
        ]
    );
    assert_eq!(finished(IssueStatus::InProgress, 0), None);
    let known = home(Vec::from([checkout()]), Vec::from([issue()]));
    assert_eq!(
        branch(Some(&known), ID),
        format!("refs/heads/issue/{}", name(ID))
    );
    assert_eq!(branch(None, ID), format!("refs/heads/issue/{}", name(ID)));
}
