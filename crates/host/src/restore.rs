//! What a restart does to each session the log holds.
//!
//! Before the host serves the session again, requests nobody answered are
//! withdrawn, a turn that was running is interrupted, after a checkpoint of
//! its files when it has a workspace, and an agent that was running is
//! closed.
//!
//! `daemon` performs each [`Step`]: it appends the events and takes the
//! snapshot.

use alloc::format;

use proto::{
    Checkpoint, CheckpointId, CloseReason, Event, EventKind, HistoryAnchor,
    HostPath, HostSeq, InstanceId, RequestId, Resolution, SessionId,
    TurnOutcome, UndoAvailability, Workspace,
};

use crate::checkpoints::Book;
use crate::prelude::*;
use crate::session::Choice;

/// A session the log says was created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub session: SessionId,
    pub instance: InstanceId,
    pub directory: HostPath,
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

/// The checkpoint an interrupted turn leaves: its files, snapshotted into
/// `reserved` on top of `base`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interrupted {
    pub workspace: Workspace,
    pub reserved: CheckpointId,
    /// The commit of the session's last checkpoint.
    pub base: Option<String>,
    parent: Option<CheckpointId>,
    turn_start: Option<HostSeq>,
    history: HistoryAnchor,
    undo: UndoAvailability,
}

impl Interrupted {
    /// The checkpoint, once the snapshot made `commit`.
    #[must_use]
    pub fn checkpoint(self, commit: String) -> Checkpoint {
        Checkpoint {
            id: self.reserved,
            parent: self.parent,
            turn_start: self.turn_start,
            commit,
            history: self.history,
            undo: self.undo,
        }
    }

    /// What the session records once the snapshot made a commit, or why
    /// it did not.
    #[must_use]
    pub fn snapshotted(self, commit: Result<String, String>) -> EventKind {
        match commit {
            Ok(made) => EventKind::CheckpointCreated {
                checkpoint: self.checkpoint(made),
            },
            Err(err) => EventKind::Error {
                message: format!("interrupted turn checkpoint failed: {err}"),
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
    /// [`Interrupted::snapshotted`] says of it.
    Snapshot(Interrupted),
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
    /// The running turn's checkpoint, when it has a workspace and no undo
    /// is being recovered.
    pub checkpoint: Option<Interrupted>,
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
            checkpoint: interrupted.then(|| checkpoint(events)).flatten(),
            closed: agent_running(events),
        }
    }

    /// What the restart does, in order: withdraw the open requests,
    /// checkpoint and interrupt the running turn, and close the agent.
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
            if let Some(turn) = &self.checkpoint {
                steps.extend([
                    Step::Append(EventKind::CheckpointReserved {
                        checkpoint: turn.reserved,
                    }),
                    Step::Barrier,
                    Step::Snapshot(turn.clone()),
                ]);
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
        .filter_map(|event| match &event.kind {
            EventKind::RequestOpened { request, .. }
            | EventKind::QuestionAsked { request, .. } => Some(*request),
            _ => None,
        })
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
        .find_map(|event| match &event.kind {
            EventKind::SessionClosed { .. } => Some(false),
            EventKind::SessionCreated { .. } | EventKind::SessionReady => {
                Some(true)
            }
            _ => None,
        })
        .unwrap_or(false)
}

fn checkpoint(events: &[Event]) -> Option<Interrupted> {
    let book = Book::replay(events);
    match (
        book.snapshot.as_ref(),
        book.workspace.as_ref(),
        &book.recovery,
    ) {
        (Some(snapshot), Some(workspace), None) => Some(Interrupted {
            workspace: workspace.clone(),
            reserved: book.highwater.next(),
            base: book.last().map(|last| last.commit.clone()),
            parent: book.last().map(|last| last.id),
            turn_start: snapshot.turn_start,
            undo: book.availability(&snapshot.history),
            history: snapshot.history.clone(),
        }),
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
    use proto::{
        Checkpoint, CheckpointId, ClientName, Event, EventKind, HistoryAnchor,
        HostPath, HostSeq, InstanceId, Provider, RequestId, Resolution,
        SessionId, ToolCallId, ToolKind, TurnOutcome, UndoAvailability,
        Workspace,
    };

    use super::{Created, Interrupted, Recovery, Step, created, events_of};
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
            instance: InstanceId::named("codex-api"),
            provider: Provider::Codex,
            directory: HostPath(String::from("/work")),
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
                    effort: proto::Effort::High,
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
                instance: InstanceId::named("codex-api"),
                directory: HostPath(String::from("/work")),
                choice: choice.clone(),
            })
        );
        assert_eq!(
            Recovery::of(choice, &events_of(SessionId(1), &history)),
            Recovery {
                choice: Choice {
                    model: None,
                    effort: Some(proto::Effort::High),
                },
                talked: true,
                withdrawn: Vec::from([RequestId(2)]),
                interrupted: true,
                checkpoint: None,
                closed: true,
            }
        );
    }

    fn turn() -> Interrupted {
        let path = HostPath(String::from("/work"));
        Interrupted {
            workspace: Workspace {
                repository: path.clone(),
                original: path.clone(),
                worktree: path.clone(),
                directory: path,
                branch: String::from("conductor/1"),
            },
            reserved: CheckpointId(3),
            base: Some(String::from("base")),
            parent: Some(CheckpointId(2)),
            turn_start: Some(HostSeq(9)),
            history: HistoryAnchor::Fresh,
            undo: UndoAvailability::Unavailable {
                reason: String::from("test"),
            },
        }
    }

    fn withdrawn(request: u64) -> Step {
        Step::Append(EventKind::RequestResolved {
            request: RequestId(request),
            resolution: Resolution::Withdrawn,
        })
    }

    #[test]
    fn a_restart_withdraws_checkpoints_interrupts_and_closes_in_order() {
        let recovery = |checkpoint, closed| Recovery {
            choice: Choice {
                model: None,
                effort: None,
            },
            talked: true,
            withdrawn: Vec::from([RequestId(2), RequestId(3)]),
            interrupted: true,
            checkpoint,
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
                    Step::Append(EventKind::CheckpointReserved {
                        checkpoint: CheckpointId(3),
                    }),
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
    fn a_snapshot_records_its_checkpoint_or_why_it_failed() {
        assert_eq!(
            [
                turn().snapshotted(Ok(String::from("made"))),
                turn().snapshotted(Err(String::from("git broke"))),
            ],
            [
                EventKind::CheckpointCreated {
                    checkpoint: Checkpoint {
                        id: CheckpointId(3),
                        parent: Some(CheckpointId(2)),
                        turn_start: Some(HostSeq(9)),
                        commit: String::from("made"),
                        history: HistoryAnchor::Fresh,
                        undo: UndoAvailability::Unavailable {
                            reason: String::from("test"),
                        },
                    },
                },
                EventKind::Error {
                    message: String::from(
                        "interrupted turn checkpoint failed: git broke"
                    ),
                },
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
                checkpoint: None,
                closed: false,
            }
        );
        assert_eq!(Recovery::of(choice, &events).steps(), Vec::new());
    }
}
