use super::{
    Handle, Intent, Next, Progress, Record, Refusal, RelationWork, relating,
};
use crate::issue::{
    Comment, CommentRequest, CommentWrite, EditId, Lookup, RelationChange,
    RelationRequest,
};
use crate::prelude::*;
use crate::{ProjectId, UnixMillis};

const SENT: UnixMillis = UnixMillis(10);

const HANDLE: Handle = Handle {
    owner: [3; 32],
    id: EditId(5),
};

fn comment(id: &str, body: &str) -> Comment {
    Comment {
        id: String::from(id),
        url: format!("https://github.com/owner/work/issues/146#{id}"),
        body: String::from(body),
        created: UnixMillis(1),
        updated: UnixMillis(1),
    }
}

fn record(
    write: CommentWrite,
    observed: &[Comment],
) -> Result<Record, Refusal> {
    let seen = write.precondition(observed);
    let request = CommentRequest {
        issue: Lookup {
            project: ProjectId::new(1),
            id: Some(crate::id::issue_id("146")).ok_or(Refusal::Conflict)?,
        },
        write,
        seen,
    };
    Ok(Record::new(HANDLE, Intent::comment(HANDLE, &request)?))
}

fn create() -> CommentWrite {
    CommentWrite::Create {
        body: format!("text {}", HANDLE.marker()),
    }
}

#[test]
fn a_comment_without_its_operation_marker_is_refused() {
    let write = CommentWrite::Create {
        body: String::from("unmarked"),
    };
    assert_eq!(record(write, &[]).err(), Some(Refusal::Unmarked));
}

#[test]
fn two_comments_with_one_marker_leave_the_outcome_unknown()
-> Result<(), Refusal> {
    let (sent, _) = record(create(), &[])?.next(&[], false, SENT)?;
    let marked = comment("2", create().body());
    let twice = [marked.clone(), comment("3", marked.body.as_str())];
    let (kept, next) = sent.next(&twice, false, SENT)?;
    assert_eq!(next, Next::Unknown);
    assert_eq!(kept.progress(), sent.progress());
    Ok(())
}

#[test]
fn an_edit_of_a_comment_the_issue_lacks_is_a_conflict() -> Result<(), Refusal> {
    let write = CommentWrite::Edit {
        comment: String::from("9"),
        body: format!("new {}", HANDLE.marker()),
    };
    let fresh = record(write, &[])?;
    assert_eq!(
        fresh.next(&[comment("1", "other")], false, SENT).err(),
        Some(Refusal::Conflict)
    );
    Ok(())
}

#[test]
fn a_rejected_send_can_be_sent_again() -> Result<(), Refusal> {
    let (sent, next) = record(create(), &[])?.next(&[], false, SENT)?;
    assert_eq!(next, Next::Send);
    let rejected = sent.reject()?;
    assert_eq!(rejected.progress(), &Progress::Ready);
    assert_eq!(rejected.next(&[], false, SENT)?.1, Next::Send);
    assert_eq!(rejected.reject().err(), Some(Refusal::NoAttempt));
    Ok(())
}

#[test]
fn only_a_comment_sent_and_unconfirmed_is_awaited_with_the_request_it_sent()
-> Result<(), Refusal> {
    let ready = record(create(), &[])?;
    let (sent, _) = ready.next(&[], false, SENT)?;
    let marked = comment("2", create().body());
    let (settled, _) =
        sent.next(core::slice::from_ref(&marked), false, SENT)?;
    assert_eq!(
        [ready.awaited(), sent.awaited(), settled.awaited()],
        [
            None,
            Some(CommentRequest {
                issue: Lookup {
                    project: ProjectId::new(1),
                    id: Some(crate::id::issue_id("146"))
                        .ok_or(Refusal::Conflict)?,
                },
                write: create(),
                seen: create().precondition(&[]),
            }),
            None,
        ]
    );
    Ok(())
}

#[test]
fn a_resumed_comment_is_sent_again_only_once_a_newer_comment_shows_the_list_caught_up()
-> Result<(), Refusal> {
    let older = comment("1", "a person's note");
    let (sent, _) = record(create(), &[])?.next(&[], false, SENT)?;
    let (lagging, next) =
        sent.next(core::slice::from_ref(&older), true, UnixMillis(30))?;
    assert_eq!((next, lagging.progress()), (Next::Unknown, sent.progress()));
    let newer = Comment {
        created: UnixMillis(20),
        ..comment("2", "a later note")
    };
    let (again, next) = sent.next(&[older, newer], true, UnixMillis(30))?;
    assert_eq!((next, again.progress()), (Next::Send, sent.progress()));
    Ok(())
}

#[test]
fn an_unsettled_relation_write_is_resumed_first_and_a_changed_one_follows_it()
-> Result<(), Refusal> {
    let issue = Lookup {
        project: ProjectId::new(1),
        id: Some(crate::id::issue_id("146")).ok_or(Refusal::Conflict)?,
    };
    let parent = |number: &str| {
        Some(crate::id::issue_id(number))
            .map(|id| RelationChange {
                relation: String::from("parent"),
                targets: Vec::from([crate::IssueKey {
                    project: ProjectId::new(1),
                    id,
                }]),
            })
            .ok_or(Refusal::Conflict)
    };
    let request = RelationRequest {
        issue: issue.clone(),
        change: parent("140")?,
        seen: String::from("seen"),
    };
    let unsettled = [Record::new(HANDLE, Intent::relate(&request))];
    let resume = RelationWork::Resume {
        handle: HANDLE,
        request,
    };
    assert_eq!(
        relating(&unsettled, &issue, Vec::from([parent("140")?])),
        core::slice::from_ref(&resume)
    );
    assert_eq!(
        relating(&unsettled, &issue, Vec::from([parent("141")?])),
        [resume, RelationWork::Write(parent("141")?)]
    );
    Ok(())
}
