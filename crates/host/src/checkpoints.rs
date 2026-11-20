//! Replayable workspace checkpoints and the durable undo phases.
use crate::prelude::*;
use alloc::collections::BTreeMap;
use proto::{
    Checkpoint, CheckpointId, Event, EventKind, HistoryAnchor, HostSeq,
    UndoAvailability, Workspace,
};

#[derive(Debug, Clone)]
pub struct Undo {
    pub selected: CheckpointId,
    pub restored: CheckpointId,
    pub history: HistoryAnchor,
}

/// The pending undo, as planned, and its phase.
#[derive(Debug, Clone)]
pub enum Recovery {
    Native(Undo),
    /// The native history is restored, to [`Book::resume`].
    Files(Undo),
}

#[derive(Debug, Clone)]
struct PendingTurn {
    history: HistoryAnchor,
    turn_start: Option<HostSeq>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotPlan {
    pub workspace: Workspace,
    pub reserved: CheckpointId,
    pub base: Option<String>,
    pub(crate) parent: Option<CheckpointId>,
    pub(crate) turn_start: Option<HostSeq>,
    pub(crate) history: HistoryAnchor,
    pub(crate) undo: UndoAvailability,
}

impl SnapshotPlan {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compared {
    pub workspace: Workspace,
    pub checkpoint: Checkpoint,
    pub parent: Checkpoint,
}

#[derive(Default)]
pub struct Book {
    pub workspace: Option<Workspace>,
    pub checkpoints: BTreeMap<CheckpointId, Checkpoint>,
    pub active: Vec<CheckpointId>,
    highwater: u64,
    pub recovery: Option<Recovery>,
    pub resume: Option<HistoryAnchor>,
    snapshot: Option<PendingTurn>,
}

impl Book {
    #[must_use]
    pub fn replay(events: &[Event]) -> Self {
        let mut book = Self::default();
        for event in events {
            book.apply(event);
        }
        book
    }

    pub fn apply(&mut self, event: &Event) {
        match &event.kind {
            EventKind::UserMessage { .. } => self.accept_message(event.seq),
            kind => self.apply_kind(kind),
        }
    }

    pub fn accept_message(&mut self, seq: HostSeq) {
        self.snapshot = Some(PendingTurn {
            history: HistoryAnchor::Unknown,
            turn_start: Some(seq),
        });
    }

    pub fn apply_kind(&mut self, kind: &EventKind) {
        match kind {
            EventKind::WorkspaceCreated { workspace } => {
                self.workspace = Some(workspace.clone());
            }
            EventKind::NativeTurnStarted {
                history,
                turn_start,
            } => {
                self.snapshot = Some(PendingTurn {
                    history: history.clone(),
                    turn_start: *turn_start,
                });
                if matches!(self.resume, None | Some(HistoryAnchor::Fresh)) {
                    self.resume = Some(history.clone());
                }
            }
            EventKind::HistoryMaterialized { history } => {
                if let Some(snapshot) = &mut self.snapshot {
                    snapshot.history = history.clone();
                }
                self.resume = Some(history.clone());
            }
            EventKind::CheckpointReserved { checkpoint } => {
                self.highwater = self.highwater.max(checkpoint.0);
            }
            EventKind::CheckpointCreated { checkpoint } => {
                self.highwater = self.highwater.max(checkpoint.id.0);
                self.active.push(checkpoint.id);
                self.checkpoints.insert(checkpoint.id, checkpoint.clone());
                self.snapshot = None;
            }
            EventKind::UndoStarted {
                checkpoint,
                restore,
                history,
            } => {
                self.recovery = Some(Recovery::Native(Undo {
                    selected: *checkpoint,
                    restored: *restore,
                    history: history.clone(),
                }));
            }
            EventKind::UndoHistoryRestored { history, .. } => {
                self.resume = Some(history.clone());
                self.recovery = self.recovery.take().map(|phase| match phase {
                    Recovery::Native(undo) | Recovery::Files(undo) => {
                        Recovery::Files(undo)
                    }
                });
            }
            EventKind::TurnsUndone { restored, .. } => {
                self.active.retain(|id| id <= restored);
                self.snapshot = None;
                self.recovery = None;
            }
            _ => {}
        }
    }

    #[must_use]
    pub const fn next_reservation(&self) -> CheckpointId {
        CheckpointId(self.highwater).next()
    }

    #[must_use]
    pub fn plan_snapshot(&self) -> Option<SnapshotPlan> {
        if self.recovery.is_some() {
            return None;
        }
        let workspace = self.workspace.as_ref()?;
        let snapshot = self.snapshot.as_ref()?;
        let previous = self.last();
        Some(SnapshotPlan {
            workspace: workspace.clone(),
            reserved: self.next_reservation(),
            base: previous.map(|parent| parent.commit.clone()),
            parent: previous.map(|parent| parent.id),
            turn_start: snapshot.turn_start,
            history: snapshot.history.clone(),
            undo: self.availability(&snapshot.history),
        })
    }

    #[must_use]
    pub fn diff(&self, id: CheckpointId) -> Option<Compared> {
        let workspace = self.workspace.as_ref()?;
        let checkpoint = self.checkpoints.get(&id)?;
        let parent = self.checkpoints.get(&checkpoint.parent?)?;
        Some(Compared {
            workspace: workspace.clone(),
            checkpoint: checkpoint.clone(),
            parent: parent.clone(),
        })
    }

    #[must_use]
    fn availability(&self, history: &HistoryAnchor) -> UndoAvailability {
        match history {
            HistoryAnchor::Turns { .. } => match self
                .active
                .iter()
                .filter_map(|id| self.checkpoints.get(id))
                .find(|checkpoint| {
                    matches!(checkpoint.history, HistoryAnchor::Unknown)
                }) {
                Some(_) => UndoAvailability::Unavailable {
                    reason: String::from(
                        "retained native turn prefix contains an unobserved boundary",
                    ),
                },
                None => availability(self.last(), history),
            },
            _ => availability(self.last(), history),
        }
    }

    #[must_use]
    pub fn last(&self) -> Option<&Checkpoint> {
        self.active.last().and_then(|id| self.checkpoints.get(id))
    }
}

#[must_use]
fn availability(
    parent: Option<&Checkpoint>,
    history: &HistoryAnchor,
) -> UndoAvailability {
    match (parent, history) {
        (_, HistoryAnchor::Unknown)
        | (
            Some(Checkpoint {
                history: HistoryAnchor::Unknown,
                ..
            }),
            _,
        ) => UndoAvailability::Unavailable {
            reason: String::from("native turn boundary was not observed"),
        },
        (
            Some(Checkpoint {
                history: HistoryAnchor::Fresh,
                ..
            }),
            _,
        )
        | (
            Some(Checkpoint {
                history:
                    HistoryAnchor::Messages {
                        message: Some(_), ..
                    },
                ..
            }),
            HistoryAnchor::Messages { .. },
        )
        | (Some(_), HistoryAnchor::Turns { turn: Some(_), .. }) => {
            UndoAvailability::Available
        }
        _ => UndoAvailability::Unavailable {
            reason: String::from("native history has no safe turn boundary"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{Book, Recovery};
    use proto::{
        Checkpoint, CheckpointId, Event, EventKind, HistoryAnchor, HostSeq,
        SessionId, UndoAvailability,
    };

    fn event(kind: EventKind) -> Event {
        Event {
            seq: HostSeq(1),
            session: SessionId(1),
            kind,
        }
    }

    fn workspace() -> Event {
        event(EventKind::WorkspaceCreated {
            workspace: proto::Workspace {
                repository: proto::HostPath(String::from("/repo")),
                original: proto::HostPath(String::from("/repo")),
                worktree: proto::HostPath(String::from("/worktree")),
                directory: proto::HostPath(String::from("/worktree")),
                branch: String::from("conductor/1"),
            },
        })
    }

    fn checkpoint(id: u64) -> Event {
        event(EventKind::CheckpointCreated {
            checkpoint: Checkpoint {
                id: CheckpointId(id),
                parent: id.checked_sub(1).map(CheckpointId),
                turn_start: Some(HostSeq(id)),
                commit: id.to_string(),
                history: HistoryAnchor::Fresh,
                undo: UndoAvailability::Available,
            },
        })
    }

    #[test]
    fn accepted_message_has_filesystem_intent_without_claiming_a_native_boundary()
    -> Result<(), &'static str> {
        let events = Vec::from([
            workspace(),
            checkpoint(0),
            event(EventKind::UserMessage {
                text: String::from("edit"),
                by: proto::ClientName(String::from("phone")),
            }),
        ]);
        let book = Book::replay(&events);
        let pending = book
            .plan_snapshot()
            .ok_or("accepted message has no pending snapshot")?
            .checkpoint(String::from("interrupted"));
        assert_eq!(pending.history, HistoryAnchor::Unknown);
        assert_eq!(pending.turn_start, Some(HostSeq(1)));
        assert_eq!(book.resume, None);
        assert_eq!(
            pending.undo,
            UndoAvailability::Unavailable {
                reason: String::from("native turn boundary was not observed")
            }
        );
        let unknown = events
            .into_iter()
            .chain([
                event(EventKind::CheckpointCreated {
                    checkpoint: Checkpoint {
                        id: CheckpointId(1),
                        parent: Some(CheckpointId(0)),
                        turn_start: Some(HostSeq(1)),
                        commit: String::from("unknown"),
                        history: HistoryAnchor::Unknown,
                        undo: UndoAvailability::Unavailable {
                            reason: String::from("unknown"),
                        },
                    },
                }),
                checkpoint(2),
            ])
            .collect::<Vec<Event>>();
        assert_eq!(
            Book::replay(&unknown).availability(&HistoryAnchor::Turns {
                conversation: String::from("native"),
                turn: Some(String::from("later"))
            }),
            UndoAvailability::Unavailable {
                reason: String::from(
                    "retained native turn prefix contains an unobserved boundary"
                )
            }
        );
        Ok(())
    }

    #[test]
    fn failed_snapshot_retains_the_completed_native_boundary_for_retry()
    -> Result<(), &'static str> {
        let history = HistoryAnchor::Messages {
            conversation: String::from("native"),
            message: Some(String::from("finished")),
        };
        let events = Vec::from([
            workspace(),
            checkpoint(0),
            Event {
                seq: HostSeq(5),
                ..event(EventKind::UserMessage {
                    text: String::from("edit"),
                    by: proto::ClientName(String::from("phone")),
                })
            },
            event(EventKind::NativeTurnStarted {
                history: HistoryAnchor::Messages {
                    conversation: String::from("native"),
                    message: None,
                },
                turn_start: Some(HostSeq(5)),
            }),
            event(EventKind::HistoryMaterialized {
                history: history.clone(),
            }),
            event(EventKind::CheckpointReserved {
                checkpoint: CheckpointId(1),
            }),
            event(EventKind::Error {
                message: String::from("checkpoint failed"),
            }),
            event(EventKind::TurnCompleted {
                outcome: proto::TurnOutcome::Completed,
            }),
        ]);
        assert_live_matches_replay(&events);
        let live = Book::replay(&events);
        let retry = live
            .plan_snapshot()
            .ok_or("retry has no pending snapshot")?;
        assert_eq!(retry.reserved, CheckpointId(2));
        assert_eq!(retry.base.as_deref(), Some("0"));
        let saved_checkpoint = retry.checkpoint(String::from("retry"));
        assert_eq!(saved_checkpoint.history, history);
        assert_eq!(saved_checkpoint.turn_start, Some(HostSeq(5)));
        assert_eq!(saved_checkpoint.undo, UndoAvailability::Available);
        let saved = events
            .into_iter()
            .chain([event(EventKind::CheckpointCreated {
                checkpoint: saved_checkpoint,
            })])
            .collect::<Vec<Event>>();
        assert_live_matches_replay(&saved);
        assert_eq!(Book::replay(&saved).plan_snapshot(), None);
        Ok(())
    }

    fn assert_live_matches_replay(events: &[Event]) {
        let mut live = Book::default();
        for (index, event) in events.iter().enumerate() {
            match &event.kind {
                EventKind::UserMessage { .. } => live.accept_message(event.seq),
                kind => live.apply_kind(kind),
            }
            let reopened = Book::replay(&events[..=index]);
            assert_eq!(live.plan_snapshot(), reopened.plan_snapshot());
            assert_eq!(live.next_reservation(), reopened.next_reservation());
            assert_eq!(live.last(), reopened.last());
            assert_eq!(live.resume, reopened.resume);
            assert_eq!(
                live.diff(CheckpointId(2)),
                reopened.diff(CheckpointId(2))
            );
        }
    }

    #[test]
    fn reservations_and_undone_checkpoints_keep_the_highwater_and_immutable_diff_history()
    -> Result<(), &'static str> {
        let book = Book::replay(&[
            workspace(),
            checkpoint(0),
            checkpoint(1),
            checkpoint(2),
            event(EventKind::CheckpointReserved {
                checkpoint: CheckpointId(3),
            }),
            event(EventKind::TurnsUndone {
                checkpoint: CheckpointId(1),
                restored: CheckpointId(0),
            }),
        ]);
        assert_eq!(book.last().map(|last| last.id), Some(CheckpointId(0)));
        assert_eq!(book.next_reservation(), CheckpointId(4));
        assert_eq!(book.diff(CheckpointId(0)), None);
        let discarded = book
            .diff(CheckpointId(2))
            .ok_or("discarded turn has no diff")?;
        assert_eq!(discarded.checkpoint.commit, "2");
        assert_eq!(discarded.parent.commit, "1");
        assert_eq!(
            Book::replay(&[checkpoint(0), checkpoint(1)]).next_reservation(),
            CheckpointId(2)
        );
        Ok(())
    }

    #[test]
    fn a_native_turn_start_is_resumed_unless_an_undo_restored_a_history() {
        let turn = |id: &str| HistoryAnchor::Turns {
            conversation: String::from("native"),
            turn: Some(String::from(id)),
        };
        let started = |id| {
            event(EventKind::NativeTurnStarted {
                history: turn(id),
                turn_start: Some(HostSeq(2)),
            })
        };
        let fork = HistoryAnchor::Fork {
            source: String::from("source"),
            through: String::from("boundary"),
            target: String::from("target"),
        };
        assert_eq!(
            [
                Book::replay(&[checkpoint(0), started("t1")]).resume,
                Book::replay(&[
                    checkpoint(0),
                    event(EventKind::UndoHistoryRestored {
                        checkpoint: CheckpointId(1),
                        history: fork.clone(),
                    }),
                    started("t2"),
                ])
                .resume,
            ],
            [Some(turn("t1")), Some(fork)]
        );
    }

    #[test]
    fn failure_keeps_native_recovery_pending_until_durable_completion() {
        let history = HistoryAnchor::Fork {
            source: String::from("source"),
            through: String::from("boundary"),
            target: String::from("target"),
        };
        let events = Vec::from([
            checkpoint(0),
            checkpoint(1),
            event(EventKind::UndoStarted {
                checkpoint: CheckpointId(1),
                restore: CheckpointId(0),
                history: history.clone(),
            }),
            event(EventKind::UndoFailed {
                checkpoint: CheckpointId(1),
                reason: String::from("native unavailable"),
            }),
        ]);
        assert_eq!(
            Book::replay(&events)
                .recovery
                .as_ref()
                .map(|phase| match phase {
                    Recovery::Native(undo) => undo.selected,
                    Recovery::Files(undo) => undo.restored,
                }),
            Some(CheckpointId(1))
        );
        let files = events
            .into_iter()
            .chain([
                event(EventKind::UndoHistoryRestored {
                    checkpoint: CheckpointId(1),
                    history: history.clone(),
                }),
                event(EventKind::UndoFailed {
                    checkpoint: CheckpointId(1),
                    reason: String::from("files unavailable"),
                }),
            ])
            .collect::<Vec<Event>>();
        let book = Book::replay(&files);
        assert_eq!(book.resume, Some(history));
        assert_eq!(
            book.recovery.as_ref().map(|phase| match phase {
                Recovery::Native(undo) => undo.selected,
                Recovery::Files(undo) => undo.restored,
            }),
            Some(CheckpointId(0))
        );
        let completed = files
            .into_iter()
            .chain([event(EventKind::TurnsUndone {
                checkpoint: CheckpointId(1),
                restored: CheckpointId(0),
            })])
            .collect::<Vec<Event>>();
        assert_eq!(
            Book::replay(&completed).recovery.map(|phase| match phase {
                Recovery::Native(undo) | Recovery::Files(undo) => undo.selected,
            }),
            None
        );
    }
}
