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

/// A hand-written issue file with the front matter Conductor needs and
/// `rest` after it.
fn bare(rest: &str) -> String {
    format!(
        "+++\nid = \"{id}\"\ntitle = \"Sign in\"\nstatus = \"open\"\n+++\n{rest}",
        id = name(ID)
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
        IssueChange::Body(current.body.clone()),
        IssueChange::Labels(current.labels.clone()),
        IssueChange::Priority(current.priority),
        IssueChange::Status(current.status),
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
            IssueChange::Priority(Priority::Low),
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
    let done = Edit {
        was: Some(text("in-progress")),
        ..changing_to(IssueChange::Status(IssueStatus::Done))
    };
    let reviewed = written().replace("in-progress", "review");
    let replayed = file_of(apply(
        Some(&reviewed),
        &done,
        Mode::Replay { changed: OPENED },
    ))?;
    assert_eq!(
        parse(&name(ID), &replayed).map(|read| read.status),
        Ok(IssueStatus::Done)
    );
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
    assert_eq!(
        parse(&name(ID), &again).map(|read| read.checklist.len()),
        Ok(3)
    );
    let noted = file_of(apply(
        Some(&written()),
        &changing_to(IssueChange::Note(text("later"))),
        Mode::Replay {
            changed: OPENED + 9,
        },
    ))?;
    assert_eq!(
        parse(&name(ID), &noted).map(|read| read.notes.len()),
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
fn a_section_conductor_doesnt_know_is_left_out() {
    let file = bare(
        "\nWhy.\n\n## Checklist\n- [ ] one\n\n## Design\n- [ ] not an item\n\n\
         ## Notes\n- a note\n\n## Later\n- not a note\n",
    );
    assert_eq!(
        parse(&name(ID), &file).map(|read| (read.checklist.len(), read.notes)),
        Ok((1, Vec::from([text("a note")])))
    );
}

#[test]
fn a_comment_is_a_hash_outside_a_string() {
    assert_eq!(
        super::comment(r#"title = "a \" # b"   # note"#),
        "   # note"
    );
    assert_eq!(super::comment("title = 'x#y' # n"), " # n");
    assert_eq!(super::comment(r#"title = "it's # not" # n"#), " # n");
    assert_eq!(super::comment(r#"title = "a#b""#), "");
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
fn opening_decides_the_file_commit_and_delivery() -> Result<(), Rejection> {
    use super::{Context, Delivery, RecordEffect, Write};

    let issues = BTreeMap::new();
    let context = Context {
        remote: Some("origin"),
        issues: &issues,
        host: "stone",
        at: OPENED,
    };
    let request = context.open(&[project()], project(), ID, &wanted())?;
    let target = request.read();
    assert_eq!(target.path, file(&project(), ID));
    let mut recorded = request.decide(None)?;
    let Some(RecordEffect::Commit(commit)) = recorded.next(Ok(()))? else {
        return Err(unexpected_effect());
    };
    let mut failed = recorded.clone();
    let reason = text("commit failed");
    assert_eq!(
        failed.next(Err(reason.clone())),
        Err(Rejection::IssueNotSaved { reason })
    );
    assert_eq!(failed.next(Ok(()))?, None);
    assert_eq!(commit.target, target);
    assert!(
        commit
            .text
            .contains(&format!("branch = \"issue/{}\"", name(ID)))
    );
    assert!(commit.message.starts_with("Open issue "));
    assert_eq!(commit.author, "Conductor");
    assert_eq!(commit.email, "conductor@localhost");
    let Some(RecordEffect::Enqueue(edit)) = recorded.next(Ok(()))? else {
        return Err(unexpected_effect());
    };
    assert_eq!(recorded.next(Ok(()))?, Some(RecordEffect::Persist));
    assert_eq!(
        recorded.next(Ok(()))?,
        Some(RecordEffect::Deliver(Delivery::Push { retry: true }))
    );
    assert_eq!(
        recorded.next(Err(text("offline")))?,
        Some(RecordEffect::Scan)
    );
    assert_eq!(recorded.next(Ok(()))?, None);
    assert_eq!(edit.replay(Some(&commit.text), OPENED), Write::Unchanged);
    let existing = context.open(&[project()], project(), ID, &wanted())?;
    let mut unchanged = existing.decide(Some(&commit.text))?;
    assert!(matches!(
        unchanged.next(Ok(()))?,
        Some(RecordEffect::Enqueue(_))
    ));
    Ok(())
}

#[test]
fn issue_changes_decide_each_commit_and_replay_against_the_file()
-> Result<(), Rejection> {
    use super::{Context, RecordEffect, Write};

    let issues = BTreeMap::from([(project(), vec![issue()])]);
    let context = Context {
        remote: Some("origin"),
        issues: &issues,
        host: "stone",
        at: OPENED,
    };
    let changes = [
        (IssueChange::Title(text("New title")), "Retitle issue"),
        (IssueChange::Body(text("New body")), "Describe issue"),
        (IssueChange::Labels(vec![text("new")]), "Label issue"),
        (IssueChange::Priority(Priority::Low), "Give issue"),
        (IssueChange::Status(IssueStatus::Review), "Move issue"),
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
    for (change, message) in changes {
        let request = context.edit(ID, change, "laptop")?;
        let mut recorded = request.decide(Some(&current))?;
        let Some(RecordEffect::Commit(commit)) = recorded.next(Ok(()))? else {
            return Err(unexpected_effect());
        };
        let Some(RecordEffect::Enqueue(edit)) = recorded.next(Ok(()))? else {
            return Err(unexpected_effect());
        };
        assert!(commit.message.starts_with(message));
        assert_eq!(
            edit.replay(Some(&current), OPENED),
            Write::Commit(commit.clone())
        );
        assert_eq!(edit.replay(Some(&commit.text), OPENED), Write::Unchanged);
        assert_eq!(edit.replay(None, OPENED), Write::Dropped);
        assert!(matches!(
            edit.replay(Some("malformed"), OPENED),
            Write::Waiting(_)
        ));
        current = commit.text;
    }
    assert!(current.contains("stone \u{b7} laptop: Summary"));
    let request =
        context.edit(ID, IssueChange::Title(text("Older edit")), "laptop")?;
    let mut recorded = request.decide(Some(&written()))?;
    assert!(matches!(
        recorded.next(Ok(()))?,
        Some(RecordEffect::Commit(_))
    ));
    let Some(RecordEffect::Enqueue(edit)) = recorded.next(Ok(()))? else {
        return Err(unexpected_effect());
    };
    assert_eq!(edit.replay(Some(&current), OPENED + 1), Write::Unchanged);
    assert!(matches!(
        edit.replay(Some(&current), OPENED),
        Write::Commit(_)
    ));
    Ok(())
}

#[test]
fn a_session_updates_its_issue_using_the_listing_after_the_note() {
    use super::SessionUpdate;

    let mut known = issue();
    known.status = IssueStatus::Open;
    let open = BTreeMap::from([(project(), vec![known.clone()])]);
    let mut start = SessionUpdate::Started;
    assert_eq!(
        start.next(&open, ID),
        Some(IssueChange::Status(IssueStatus::InProgress))
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
    known.status = IssueStatus::Done;
    let closed = BTreeMap::from([(project(), vec![known])]);
    assert_eq!(finish.next(&closed, ID), None);

    let mut finish = SessionUpdate::Finished {
        summary: None,
        landed: 1,
    };
    assert_eq!(
        finish.next(&open, ID),
        Some(IssueChange::Status(IssueStatus::Review))
    );
    assert_eq!(finish.next(&open, ID), None);
}

#[test]
fn a_failed_push_fetches_and_replays_before_acknowledging_edits() {
    use super::Delivery::{
        self, Done, Fetch, Landed, Persist, Push, Reset, Rewrite,
    };

    let push = Push { retry: true };
    let retry = Push { retry: false };
    assert_eq!(push.after(true), Landed);
    assert_eq!(push.after(false), Fetch);
    assert_eq!(Fetch.after(false), Done);
    assert_eq!(Fetch.after(true), Reset);
    assert_eq!(Delivery::replay(true), Reset);
    assert_eq!(Delivery::replay(false), Done);
    assert_eq!(Reset.after(false), Done);
    assert_eq!(Reset.after(true), Rewrite);
    assert_eq!(Rewrite.after(true), Persist);
    assert_eq!(Persist.after(true), retry);
    assert_eq!(retry.after(false), Done);
    assert_eq!(retry.after(true), Landed);
    assert_eq!(Landed.after(true), Done);
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
            refspec: format!("+refs/heads/issue/task:{tracking}"),
            tracking: tracking.clone()
        }
    );
    assert_eq!(fetch.clone().after(Err(text("fetch failed"))), Reachability);
    assert_eq!(Reachability.after(Ok(text("origin reachable"))), absent);
    assert_eq!(
        Reachability.after(Err(text("offline"))),
        Observed(Remote::Unreachable(text("offline")))
    );
    let read = fetch.after(Ok(String::new()));
    assert_eq!(read, Tracking(tracking));
    assert_eq!(read.clone().after(Err(text("unknown ref"))), absent);
    assert_eq!(
        read.after(Ok(text("abc"))),
        Observed(Remote::At(text("abc")))
    );
}
