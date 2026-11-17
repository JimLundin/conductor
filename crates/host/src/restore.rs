//! What a restart does to each session the log holds.
//!
//! Before the host serves the session again, requests nobody answered are
//! withdrawn, a turn that was running is interrupted, after a checkpoint of
//! its files when it has a workspace, and an agent that was running is
//! closed.
//!
//! `daemon` appends the events and takes the snapshot.

use proto::{
    Checkpoint, CheckpointId, CloseReason, Event, EventKind, HistoryAnchor,
    HostPath, HostSeq, Provider, RequestId, SessionId, UndoAvailability,
    Workspace,
};

use crate::checkpoints::Book;
use crate::prelude::*;
use crate::session::Choice;

/// A session the log says was created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub session: SessionId,
    pub provider: Provider,
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
                provider,
                directory,
                model,
                effort,
                ..
            } => Some(Created {
                session: event.session,
                provider: *provider,
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
        ClientName, Event, EventKind, HostPath, HostSeq, Provider, RequestId,
        Resolution, SessionId, ToolCallId, ToolKind,
    };

    use super::{Created, Recovery, created, events_of};
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
                provider: Provider::Codex,
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
                choice,
                talked: false,
                withdrawn: Vec::new(),
                interrupted: false,
                checkpoint: None,
                closed: false,
            }
        );
    }
}
