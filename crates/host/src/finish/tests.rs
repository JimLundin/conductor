use crate::finish::Finished;
use crate::log::{Event, HostSeq};
use crate::registry::InstanceId;
use crate::session::SessionDirectory;
use crate::session::{EventKind, SessionId, SessionRefusal, SessionResult};
use crate::workspace::Workspace;
use agents::Output;
use agents::{ClientName, CloseReason, Provider, TurnOutcome};
use home::IssueId;
use vcs::Kept;
use vcs::{Repository, WorkingCopy, Worktree};

use super::{
    Effect, Finish, Finishing, Input, finished, finishing, landed, summary,
    turn_ended,
};
use crate::assignment::Assignment;
use crate::session::Order;

const SESSION: SessionId = SessionId(1);
fn issue_id() -> Result<IssueId, &'static str> {
    IssueId::new("local", "00000009").ok_or("invalid fixture identity")
}

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
        repository: Repository(text("/src")),
        original: WorkingCopy(text("/src")),
        worktree: Worktree(text("/data/worktrees/1")),
        directory: SessionDirectory(text("/data/worktrees/1")),
        branch: text("conductor/1"),
    }
}

fn instance() -> InstanceId {
    InstanceId::own(Provider::new("claude"))
}

fn started() -> Result<Vec<Event>, &'static str> {
    Ok(Vec::from([
        event(
            1,
            EventKind::SessionCreated {
                instance: instance(),
                directory: SessionDirectory(text("/src")),
                by: ClientName(text("phone")),
                model: None,
                effort: None,
            },
        ),
        event(2, EventKind::OnIssue { issue: issue_id()? }),
        event(
            3,
            EventKind::WorkspaceCreated {
                workspace: workspace(),
            },
        ),
        event(4, EventKind::SessionReady),
    ]))
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
    event(
        seq,
        EventKind::Agent(Output::AssistantMessage { text: text(words) }),
    )
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
fn a_session_on_an_issue_finishes_between_turns() -> Result<(), &'static str> {
    assert_eq!(
        finishing(&started()?, SESSION),
        Ok(Finishing {
            retry: false,
            assignment: Assignment {
                issue: issue_id()?,
                instance: instance(),
            },
            workspace: workspace(),
            running: true,
        })
    );
    assert_eq!(
        finishing(&and(started()?, Vec::from([user(5)])), SESSION),
        Err(SessionRefusal::TurnInProgress)
    );
    let closed = and(
        started()?,
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
    let plain = started()?
        .into_iter()
        .filter(|event| !matches!(event.kind, EventKind::OnIssue { .. }))
        .collect::<Vec<Event>>();
    assert_eq!(finishing(&plain, SESSION), Err(SessionRefusal::NotOnIssue));
    assert_eq!(
        finishing(
            &and(
                started()?,
                Vec::from([event(5, EventKind::SessionRemoved { kept: None })])
            ),
            SESSION
        ),
        Err(SessionRefusal::SessionRemoved)
    );
    assert_eq!(finishing(&[], SESSION), Err(SessionRefusal::UnknownSession));
    Ok(())
}

#[test]
fn the_summary_is_the_last_reply_of_the_summary_turn()
-> Result<(), &'static str> {
    let history = and(
        started()?,
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
    assert_eq!(summary(&started()?, SESSION, HostSeq(0)), None);
    let other = Event {
        session: SessionId(2),
        ..reply(12, "Another session's reply.")
    };
    assert_eq!(
        summary(
            &and(history.clone(), Vec::from([other])),
            SESSION,
            HostSeq(7)
        ),
        Some(text("Built the protocol; the browser part is left.")),
        "another session's reply isn't the summary"
    );
    assert_eq!(
        summary(&history, SESSION, HostSeq(10)),
        None,
        "the reply the summary turn started after isn't the summary"
    );
    assert!(turn_ended(&done(11), SESSION));
    assert!(!turn_ended(&reply(10, "x"), SESSION));
    Ok(())
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
fn a_session_finishes_once_until_it_does_more() -> Result<(), &'static str> {
    let finishing_now =
        and(started()?, Vec::from([event(5, EventKind::FinishStarted)]));
    assert_eq!(
        finishing(&finishing_now, SESSION).map(|_| ()),
        Err(SessionRefusal::AlreadyFinished)
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
        Err(SessionRefusal::AlreadyFinished),
        "its own summary turn doesn't count as more work"
    );
    let more = and(
        summed_up,
        Vec::from([user(10), reply(11, "more"), done(12)]),
    );
    assert!(finishing(&more, SESSION).is_ok());
    let starting = started()?
        .into_iter()
        .filter(|event| event.kind != EventKind::SessionReady)
        .collect::<Vec<Event>>();
    assert_eq!(
        finishing(&starting, SESSION).map(|it| it.running),
        Ok(true),
        "a session still starting counts as running"
    );
    Ok(())
}

#[test]
fn rejected_summary_order_explains_what_was_interrupted() {
    assert_eq!(
        super::interrupted(
            "asking for a summary",
            Some(&crate::session::SessionResult::Accepted)
        ),
        None
    );
    assert!(
        super::interrupted(
            "asking for a summary",
            Some(&crate::session::SessionResult::Refused(
                SessionRefusal::TurnInProgress
            ))
        )
        .is_some_and(|reason| reason.starts_with("asking for a summary: "))
    );
}

fn client() -> ClientName {
    ClientName(text("phone"))
}

fn ask() -> Result<(Finish, Vec<Effect>), &'static str> {
    let (mut finish, first) = Finish::new(SESSION, client());
    assert_eq!(first, Effect::History);
    let effects = finish.step(Input::History(started()?));
    Ok((finish, effects))
}

fn watching() -> Result<Finish, &'static str> {
    let (mut finish, _) = ask()?;
    assert_eq!(
        finish.step(Input::Ordered(Some(SessionResult::Accepted))),
        [Effect::Watch {
            after: HostSeq(4),
            seconds: 600
        }]
    );
    Ok(finish)
}

fn branch() -> Result<String, &'static str> {
    Ok(home::issue::branch(None, &issue_id()?))
}

fn note(summary: Option<&str>, commits: u64) -> Result<Effect, &'static str> {
    Ok(Effect::Note {
        issue: issue_id()?,
        summary: summary.map(text),
        by: Assignment {
            issue: issue_id()?,
            instance: instance(),
        }
        .signer(),
        commits,
    })
}

fn last(finish: &mut Finish, finished: Finished) {
    assert_eq!(
        finish.step(Input::Noted),
        [Effect::Emit(EventKind::SessionFinished { finished })]
    );
}

fn failed(finish: &mut Finish, reason: &str) {
    last(
        finish,
        Finished::Failed {
            reason: text(reason),
        },
    );
}

#[test]
fn a_running_finish_records_acceptance_before_asking_and_recovers_a_lagged_watch()
-> Result<(), &'static str> {
    let (mut finish, effects) = ask()?;
    assert_eq!(
        effects,
        [
            Effect::Emit(EventKind::FinishStarted),
            Effect::Reply(SessionResult::Accepted),
            Effect::Order(Order::Send {
                text: text(super::SUMMARY_PROMPT),
                by: client()
            }),
        ]
    );
    assert!(matches!(
        &finish.step(Input::Ordered(Some(SessionResult::Accepted)))[..],
        [Effect::Watch { .. }]
    ));
    assert_eq!(
        finish.step(Input::RequestedAgain(and(
            started()?,
            Vec::from([event(5, EventKind::FinishStarted)])
        ))),
        [
            Effect::Reply(SessionResult::Refused(
                SessionRefusal::AlreadyFinished
            )),
            Effect::NextEvent,
        ]
    );
    assert_eq!(
        finish.step(Input::Events(Vec::from([reply(5, "old")]))),
        [Effect::NextEvent]
    );
    assert_eq!(finish.step(Input::Lagged), [Effect::Replay(HostSeq(4))]);
    assert_eq!(
        finish.step(Input::Events(Vec::from([
            reply(4, "before"),
            reply(5, "old"),
            reply(6, "summary"),
            done(7),
            reply(8, "after"),
        ]))),
        [Effect::Order(Order::Stop(text(super::FINISHED_REASON)))]
    );
    assert_eq!(
        finish.step(Input::Ordered(Some(SessionResult::Accepted))),
        [Effect::ReadHome]
    );
    assert_eq!(
        finish.step(Input::Home(None)),
        [Effect::Ready {
            issue: issue_id()?,
            workspace: workspace(),
            branch: branch()?
        }]
    );
    assert_eq!(
        finish.step(Input::Ready(Ok(()))),
        [Effect::Land {
            workspace: workspace(),
            branch: branch()?
        }]
    );
    assert_eq!(
        finish.step(Input::Landed(Ok(3))),
        [Effect::Push {
            workspace: workspace(),
            branch: branch()?
        }]
    );
    assert_eq!(
        finish.step(Input::Pushed(Some((text("origin"), Ok(()))))),
        [note(Some("summary"), 3)?]
    );
    last(
        &mut finish,
        Finished::Landed {
            commits: 3,
            kept: Kept::Pushed {
                remote: text("origin"),
            },
        },
    );
    Ok(())
}

#[test]
fn an_active_turn_is_rejected_before_logging_or_asking()
-> Result<(), &'static str> {
    let (mut finish, _) = Finish::new(SESSION, client());
    assert_eq!(
        finish.step(Input::History(and(started()?, Vec::from([user(5)])))),
        [Effect::Reply(SessionResult::Refused(
            SessionRefusal::TurnInProgress
        ))]
    );
    Ok(())
}

#[test]
fn a_finish_that_cannot_ask_or_stop_only_notes_the_failure()
-> Result<(), &'static str> {
    let (mut finish, _) = ask()?;
    assert_eq!(finish.step(Input::Ordered(None)), [note(None, 0)?]);
    failed(&mut finish, "asking for a summary: the session stopped");

    let mut finish = watching()?;
    let closed = event(
        6,
        EventKind::SessionClosed {
            reason: CloseReason::Exited { status: text("0") },
        },
    );
    assert_eq!(
        finish.step(Input::Events(Vec::from([reply(5, "summary"), closed]))),
        [Effect::Order(Order::Stop(text(super::FINISHED_REASON)))]
    );
    let refusal = SessionRefusal::TurnInProgress;
    let reason = format!("stopping the agent: {}", refusal.label());
    assert_eq!(
        finish.step(Input::Ordered(Some(SessionResult::Refused(refusal)))),
        [note(None, 0)?]
    );
    failed(&mut finish, &reason);
    Ok(())
}

#[test]
fn a_summary_timeout_interrupts_and_skips_landing_even_if_the_interrupt_is_refused()
-> Result<(), &'static str> {
    let mut finish = watching()?;
    assert_eq!(
        finish.step(Input::TimedOut),
        [Effect::Order(Order::Interrupt)]
    );
    assert_eq!(
        finish.step(Input::Ordered(Some(SessionResult::Refused(
            SessionRefusal::NoTurnRunning
        )))),
        [note(None, 0)?]
    );
    failed(&mut finish, super::IMPATIENT);
    Ok(())
}

fn closed_finish() -> Result<Finish, &'static str> {
    let (mut finish, _) = Finish::new(SESSION, client());
    let closed = event(
        8,
        EventKind::SessionClosed {
            reason: CloseReason::Exited { status: text("0") },
        },
    );
    assert_eq!(
        finish.step(Input::History(and(
            started()?,
            Vec::from([user(5), reply(6, "old summary"), done(7), closed]),
        ))),
        [
            Effect::Emit(EventKind::FinishStarted),
            Effect::Reply(SessionResult::Accepted),
            Effect::ReadHome,
        ]
    );
    let _ = finish.step(Input::Home(None));
    Ok(finish)
}

#[test]
fn readiness_and_landing_failures_keep_the_closed_agents_summary_and_skip_push()
-> Result<(), &'static str> {
    let mut finish = closed_finish()?;
    assert_eq!(
        finish.step(Input::Ready(Err(text("diverged")))),
        [note(Some("old summary"), 0)?]
    );
    failed(&mut finish, "diverged");

    let mut finish = closed_finish()?;
    let _ = finish.step(Input::Ready(Ok(())));
    assert_eq!(
        finish.step(Input::Landed(Err(text("conflict")))),
        [note(Some("old summary"), 0)?]
    );
    failed(&mut finish, "conflict");
    Ok(())
}

#[test]
fn a_failed_push_still_notes_the_commits_that_landed()
-> Result<(), &'static str> {
    let mut finish = closed_finish()?;
    let _ = finish.step(Input::Ready(Ok(())));
    let _ = finish.step(Input::Landed(Ok(2)));
    assert_eq!(
        finish
            .step(Input::Pushed(Some((text("origin"), Err(text("offline")))))),
        [note(Some("old summary"), 2)?]
    );
    last(
        &mut finish,
        Finished::Landed {
            commits: 2,
            kept: Kept::PushFailed {
                reason: text("offline"),
            },
        },
    );
    Ok(())
}

#[test]
fn a_restarted_finish_reuses_its_pending_attempt_and_summary()
-> Result<(), &'static str> {
    let history = and(
        started()?,
        Vec::from([
            event(5, EventKind::FinishStarted),
            user(6),
            reply(7, "saved summary"),
            done(8),
            event(
                9,
                EventKind::SessionClosed {
                    reason: crate::restore::restarted(),
                },
            ),
        ]),
    );
    assert!(crate::restore::finish_interrupted(&history));
    let interrupted = and(
        history,
        Vec::from([event(10, EventKind::FinishInterrupted)]),
    );
    let (mut finish, _) = Finish::new(SESSION, client());
    assert_eq!(
        finish.step(Input::History(interrupted)),
        [Effect::Reply(SessionResult::Accepted), Effect::ReadHome]
    );
    let _ = finish.step(Input::Home(None));
    let _ = finish.step(Input::Ready(Ok(())));
    let _ = finish.step(Input::Landed(Ok(2)));
    assert_eq!(
        finish.step(Input::Pushed(None)),
        [note(Some("saved summary"), 2)?]
    );
    Ok(())
}
