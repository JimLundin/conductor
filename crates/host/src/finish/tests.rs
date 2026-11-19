use proto::{
    ClientName, CloseReason, Event, EventKind, Finished, HostPath, HostSeq,
    InstanceId, IssueId, Kept, Provider, Rejection, SessionId, TurnOutcome,
    Workspace,
};

use super::{
    Branches, Finishing, Landing, checked_out, finished, finishing, landed,
    summary, turn_ended,
};
use crate::assignment::Assignment;

const SESSION: SessionId = SessionId(1);
const ISSUE: IssueId = IssueId(9);

fn text(value: &str) -> String {
    String::from(value)
}

fn event(seq: u64, kind: EventKind) -> Event {
    Event {
        seq: HostSeq(seq),
        session: SESSION,
        kind,
    }
}

fn workspace() -> Workspace {
    Workspace {
        repository: HostPath(text("/src")),
        original: HostPath(text("/src")),
        worktree: HostPath(text("/data/worktrees/1")),
        directory: HostPath(text("/data/worktrees/1")),
        branch: text("conductor/1"),
    }
}

fn started() -> Vec<Event> {
    Vec::from([
        event(
            1,
            EventKind::SessionCreated {
                instance: InstanceId::own(Provider::Claude),
                provider: Provider::Claude,
                directory: HostPath(text("/src")),
                by: ClientName(text("phone")),
                model: None,
                effort: None,
            },
        ),
        event(2, EventKind::OnIssue { issue: ISSUE }),
        event(
            3,
            EventKind::WorkspaceCreated {
                workspace: workspace(),
            },
        ),
        event(4, EventKind::SessionReady),
    ])
}

fn and(mut history: Vec<Event>, more: Vec<Event>) -> Vec<Event> {
    history.extend(more);
    history
}

fn user(seq: u64) -> Event {
    event(
        seq,
        EventKind::UserMessage {
            text: text("go"),
            by: ClientName(text("phone")),
        },
    )
}

fn reply(seq: u64, words: &str) -> Event {
    event(seq, EventKind::AssistantMessage { text: text(words) })
}

fn done(seq: u64) -> Event {
    event(
        seq,
        EventKind::TurnCompleted {
            outcome: TurnOutcome::Completed,
        },
    )
}

#[test]
fn a_session_on_an_issue_finishes_between_turns() {
    assert_eq!(
        finishing(&started(), SESSION),
        Ok(Finishing {
            assignment: Assignment {
                issue: ISSUE,
                instance: InstanceId::own(Provider::Claude),
            },
            workspace: workspace(),
            running: true,
        })
    );
    assert_eq!(
        finishing(&and(started(), Vec::from([user(5)])), SESSION),
        Err(Rejection::TurnInProgress)
    );
    let closed = and(
        started(),
        Vec::from([
            user(5),
            event(
                6,
                EventKind::SessionClosed {
                    reason: CloseReason::Exited { status: text("0") },
                },
            ),
        ]),
    );
    assert_eq!(
        finishing(&closed, SESSION).map(|it| it.running),
        Ok(false),
        "a closed session finishes without asking its agent"
    );
    let plain = started()
        .into_iter()
        .filter(|event| !matches!(event.kind, EventKind::OnIssue { .. }))
        .collect::<Vec<Event>>();
    assert_eq!(finishing(&plain, SESSION), Err(Rejection::NotOnIssue));
    assert_eq!(
        finishing(
            &and(
                started(),
                Vec::from([event(5, EventKind::SessionRemoved { kept: None })])
            ),
            SESSION
        ),
        Err(Rejection::SessionRemoved)
    );
    assert_eq!(finishing(&[], SESSION), Err(Rejection::UnknownSession));
}

#[test]
fn the_summary_is_the_last_reply_of_the_summary_turn() {
    let history = and(
        started(),
        Vec::from([
            user(5),
            reply(6, "Built it."),
            done(7),
            user(8),
            reply(9, "Thinking…"),
            reply(10, "Built the protocol; the browser part is left."),
            done(11),
        ]),
    );
    assert_eq!(
        summary(&history, SESSION, HostSeq(7)),
        Some(text("Built the protocol; the browser part is left."))
    );
    assert_eq!(
        summary(&history[..7], SESSION, HostSeq(0)),
        Some(text("Built it.")),
        "a stopped agent's last reply stands in"
    );
    assert_eq!(summary(&started(), SESSION, HostSeq(0)), None);
    assert!(turn_ended(&done(11), SESSION));
    let accepted = Some(&proto::CommandResult::Accepted);
    assert!(super::waits(accepted, &[], SESSION));
    assert!(!super::waits(accepted, &[done(11)], SESSION));
    assert!(!super::waits(None, &[], SESSION));
    assert!(!turn_ended(&reply(10, "x"), SESSION));
}

#[test]
fn the_branch_lands_by_fast_forward_or_merge_and_never_under_a_checkout() {
    let branches = |ahead, fast_forward, checked_out: Option<&str>| Branches {
        ahead,
        fast_forward,
        checked_out: checked_out.map(text),
    };
    assert_eq!(
        [
            branches(0, true, None),
            branches(2, true, None),
            branches(2, false, None),
        ]
        .map(Branches::landing),
        [Landing::Nothing, Landing::FastForward, Landing::Merge]
    );
    assert!(matches!(
        branches(2, true, Some("/src")).landing(),
        Landing::Refuse(reason) if reason.contains("/src")
    ));
}

#[test]
fn finishing_says_what_landed_and_where_the_branch_went() {
    let pushed = finished(Ok(2), Some((text("file:///remote.git"), Ok(()))));
    assert_eq!(
        pushed,
        Finished::Landed {
            commits: 2,
            kept: Kept::Pushed {
                remote: text("file:///remote.git")
            }
        }
    );
    assert_eq!(landed(&pushed), 2);
    let failed = finished(Err(text("conflicts")), None);
    assert_eq!(
        failed,
        Finished::Failed {
            reason: text("conflicts")
        }
    );
    assert_eq!(landed(&failed), 0);
}

#[test]
fn a_branch_is_checked_out_where_a_worktree_has_it() {
    let listed = "worktree /src\nHEAD abc\nbranch refs/heads/main\n\n\
                  worktree /src-issue\nHEAD def\nbranch refs/heads/issue/x\n\n\
                  worktree /data/worktrees/1\nHEAD 123\ndetached\n";
    assert_eq!(
        checked_out(listed, "refs/heads/issue/x"),
        Some(text("/src-issue"))
    );
    assert_eq!(checked_out(listed, "refs/heads/issue/y"), None);
}

#[test]
fn a_session_finishes_once_until_it_does_more() {
    let finishing_now =
        and(started(), Vec::from([event(5, EventKind::FinishStarted)]));
    assert_eq!(
        finishing(&finishing_now, SESSION).map(|_| ()),
        Err(Rejection::AlreadyFinished)
    );
    let summed_up = and(
        finishing_now,
        Vec::from([
            user(6),
            reply(7, "summary"),
            done(8),
            event(
                9,
                EventKind::SessionFinished {
                    finished: Finished::Failed { reason: text("x") },
                },
            ),
        ]),
    );
    assert_eq!(
        finishing(&summed_up, SESSION).map(|_| ()),
        Err(Rejection::AlreadyFinished),
        "its own summary turn doesn't count as more work"
    );
    let more = and(
        summed_up,
        Vec::from([user(10), reply(11, "more"), done(12)]),
    );
    assert!(finishing(&more, SESSION).is_ok());
    let starting = started()
        .into_iter()
        .filter(|event| event.kind != EventKind::SessionReady)
        .collect::<Vec<Event>>();
    assert_eq!(
        finishing(&starting, SESSION).map(|it| it.running),
        Ok(true),
        "a session still starting counts as running"
    );
}

#[test]
fn a_merge_gives_its_tree_or_names_the_conflicts() {
    assert_eq!(super::merged(true, "abc\n", ""), Ok(text("abc")));
    assert_eq!(
        super::merged(false, "abc\na.txt\nb.txt\n", ""),
        Err(text(
            "the session's branch conflicts with the issue's in a.txt, b.txt; merge them by hand"
        ))
    );
    assert_eq!(
        super::merged(false, "", "fatal: bad revision"),
        Err(text("the branches couldn't be merged: fatal: bad revision"))
    );
    assert_eq!(
        super::interrupted(
            "asking for a summary",
            Some(&proto::CommandResult::Accepted)
        ),
        None
    );
    assert!(
        super::interrupted(
            "asking for a summary",
            Some(&proto::CommandResult::Rejected(Rejection::TurnInProgress))
        )
        .is_some_and(|reason| reason.starts_with("asking for a summary: "))
    );
}
