use std::collections::BTreeMap;

use proto::{
    ChecklistItem, Issue, IssueChange, IssueFields, IssueId, IssueStatus,
    Priority, ProjectId, Rejection,
};

use super::{
    Applied, Change, Edit, apply, changing, fields, file, listed, message,
    name, opening, parse, render,
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

#[test]
fn an_issue_file_reads_and_writes_back_the_same() {
    let parsed = parse(&name(ID), &written());
    assert_eq!(parsed.as_ref().map(|read| read.issue.clone()), Ok(issue()));
    assert_eq!(parsed.map(|read| render(&read)), Ok(written()));
}

#[test]
fn front_matter_a_person_added_is_kept() {
    let added = written().replacen(
        "created = 2026-11-18T10:12:00Z\n",
        "created = 2026-11-18T10:12:00Z\nestimate = 3\n",
        1,
    );
    assert_eq!(
        parse(&name(ID), &added).map(|read| render(&read)),
        Ok(added)
    );
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

fn edit(change: Change) -> Edit {
    Edit {
        project: project(),
        issue: ID,
        at: OPENED,
        by: text("laptop · iPhone"),
        change,
    }
}

fn wanted() -> IssueFields {
    IssueFields {
        title: text("  Sign in  "),
        body: text("Why.\n"),
        priority: Priority::Normal,
        labels: Vec::from([text(" app "), text(""), text("app")]),
    }
}

/// The file an edit writes, or what it did instead.
fn file_of(applied: Applied) -> Result<String, String> {
    match applied {
        Applied::Write(text) => Ok(text),
        other => Err(format!("expected a file, not {other:?}")),
    }
}

#[test]
fn creating_opens_the_issue_on_its_own_branch_and_again_changes_nothing()
-> Result<(), String> {
    let opened = file_of(apply(None, &edit(Change::Create(wanted()))))?;
    assert_eq!(
        parse(&name(ID), &opened).map(|read| read.issue),
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
        apply(Some(&opened), &edit(Change::Create(wanted()))),
        Applied::Unchanged
    );
    Ok(())
}

#[test]
fn a_change_sets_its_fields_and_a_note_is_signed_and_added_once()
-> Result<(), String> {
    let note = edit(Change::Issue(IssueChange::Note(text("looked at it"))));
    let with_note = file_of(apply(Some(&written()), &note))?;
    assert_eq!(
        parse(&name(ID), &with_note).map(|read| read
            .issue
            .notes
            .last()
            .cloned()),
        Ok(Some(text("2026-11-18 laptop · iPhone: looked at it")))
    );
    assert_eq!(apply(Some(&with_note), &note), Applied::Unchanged);
    let done = edit(Change::Issue(IssueChange::Status(IssueStatus::Done)));
    let closed = file_of(apply(Some(&written()), &done))?;
    assert_eq!(
        parse(&name(ID), &closed).map(|read| read.issue.status),
        Ok(IssueStatus::Done)
    );
    assert_eq!(
        apply(None, &done),
        Applied::Gone,
        "a change to an issue the home no longer has is dropped"
    );
    Ok(())
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
            .map(|fields| fields.title),
        Ok(text("Sign in"))
    );
}

#[test]
fn a_change_finds_its_issues_project() {
    let issues = BTreeMap::from([(project(), Vec::from([issue()]))]);
    let note = IssueChange::Note(text("hi"));
    assert_eq!(
        changing(Some("remote"), &issues, ID, note.clone()),
        Ok((project(), note.clone()))
    );
    assert_eq!(
        changing(Some("remote"), &issues, IssueId(7), note.clone()),
        Err(Rejection::UnknownIssue)
    );
    assert_eq!(
        changing(Some("remote"), &issues, ID, IssueChange::Note(text(" "))),
        Err(Rejection::IssueNotSaved {
            reason: text("a note needs text")
        })
    );
    assert_eq!(changing(None, &issues, ID, note), Err(Rejection::NoHome));
}

#[test]
fn each_change_is_its_own_commit() {
    assert_eq!(
        [
            message(&edit(Change::Create(wanted()))),
            message(&edit(Change::Issue(IssueChange::Status(
                IssueStatus::Review
            )))),
            message(&edit(Change::Issue(IssueChange::Note(text("x"))))),
        ],
        [
            format!("Open issue {}: Sign in", name(ID)),
            format!("Move issue {} to review", name(ID)),
            format!("Note on issue {}", name(ID)),
        ]
    );
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
