//! What a restart does to each session the log holds.
//!
//! Before the host serves the session again, requests nobody answered are
//! withdrawn, a turn that was running is interrupted, after a snapshot of
//! its files when it has a workspace, and an agent that was running is
//! closed.
//!
//! `daemon` performs each [`Step`]: it appends the events and takes the
//! snapshot.

use alloc::format;

use crate::log::Event;
use crate::registry::InstanceId;
use crate::session::SessionDirectory;
use crate::session::{EventKind, RequestId, Resolution, SessionId};
use agents::{CloseReason, TurnOutcome};

use crate::prelude::*;
use crate::session::Choice;
use crate::snapshots::Book;
pub use crate::snapshots::SnapshotPlan;

/// A session the log says was created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub session: SessionId,
    pub instance: InstanceId,
    pub directory: SessionDirectory,
    pub choice: Choice,
}

/// Every session the log holds, in the order they were created.
#[must_use]
pub fn created(history: &[Event]) -> Vec<Created> {
    history
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::SessionCreated {
                instance,
                directory,
                model,
                effort,
                ..
            } => Some(Created {
                session: event.session,
                instance: instance.clone(),
                directory: directory.clone(),
                choice: Choice {
                    model: model.clone(),
                    effort: *effort,
                },
            }),
            _ => None,
        })
        .collect()
}

/// One session's events.
#[must_use]
pub fn events_of(session: SessionId, history: &[Event]) -> Vec<Event> {
    history
        .iter()
        .filter(|event| event.session == session)
        .cloned()
        .collect()
}

impl SnapshotPlan {
    /// What the session records once the snapshot made a commit, or why
    /// it did not.
    #[must_use]
    pub fn snapshotted(self, commit: Result<String, String>) -> EventKind {
        match commit {
            Ok(made) => EventKind::SnapshotRecorded {
                snapshot: self.snapshot(made),
            },
            Err(err) => EventKind::Error {
                message: format!("interrupted turn snapshot failed: {err}"),
            },
        }
    }
}

/// One thing a restart does to a session, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Append(EventKind),
    /// Commit what the log has staged before the snapshot runs.
    Barrier,
    /// Snapshot the turn's files, then append what
    /// [`SnapshotPlan::snapshotted`] says of it.
    Snapshot(SnapshotPlan),
}

/// What a restart does to a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    /// The model and effort it runs with now.
    pub choice: Choice,
    /// A turn started in it, so its conversation was saved.
    pub talked: bool,
    pub withdrawn: Vec<RequestId>,
    /// A turn was running.
    pub interrupted: bool,
    /// Pending work to capture before the session can continue.
    pub snapshot: Option<SnapshotPlan>,
    /// The agent was running.
    pub closed: bool,
}

/// Why a restart closes a running agent.
#[must_use]
pub fn restarted() -> CloseReason {
    CloseReason::Stopped {
        reason: String::from("the host restarted"),
    }
}

impl Recovery {
    /// What a restart does to the session `created` began, given its
    /// `events`.
    #[must_use]
    pub fn of(created: Choice, events: &[Event]) -> Self {
        let interrupted = turn_running(events);
        Self {
            choice: replay_choice(created, events),
            talked: events.iter().any(|event| {
                matches!(event.kind, EventKind::UserMessage { .. })
            }),
            withdrawn: open_requests(events),
            interrupted,
            snapshot: interrupted
                .then(|| Book::replay(events).plan_snapshot())
                .flatten(),
            closed: agent_running(events),
        }
    }

    /// What the restart does, in order: withdraw the open requests,
    /// snapshot and interrupt the running turn, and close the agent.
    #[must_use]
    pub fn steps(&self) -> Vec<Step> {
        let mut steps = self
            .withdrawn
            .iter()
            .map(|request| {
                Step::Append(EventKind::RequestResolved {
                    request: *request,
                    resolution: Resolution::Withdrawn,
                })
            })
            .collect::<Vec<Step>>();
        if self.interrupted {
            if let Some(turn) = &self.snapshot {
                steps.extend([Step::Barrier, Step::Snapshot(turn.clone())]);
            }
            steps.push(Step::Append(EventKind::TurnCompleted {
                outcome: TurnOutcome::Interrupted,
            }));
        }
        if self.closed {
            steps.push(Step::Append(EventKind::SessionClosed {
                reason: restarted(),
            }));
        }
        steps
    }
}

/// Whether a finish lost its actor before recording its outcome.
#[must_use]
pub fn finish_interrupted(events: &[Event]) -> bool {
    events
        .iter()
        .fold(crate::finish::FinishState::Ready, |state, event| {
            state.after(&event.kind)
        })
        == crate::finish::FinishState::Finishing
}

fn open_requests(events: &[Event]) -> Vec<RequestId> {
    let resolved = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::RequestResolved { request, .. } => Some(*request),
            _ => None,
        })
        .collect::<Vec<RequestId>>();
    events
        .iter()
        .filter_map(|event| event.kind.opened_request())
        .filter(|request| !resolved.contains(request))
        .collect()
}

fn turn_running(events: &[Event]) -> bool {
    events
        .iter()
        .rev()
        .find_map(|event| match &event.kind {
            EventKind::UserMessage { .. }
            | EventKind::NativeTurnStarted { .. } => Some(true),
            EventKind::TurnCompleted { .. }
            | EventKind::SessionClosed { .. } => Some(false),
            _ => None,
        })
        .unwrap_or(false)
}

fn agent_running(events: &[Event]) -> bool {
    events
        .iter()
        .rev()
        .find_map(|event| agent_activity(&event.kind))
        .unwrap_or(false)
}

pub(crate) const fn agent_activity(kind: &EventKind) -> Option<bool> {
    match kind {
        EventKind::SessionClosed { .. } | EventKind::SessionRemoved { .. } => {
            Some(false)
        }
        EventKind::SessionCreated { .. } | EventKind::SessionReady => {
            Some(true)
        }
        _ => None,
    }
}

fn replay_choice(created: Choice, events: &[Event]) -> Choice {
    events
        .iter()
        .fold(created, |current, event| match &event.kind {
            EventKind::ModelChanged { model, effort, .. } => Choice {
                model: model.clone(),
                effort: *effort,
            },
            EventKind::EffortChanged { effort } => Choice {
                effort: Some(*effort),
                ..current
            },
            _ => current,
        })
}

#[cfg(test)]
mod tests {
    use crate::log::{Event, HostSeq};
    use crate::registry::InstanceId;
    use crate::session::SessionDirectory;
    use crate::session::{EventKind, RequestId, Resolution, SessionId};
    use crate::workspace::{
        Repository, Snapshot, WorkingCopy, Workspace, Worktree,
    };
    use agents::{
        ClientName, Effort, ModelId, Provider, ToolCallId, ToolKind,
        TurnOutcome,
    };

    use super::{Created, Recovery, SnapshotPlan, Step, created, events_of};
    use crate::session::Choice;

    fn event(seq: u64, session: u128, kind: EventKind) -> Event {
        Event {
            seq: HostSeq(seq),
            session: SessionId(session),
            kind,
        }
    }

    fn opened(request: u64) -> EventKind {
        EventKind::RequestOpened {
            request: RequestId(request),
            call: ToolCallId(String::from("tool")),
            tool: ToolKind::Other {
                name: String::from("test"),
                detail: String::new(),
            },
            reason: None,
            for_session: None,
        }
    }

    fn started() -> EventKind {
        EventKind::SessionCreated {
            instance: InstanceId::named(
                Provider::new("codex"),
                "https://gw.example",
            ),
            directory: SessionDirectory(String::from("/work")),
            by: ClientName(String::from("phone")),
            model: None,
            effort: None,
        }
    }

    #[test]
    fn a_session_mid_turn_withdraws_open_requests_and_is_interrupted_and_closed()
     {
        let history = Vec::from([
            event(1, 1, started()),
            event(2, 2, started()),
            event(3, 1, EventKind::SessionReady),
            event(
                4,
                1,
                EventKind::UserMessage {
                    text: String::from("hi"),
                    by: ClientName(String::from("phone")),
                },
            ),
            event(5, 1, opened(1)),
            event(6, 1, opened(2)),
            event(
                7,
                1,
                EventKind::RequestResolved {
                    request: RequestId(1),
                    resolution: Resolution::Withdrawn,
                },
            ),
            event(
                8,
                1,
                EventKind::EffortChanged {
                    effort: agents::Effort::High,
                },
            ),
        ]);
        let choice = Choice {
            model: None,
            effort: None,
        };
        assert_eq!(
            created(&history)
                .into_iter()
                .map(|created| created.session)
                .collect::<Vec<_>>(),
            [SessionId(1), SessionId(2)]
        );
        assert_eq!(
            created(&history).first(),
            Some(&Created {
                session: SessionId(1),
                instance: InstanceId::named(
                    Provider::new("codex"),
                    "https://gw.example"
                ),
                directory: SessionDirectory(String::from("/work")),
                choice: choice.clone(),
            })
        );
        assert_eq!(
            Recovery::of(choice, &events_of(SessionId(1), &history)),
            Recovery {
                choice: Choice {
                    model: None,
                    effort: Some(agents::Effort::High),
                },
                talked: true,
                withdrawn: Vec::from([RequestId(2)]),
                interrupted: true,
                snapshot: None,
                closed: true,
            }
        );
    }

    fn turn() -> SnapshotPlan {
        let path = String::from("/work");
        SnapshotPlan {
            workspace: Workspace {
                repository: Repository(path.clone()),
                original: WorkingCopy(path.clone()),
                worktree: Worktree(path.clone()),
                directory: SessionDirectory(path),
                branch: String::from("conductor/1"),
            },
            turn_start: Some(HostSeq(9)),
        }
    }

    fn withdrawn(request: u64) -> Step {
        Step::Append(EventKind::RequestResolved {
            request: RequestId(request),
            resolution: Resolution::Withdrawn,
        })
    }

    #[test]
    fn a_restart_withdraws_snapshots_interrupts_and_closes_in_order() {
        let recovery = |snapshot, closed| Recovery {
            choice: Choice {
                model: None,
                effort: None,
            },
            talked: true,
            withdrawn: Vec::from([RequestId(2), RequestId(3)]),
            interrupted: true,
            snapshot,
            closed,
        };
        let interrupted = Step::Append(EventKind::TurnCompleted {
            outcome: TurnOutcome::Interrupted,
        });
        assert_eq!(
            [
                recovery(Some(turn()), true).steps(),
                recovery(None, false).steps()
            ],
            [
                Vec::from([
                    withdrawn(2),
                    withdrawn(3),
                    Step::Barrier,
                    Step::Snapshot(turn()),
                    interrupted.clone(),
                    Step::Append(EventKind::SessionClosed {
                        reason: super::restarted(),
                    }),
                ]),
                Vec::from([withdrawn(2), withdrawn(3), interrupted]),
            ]
        );
    }

    #[test]
    fn a_capture_records_its_snapshot_or_why_it_failed() {
        assert_eq!(
            [
                turn().snapshotted(Ok(String::from("made"))),
                turn().snapshotted(Err(String::from("git broke"))),
            ],
            [
                EventKind::SnapshotRecorded {
                    snapshot: Snapshot {
                        turn_start: Some(HostSeq(9)),
                        commit: String::from("made"),
                    },
                },
                EventKind::Error {
                    message: String::from(
                        "interrupted turn snapshot failed: git broke"
                    ),
                },
            ]
        );
    }

    /// A running turn's files are snapshotted on top of the last
    /// snapshot, at the model the log last chose; a finished turn's
    /// are not.
    #[test]
    fn a_restart_snapshots_a_running_turn_and_leaves_a_finished_one() {
        let user = EventKind::UserMessage {
            text: String::from("edit"),
            by: ClientName(String::from("phone")),
        };
        let running = Vec::from([
            event(1, 1, started()),
            event(
                2,
                1,
                EventKind::WorkspaceCreated {
                    workspace: turn().workspace,
                },
            ),
            event(
                3,
                1,
                EventKind::SnapshotRecorded {
                    snapshot: Snapshot {
                        turn_start: None,
                        commit: String::from("c0"),
                    },
                },
            ),
            event(4, 1, EventKind::SessionReady),
            event(
                5,
                1,
                EventKind::ModelChanged {
                    model: Some(ModelId(String::from("small"))),
                    effort: Some(Effort::Low),
                    reason: String::from("not offered"),
                },
            ),
            event(6, 1, user),
        ]);
        let finished = running
            .iter()
            .cloned()
            .chain([event(
                7,
                1,
                EventKind::TurnCompleted {
                    outcome: TurnOutcome::Completed,
                },
            )])
            .collect::<Vec<Event>>();
        let recovery = |snapshot: Option<SnapshotPlan>| Recovery {
            choice: Choice {
                model: Some(ModelId(String::from("small"))),
                effort: Some(Effort::Low),
            },
            talked: true,
            withdrawn: Vec::new(),
            interrupted: snapshot.is_some(),
            snapshot,
            closed: true,
        };
        let none = Choice {
            model: None,
            effort: None,
        };
        assert_eq!(
            [
                Recovery::of(none.clone(), &running),
                Recovery::of(none, &finished),
            ],
            [
                recovery(Some(SnapshotPlan {
                    turn_start: Some(HostSeq(6)),
                    ..turn()
                })),
                recovery(None),
            ]
        );
    }

    #[test]
    fn a_closed_idle_session_needs_nothing() {
        let events = Vec::from([
            event(1, 1, started()),
            event(
                2,
                1,
                EventKind::SessionClosed {
                    reason: super::restarted(),
                },
            ),
        ]);
        let choice = Choice {
            model: None,
            effort: None,
        };
        assert_eq!(
            Recovery::of(choice.clone(), &events),
            Recovery {
                choice: choice.clone(),
                talked: false,
                withdrawn: Vec::new(),
                interrupted: false,
                snapshot: None,
                closed: false,
            }
        );
        assert_eq!(Recovery::of(choice, &events).steps(), Vec::new());
    }
}
