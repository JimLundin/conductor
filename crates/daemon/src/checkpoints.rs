//! Replayable workspace checkpoints and the durable undo phases.
use proto::{
    Checkpoint, CheckpointId, Event, EventKind, HistoryAnchor, UndoAvailability, Workspace,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub(crate) struct Undo {
    pub(crate) selected: CheckpointId,
    pub(crate) restored: CheckpointId,
    pub(crate) history: HistoryAnchor,
}

#[derive(Debug, Clone)]
pub(crate) enum Recovery {
    Native(Undo),
    Files(Undo),
}

#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    pub(crate) history: HistoryAnchor,
    pub(crate) turn_start: Option<proto::HostSeq>,
}

pub(crate) struct Book {
    pub(crate) workspace: Option<Workspace>,
    pub(crate) checkpoints: BTreeMap<CheckpointId, Checkpoint>,
    pub(crate) active: Vec<CheckpointId>,
    pub(crate) highwater: CheckpointId,
    pub(crate) recovery: Option<Recovery>,
    pub(crate) resume: Option<HistoryAnchor>,
    pub(crate) snapshot: Option<Snapshot>,
}

impl Book {
    pub(crate) fn replay(events: &[Event]) -> Book {
        Book {
            workspace: events.iter().rev().find_map(|event| match &event.kind {
                EventKind::WorkspaceCreated { workspace } => Some(workspace.clone()),
                _ => None,
            }),
            checkpoints: events
                .iter()
                .filter_map(|event| match &event.kind {
                    EventKind::CheckpointCreated { checkpoint } => {
                        Some((checkpoint.id, checkpoint.clone()))
                    }
                    _ => None,
                })
                .collect(),
            active: events.iter().fold(Vec::new(), |mut active, event| {
                match &event.kind {
                    EventKind::CheckpointCreated { checkpoint } => active.push(checkpoint.id),
                    EventKind::TurnsUndone { restored, .. } => active.retain(|id| id <= restored),
                    _ => {}
                }
                active
            }),
            highwater: events
                .iter()
                .filter_map(|event| match &event.kind {
                    EventKind::CheckpointCreated { checkpoint } => Some(checkpoint.id),
                    EventKind::CheckpointReserved { checkpoint } => Some(checkpoint.to_owned()),
                    _ => None,
                })
                .max()
                .unwrap_or(CheckpointId(0)),
            recovery: events
                .iter()
                .fold(None, |pending, event| match &event.kind {
                    EventKind::UndoStarted {
                        checkpoint,
                        restore,
                        history,
                    } => Some(Recovery::Native(Undo {
                        selected: checkpoint.to_owned(),
                        restored: restore.to_owned(),
                        history: history.clone(),
                    })),
                    EventKind::UndoHistoryRestored {
                        checkpoint,
                        history,
                    } => pending.map(|pending| {
                        let undo = match pending {
                            Recovery::Native(undo) | Recovery::Files(undo) => undo,
                        };
                        Recovery::Files(Undo {
                            selected: checkpoint.to_owned(),
                            history: history.clone(),
                            ..undo
                        })
                    }),
                    EventKind::TurnsUndone { .. } => None,
                    _ => pending,
                }),
            snapshot: events
                .iter()
                .fold(None, |snapshot, event| match &event.kind {
                    EventKind::UserMessage { .. } => Some(Snapshot {
                        history: HistoryAnchor::Unknown,
                        turn_start: Some(event.seq),
                    }),
                    EventKind::NativeTurnStarted {
                        history,
                        turn_start,
                    } => Some(Snapshot {
                        history: history.clone(),
                        turn_start: turn_start.to_owned(),
                    }),
                    EventKind::HistoryMaterialized { history } => {
                        snapshot.map(|snapshot| Snapshot {
                            history: history.clone(),
                            ..snapshot
                        })
                    }
                    EventKind::CheckpointCreated { .. } | EventKind::TurnsUndone { .. } => None,
                    _ => snapshot,
                }),
            resume: events.iter().fold(None, |resume, event| match &event.kind {
                EventKind::UndoHistoryRestored { history, .. }
                | EventKind::HistoryMaterialized { history } => Some(history.clone()),
                EventKind::NativeTurnStarted { history, .. } => match resume {
                    None | Some(HistoryAnchor::Fresh) => Some(history.clone()),
                    resume => resume,
                },
                _ => resume,
            }),
        }
    }

    pub(crate) fn availability(&self, history: &HistoryAnchor) -> UndoAvailability {
        match history {
            HistoryAnchor::Codex { .. } => match self
                .active
                .iter()
                .filter_map(|id| self.checkpoints.get(id))
                .find(|checkpoint| matches!(checkpoint.history, HistoryAnchor::Unknown))
            {
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

    pub(crate) fn last(&self) -> Option<&Checkpoint> {
        self.active.last().and_then(|id| self.checkpoints.get(id))
    }
}

pub(crate) fn availability(
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
        ) => UndoAvailability::Available,
        (
            Some(Checkpoint {
                history:
                    HistoryAnchor::Claude {
                        message: Some(_), ..
                    },
                ..
            }),
            HistoryAnchor::Claude { .. },
        ) => UndoAvailability::Available,
        (Some(_), HistoryAnchor::Codex { turn: Some(_), .. }) => UndoAvailability::Available,
        _ => UndoAvailability::Unavailable {
            reason: String::from("native history has no safe turn boundary"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{Book, Recovery};
    use proto::{
        Checkpoint, CheckpointId, Event, EventKind, HistoryAnchor, HostSeq, SessionId,
        UndoAvailability,
    };

    fn event(kind: EventKind) -> Event {
        Event {
            seq: HostSeq(1),
            session: SessionId(1),
            kind,
        }
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
    fn accepted_message_has_filesystem_intent_without_claiming_a_native_boundary() {
        let events = Vec::from([
            checkpoint(0),
            event(EventKind::UserMessage {
                text: String::from("edit"),
                by: proto::ClientName(String::from("phone")),
            }),
        ]);
        let book = Book::replay(&events);
        assert_eq!(
            book.snapshot.as_ref().map(|snapshot| &snapshot.history),
            Some(&HistoryAnchor::Unknown)
        );
        assert_eq!(book.resume, None);
        assert_eq!(
            super::availability(book.last(), &HistoryAnchor::Unknown),
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
            Book::replay(&unknown).availability(&HistoryAnchor::Codex {
                conversation: String::from("native"),
                turn: Some(String::from("later"))
            }),
            UndoAvailability::Unavailable {
                reason: String::from("retained native turn prefix contains an unobserved boundary")
            }
        );
    }

    #[test]
    fn failed_snapshot_retains_the_completed_native_boundary_for_retry() {
        let history = HistoryAnchor::Claude {
            conversation: String::from("native"),
            message: Some(String::from("finished")),
        };
        let events = Vec::from([
            checkpoint(0),
            event(EventKind::NativeTurnStarted {
                history: HistoryAnchor::Claude {
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
        let book = Book::replay(&events);
        assert_eq!(
            book.snapshot.as_ref().map(|snapshot| &snapshot.history),
            Some(&history)
        );
        assert_eq!(
            book.snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.turn_start),
            Some(HostSeq(5))
        );
        assert_eq!(book.highwater.next(), CheckpointId(2));
        assert_eq!(
            super::availability(book.last(), &history),
            UndoAvailability::Available
        );
        let saved = events
            .into_iter()
            .chain([event(EventKind::CheckpointCreated {
                checkpoint: Checkpoint {
                    id: CheckpointId(2),
                    parent: Some(CheckpointId(0)),
                    turn_start: Some(HostSeq(5)),
                    commit: String::from("retry"),
                    history,
                    undo: UndoAvailability::Available,
                },
            })])
            .collect::<Vec<Event>>();
        assert_eq!(
            Book::replay(&saved)
                .snapshot
                .map(|snapshot| snapshot.history),
            None
        );
    }

    #[test]
    fn reservations_and_undone_checkpoints_keep_the_highwater_and_immutable_diff_history() {
        let book = Book::replay(&[
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
        assert_eq!(book.active, Vec::from([CheckpointId(0)]));
        assert_eq!(book.highwater.next(), CheckpointId(4));
        assert_eq!(book.checkpoints.len(), 3);
    }

    #[test]
    fn failure_keeps_native_recovery_pending_until_durable_completion() {
        let history = HistoryAnchor::ClaudeFork {
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
