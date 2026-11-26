use alloc::collections::BTreeMap;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub session: SessionId,
    pub instance: InstanceId,
    pub directory: SessionDirectory,
    pub choice: Choice,
}

pub fn histories(
    history: Vec<Event>,
) -> impl Iterator<Item = (Created, Vec<Event>)> {
    let mut created = Vec::new();
    let mut sessions = BTreeMap::<SessionId, Vec<Event>>::new();
    for event in history {
        if let EventKind::SessionCreated {
            instance,
            directory,
            model,
            effort,
            ..
        } = &event.kind
        {
            created.push(Created {
                session: event.session,
                instance: instance.clone(),
                directory: directory.clone(),
                choice: Choice {
                    model: model.clone(),
                    effort: *effort,
                },
            });
        }
        sessions.entry(event.session).or_default().push(event);
    }
    created.into_iter().filter_map(move |session| {
        let events = sessions.remove(&session.session)?;
        Some((session, events))
    })
}

#[must_use]
pub fn events_of(session: SessionId, history: &[Event]) -> Vec<Event> {
    history
        .iter()
        .filter(|event| event.session == session)
        .cloned()
        .collect()
}

impl SnapshotPlan {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Append(EventKind),
    Barrier,
    Snapshot(SnapshotPlan),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    pub choice: Choice,
    pub talked: bool,
    pub steps: Vec<Step>,
}

#[must_use]
pub fn restarted() -> CloseReason {
    CloseReason::Stopped {
        reason: String::from("the host restarted"),
    }
}

impl Recovery {
    #[must_use]
    pub fn of(created: Choice, events: &[Event]) -> Self {
        Self {
            choice: replay_choice(created, events),
            talked: events.iter().any(|event| {
                matches!(event.kind, EventKind::UserMessage { .. })
            }),
            steps: recovery_steps(events),
        }
    }
}

fn recovery_steps(events: &[Event]) -> Vec<Step> {
    let mut steps = open_requests(events)
        .into_iter()
        .map(|request| {
            Step::Append(EventKind::RequestResolved {
                request,
                resolution: Resolution::Withdrawn,
            })
        })
        .collect::<Vec<_>>();
    if turn_running(events.iter()) {
        if let Some(turn) = Book::replay(events).plan_snapshot() {
            steps.extend([Step::Barrier, Step::Snapshot(turn)]);
        }
        steps.push(Step::Append(EventKind::TurnCompleted {
            outcome: TurnOutcome::Interrupted,
        }));
    }
    if agent_running(events) {
        steps.push(Step::Append(EventKind::SessionClosed {
            reason: restarted(),
        }));
    }
    if finish_interrupted(events) {
        steps.push(Step::Append(EventKind::FinishInterrupted));
    }
    steps
}

pub(crate) fn finish_interrupted(events: &[Event]) -> bool {
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

pub(crate) fn turn_running<'a>(
    events: impl DoubleEndedIterator<Item = &'a Event>,
) -> bool {
    events
        .rev()
        .find_map(|event| event.kind.turn_activity())
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
    use crate::workspace::{Snapshot, Workspace};
    use agents::{
        ClientName, Effort, ModelId, Provider, ToolCallId, ToolKind,
        TurnOutcome,
    };
    use vcs::{Repository, WorkingCopy, Worktree};

    use super::{Created, Recovery, SnapshotPlan, Step, histories};
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
        let sessions = histories(history).collect::<Vec<_>>();
        assert_eq!(
            sessions
                .iter()
                .map(|(created, _)| created.session)
                .collect::<Vec<_>>(),
            [SessionId(1), SessionId(2)]
        );
        assert_eq!(
            sessions.first().map(|(created, _)| created),
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
            Recovery::of(choice, &sessions[0].1),
            Recovery {
                choice: Choice {
                    model: None,
                    effort: Some(agents::Effort::High),
                },
                talked: true,
                steps: Vec::from([withdrawn(2), interrupted(), closed()]),
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
            attempt: vcs::command::SnapshotAttempt(String::from("turn-9")),
        }
    }

    fn withdrawn(request: u64) -> Step {
        Step::Append(EventKind::RequestResolved {
            request: RequestId(request),
            resolution: Resolution::Withdrawn,
        })
    }

    fn interrupted() -> Step {
        Step::Append(EventKind::TurnCompleted {
            outcome: TurnOutcome::Interrupted,
        })
    }

    fn closed() -> Step {
        Step::Append(EventKind::SessionClosed {
            reason: super::restarted(),
        })
    }

    #[test]
    fn a_restart_withdraws_snapshots_interrupts_and_closes_in_order() {
        let history = [
            event(1, 1, started()),
            event(
                2,
                1,
                EventKind::WorkspaceCreated {
                    workspace: turn().workspace,
                },
            ),
            event(3, 1, EventKind::FinishStarted),
            event(
                9,
                1,
                EventKind::UserMessage {
                    text: String::from("summarize"),
                    by: ClientName(String::from("phone")),
                },
            ),
            event(10, 1, opened(2)),
            event(11, 1, opened(3)),
        ];
        let choice = Choice {
            model: None,
            effort: None,
        };
        assert_eq!(
            Recovery::of(choice, &history).steps,
            [
                withdrawn(2),
                withdrawn(3),
                Step::Barrier,
                Step::Snapshot(turn()),
                interrupted(),
                closed(),
                Step::Append(EventKind::FinishInterrupted),
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

    #[test]
    fn a_restart_snapshots_a_running_turn_and_leaves_a_finished_one() {
        let user = EventKind::UserMessage {
            text: String::from("edit"),
            by: ClientName(String::from("phone")),
        };
        let mut history = Vec::from([
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
        let recovery = |steps| Recovery {
            choice: Choice {
                model: Some(ModelId(String::from("small"))),
                effort: Some(Effort::Low),
            },
            talked: true,
            steps,
        };
        let none = Choice {
            model: None,
            effort: None,
        };
        let active = recovery(Vec::from([
            Step::Barrier,
            Step::Snapshot(SnapshotPlan {
                turn_start: Some(HostSeq(6)),
                attempt: vcs::command::SnapshotAttempt(String::from("turn-6")),
                ..turn()
            }),
            interrupted(),
            closed(),
        ]));
        assert_eq!(Recovery::of(none.clone(), &history), active);
        history.push(event(
            7,
            1,
            EventKind::NativeTurnStarted {
                turn_start: Some(HostSeq(6)),
            },
        ));
        assert_eq!(Recovery::of(none.clone(), &history), active);
        history.push(event(
            8,
            1,
            EventKind::TurnCompleted {
                outcome: TurnOutcome::Completed,
            },
        ));
        assert_eq!(
            Recovery::of(none, &history),
            recovery(Vec::from([closed()]))
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
                steps: Vec::new(),
            }
        );
    }
}
