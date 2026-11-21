//! Replayable workspace checkpoints and the durable undo phases.
use crate::prelude::*;
use alloc::collections::BTreeMap;
use proto::{
    Checkpoint, CheckpointId, Event, EventKind, HistoryAnchor, HostSeq,
    Rejection, TurnOutcome, UndoAvailability, Workspace,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Undo {
    pub selected: CheckpointId,
    pub restored: CheckpointId,
    pub history: HistoryAnchor,
}

/// The pending undo, as planned, and its phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovery {
    Native(Undo),
    Verify(Undo),
    Files(Undo),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoPlan {
    Start {
        undo: Undo,
        workspace: Workspace,
        commit: String,
    },
    Resume(Recovery),
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
    workspace: Option<Workspace>,
    checkpoints: BTreeMap<CheckpointId, Checkpoint>,
    active: Vec<CheckpointId>,
    highwater: u64,
    recovery: Option<Recovery>,
    resume: Option<HistoryAnchor>,
    snapshot: Option<PendingTurn>,
    accepted_since_undo_restore: bool,
    before_pending_message: Option<bool>,
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

    #[must_use]
    pub const fn workspace(&self) -> Option<&Workspace> {
        self.workspace.as_ref()
    }

    /// A message sent after a fresh undo can have a saved conversation before
    /// its native boundary arrives.
    #[must_use]
    pub fn resume(&self, has_conversation: bool) -> Option<HistoryAnchor> {
        match &self.resume {
            Some(HistoryAnchor::Fresh)
                if has_conversation && self.accepted_since_undo_restore =>
            {
                None
            }
            history => history.clone(),
        }
    }

    pub fn apply(&mut self, event: &Event) {
        match &event.kind {
            EventKind::UserMessage { .. } => self.accept_message(event.seq),
            kind => self.apply_kind(kind),
        }
    }

    pub fn accept_message(&mut self, seq: HostSeq) {
        self.before_pending_message = Some(self.accepted_since_undo_restore);
        self.accepted_since_undo_restore = true;
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
            EventKind::TurnCompleted {
                outcome: TurnOutcome::Failed { .. },
            } => {
                if let Some(previous) = self.before_pending_message.take() {
                    self.snapshot = None;
                    self.accepted_since_undo_restore = previous;
                }
            }
            EventKind::NativeTurnStarted {
                history,
                turn_start,
            } => {
                self.before_pending_message = None;
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
                self.before_pending_message = None;
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
            EventKind::UndoRevertDispatched { checkpoint } if matches!(&self.recovery, Some(Recovery::Native(undo)) if undo.selected == *checkpoint) =>
            {
                self.recovery = self.recovery.take().map(|phase| match phase {
                    Recovery::Native(undo) => Recovery::Verify(undo),
                    other => other,
                });
            }
            EventKind::UndoFailed { checkpoint, .. } if matches!(&self.recovery, Some(Recovery::Verify(undo)) if undo.selected == *checkpoint) =>
            {
                self.recovery = self.recovery.take().map(|phase| match phase {
                    Recovery::Verify(undo) => Recovery::Native(undo),
                    other => other,
                });
            }
            EventKind::UndoHistoryRestored { history, .. } => {
                self.before_pending_message = None;
                self.accepted_since_undo_restore = false;
                self.resume = Some(history.clone());
                self.recovery = self.recovery.take().map(|phase| match phase {
                    Recovery::Native(undo)
                    | Recovery::Verify(undo)
                    | Recovery::Files(undo) => Recovery::Files(undo),
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
        let previous = self.active_checkpoint();
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
    pub const fn undo_pending(&self) -> bool {
        self.recovery.is_some()
    }

    #[must_use]
    pub const fn awaiting_revert(&self) -> bool {
        matches!(&self.recovery, Some(Recovery::Verify(_)))
    }

    pub fn plan_undo(
        &self,
        selected: CheckpointId,
        fork: &str,
        turn_running: bool,
    ) -> Result<UndoPlan, Rejection> {
        match &self.recovery {
            Some(
                phase @ (Recovery::Native(undo)
                | Recovery::Verify(undo)
                | Recovery::Files(undo)),
            ) if undo.selected == selected => {
                Ok(UndoPlan::Resume(phase.clone()))
            }
            Some(_) => Err(Rejection::UndoInProgress),
            None if turn_running => Err(Rejection::TurnInProgress),
            None => self.new_undo(selected, fork),
        }
    }

    fn new_undo(
        &self,
        selected: CheckpointId,
        fork: &str,
    ) -> Result<UndoPlan, Rejection> {
        let compared = self
            .diff(selected)
            .filter(|compared| {
                self.active.contains(&selected)
                    && compared.checkpoint.undo == UndoAvailability::Available
            })
            .ok_or(Rejection::CheckpointUnavailable)?;
        let history = undo_history(
            &compared.parent.history,
            &compared.checkpoint.history,
            fork,
        )?;
        Ok(UndoPlan::Start {
            undo: Undo {
                selected,
                restored: compared.parent.id,
                history,
            },
            workspace: compared.workspace,
            commit: compared.parent.commit,
        })
    }

    #[must_use]
    pub fn restore_point(
        &self,
        restored: CheckpointId,
    ) -> Option<(Workspace, String)> {
        let workspace = self.workspace.as_ref()?;
        let checkpoint = self.checkpoints.get(&restored)?;
        Some((workspace.clone(), checkpoint.commit.clone()))
    }

    #[must_use]
    pub fn retained_turns(&self, restored: CheckpointId) -> Vec<String> {
        self.active
            .iter()
            .copied()
            .filter(|id| *id <= restored)
            .filter_map(|id| self.checkpoints.get(&id))
            .filter_map(|checkpoint| match &checkpoint.history {
                HistoryAnchor::Turns {
                    turn: Some(kept), ..
                } => Some(kept.clone()),
                _ => None,
            })
            .collect()
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
                None => availability(self.active_checkpoint(), history),
            },
            _ => availability(self.active_checkpoint(), history),
        }
    }

    #[must_use]
    pub fn active_checkpoint(&self) -> Option<&Checkpoint> {
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

/// What an undo restores the agent's history to: a fresh conversation, a
/// fork through the parent's last message, or the turns before the
/// checkpoint's.
fn undo_history(
    parent: &HistoryAnchor,
    checkpoint: &HistoryAnchor,
    fork: &str,
) -> Result<HistoryAnchor, Rejection> {
    match (parent, checkpoint) {
        (HistoryAnchor::Fresh, _) => Ok(HistoryAnchor::Fresh),
        (
            HistoryAnchor::Messages {
                conversation,
                message: Some(message),
            },
            _,
        ) => Ok(HistoryAnchor::Fork {
            source: conversation.clone(),
            through: message.clone(),
            target: String::from(fork),
        }),
        (
            _,
            HistoryAnchor::Turns {
                conversation,
                turn: Some(turn),
            },
        ) => Ok(HistoryAnchor::Turns {
            conversation: conversation.clone(),
            turn: Some(turn.clone()),
        }),
        _ => Err(Rejection::CheckpointUnavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::{Book, Recovery, UndoPlan};
    use proto::{
        Checkpoint, CheckpointId, Event, EventKind, HistoryAnchor, HostSeq,
        Rejection, SessionId, UndoAvailability,
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
                turn_start: (id != 0).then_some(HostSeq(id)),
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
        assert_eq!(book.resume(false), None);
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
                event(EventKind::NativeTurnStarted {
                    history: HistoryAnchor::Turns {
                        conversation: String::from("native"),
                        turn: Some(String::from("later")),
                    },
                    turn_start: Some(HostSeq(3)),
                }),
            ])
            .collect::<Vec<Event>>();
        assert_eq!(
            Book::replay(&unknown)
                .plan_snapshot()
                .ok_or("later turn has no pending snapshot")?
                .checkpoint(String::from("later"))
                .undo,
            UndoAvailability::Unavailable {
                reason: String::from(
                    "retained native turn prefix contains an unobserved boundary"
                )
            }
        );
        Ok(())
    }

    #[test]
    fn replay_discards_a_failed_message_without_a_native_turn() {
        let events = [
            workspace(),
            checkpoint(0),
            event(EventKind::UserMessage {
                text: String::from("not delivered"),
                by: proto::ClientName(String::from("phone")),
            }),
            event(EventKind::TurnCompleted {
                outcome: proto::TurnOutcome::Failed {
                    message: String::from("send rejected"),
                },
            }),
        ];
        assert!(Book::replay(&events[..3]).plan_snapshot().is_some());
        assert!(Book::replay(&events).plan_snapshot().is_none());
    }

    #[test]
    fn failed_snapshot_retains_the_completed_native_boundary_for_retry() {
        let history = HistoryAnchor::Messages {
            conversation: String::from("native"),
            message: Some(String::from("finished")),
        };
        let events = [
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
            event(EventKind::CheckpointReserved {
                checkpoint: CheckpointId(2),
            }),
            event(EventKind::CheckpointCreated {
                checkpoint: Checkpoint {
                    id: CheckpointId(2),
                    parent: Some(CheckpointId(0)),
                    turn_start: Some(HostSeq(5)),
                    commit: String::from("retry"),
                    history: history.clone(),
                    undo: UndoAvailability::Available,
                },
            }),
        ];
        let started = HistoryAnchor::Messages {
            conversation: String::from("native"),
            message: None,
        };
        let expected = [
            (None, 1, None),
            (None, 1, None),
            (Some(HistoryAnchor::Unknown), 1, None),
            (Some(started.clone()), 1, Some(started)),
            (Some(history.clone()), 1, Some(history.clone())),
            (Some(history.clone()), 2, Some(history.clone())),
            (Some(history.clone()), 2, Some(history.clone())),
            (Some(history.clone()), 2, Some(history.clone())),
            (Some(history.clone()), 3, Some(history.clone())),
            (None, 3, Some(history)),
        ];
        let mut live = Book::default();
        for (index, (boundary, reservation, resume)) in
            expected.into_iter().enumerate()
        {
            live.apply(&events[index]);
            for book in [&live, &Book::replay(&events[..=index])] {
                assert_eq!(book.next_reservation(), CheckpointId(reservation));
                assert_eq!(book.resume(true), resume);
                assert_turn_checkpoints(book, index);
                assert_snapshot(book, boundary.as_ref(), reservation);
            }
        }
    }

    fn assert_turn_checkpoints(book: &Book, index: usize) {
        assert_eq!(
            book.active_checkpoint().map(|checkpoint| checkpoint.id),
            match index {
                0 => None,
                9 => Some(CheckpointId(2)),
                _ => Some(CheckpointId(0)),
            }
        );
        let diff = book.diff(CheckpointId(2));
        let plan = book.plan_undo(CheckpointId(2), "fork", false);
        if index == 9 {
            assert_eq!(
                diff.map(|compared| (
                    compared.checkpoint.commit,
                    compared.parent.commit
                )),
                Some((String::from("retry"), String::from("0")))
            );
            assert!(matches!(plan, Ok(UndoPlan::Start { undo, commit, .. })
                if undo.restored == CheckpointId(0) && commit == "0"));
        } else {
            assert_eq!(diff, None);
            assert_eq!(plan, Err(Rejection::CheckpointUnavailable));
        }
    }

    fn assert_snapshot(
        book: &Book,
        boundary: Option<&HistoryAnchor>,
        reservation: u64,
    ) {
        let pending = book.plan_snapshot();
        assert_eq!(pending.is_some(), boundary.is_some());
        if let Some(plan) = pending {
            assert_eq!(plan.reserved, CheckpointId(reservation));
            assert_eq!(plan.base.as_deref(), Some("0"));
            let checkpoint = plan.checkpoint(String::from("retry"));
            assert_eq!(Some(&checkpoint.history), boundary);
            assert_eq!(checkpoint.parent, Some(CheckpointId(0)));
            assert_eq!(checkpoint.turn_start, Some(HostSeq(5)));
            if matches!(
                boundary,
                Some(HistoryAnchor::Messages {
                    message: Some(_),
                    ..
                })
            ) {
                assert_eq!(checkpoint.undo, UndoAvailability::Available);
            }
        }
    }

    #[test]
    fn undo_request_identifies_the_work_remaining_at_each_durable_prefix()
    -> Result<(), &'static str> {
        let selected = CheckpointId(1);
        let restored = CheckpointId(0);
        let events = [
            workspace(),
            checkpoint(0),
            checkpoint(1),
            checkpoint(2),
            event(EventKind::CheckpointReserved {
                checkpoint: CheckpointId(3),
            }),
            event(EventKind::UndoStarted {
                checkpoint: selected,
                restore: restored,
                history: HistoryAnchor::Fresh,
            }),
            event(EventKind::UndoFailed {
                checkpoint: selected,
                reason: String::from("native unavailable"),
            }),
            event(EventKind::UndoHistoryRestored {
                checkpoint: selected,
                history: HistoryAnchor::Fresh,
            }),
            event(EventKind::UndoFailed {
                checkpoint: selected,
                reason: String::from("files unavailable"),
            }),
            event(EventKind::TurnsUndone {
                checkpoint: selected,
                restored,
            }),
        ];
        let mut live = Book::default();
        for (index, event) in events.iter().enumerate() {
            live.apply(event);
            for book in [&live, &Book::replay(&events[..=index])] {
                assert_undo_prefix(book, index)?;
            }
        }
        let mut after_undo = Vec::from(events);
        after_undo.push(event(EventKind::UserMessage {
            text: String::from("edit again"),
            by: proto::ClientName(String::from("phone")),
        }));
        let book = Book::replay(&after_undo);
        let next = book.plan_snapshot().ok_or("missing post-undo snapshot")?;
        assert_eq!(next.reserved, CheckpointId(4));
        assert_eq!(next.base.as_deref(), Some("0"));
        Ok(())
    }

    fn assert_undo_prefix(
        book: &Book,
        index: usize,
    ) -> Result<(), &'static str> {
        let selected = CheckpointId(1);
        let restored = CheckpointId(0);
        let plan = book.plan_undo(selected, "fork", false);
        assert_eq!(
            book.next_reservation(),
            CheckpointId(match index {
                0 | 1 => 1,
                2 => 2,
                3 => 3,
                _ => 4,
            }),
        );
        assert_eq!(
            book.active_checkpoint().map(|checkpoint| checkpoint.id),
            match index {
                0 => None,
                1 | 9 => Some(restored),
                2 => Some(selected),
                _ => Some(CheckpointId(2)),
            }
        );
        assert_eq!(book.undo_pending(), (5..=8).contains(&index));
        assert_eq!(
            book.resume(true),
            (index >= 7).then_some(HistoryAnchor::Fresh)
        );
        assert_eq!(
            book.diff(CheckpointId(2)).map(|compared| (
                compared.checkpoint.commit,
                compared.parent.commit
            )),
            (index >= 3).then(|| (String::from("2"), String::from("1")))
        );
        match index {
            0 | 1 | 9 => {
                assert_eq!(plan, Err(Rejection::CheckpointUnavailable));
            }
            2..=4 => {
                assert!(
                    matches!(plan, Ok(UndoPlan::Start { undo, commit, .. })
                if undo.restored == restored && undo.history == HistoryAnchor::Fresh && commit == "0")
                );
            }
            5 | 6 => assert!(
                matches!(plan, Ok(UndoPlan::Resume(Recovery::Native(undo)))
                if undo.selected == selected && undo.history == HistoryAnchor::Fresh)
            ),
            7 | 8 => assert!(
                matches!(plan, Ok(UndoPlan::Resume(Recovery::Files(undo)))
                if undo.selected == selected && undo.restored == restored)
            ),
            _ => return Err("unexpected prefix"),
        }
        if (5..=8).contains(&index) {
            assert_eq!(
                book.plan_undo(CheckpointId(2), "another fork", false),
                Err(Rejection::UndoInProgress)
            );
        } else {
            assert_eq!(
                book.plan_undo(restored, "fork", false),
                Err(Rejection::CheckpointUnavailable)
            );
        }
        Ok(())
    }

    #[test]
    fn accepted_input_after_a_fresh_undo_resumes_the_saved_conversation() {
        let restored = event(EventKind::UndoHistoryRestored {
            checkpoint: CheckpointId(1),
            history: HistoryAnchor::Fresh,
        });
        let accepted = event(EventKind::UserMessage {
            text: String::from("edit again"),
            by: proto::ClientName(String::from("phone")),
        });
        assert_eq!(
            Book::replay(core::slice::from_ref(&restored)).resume(true),
            Some(HistoryAnchor::Fresh)
        );
        let book = Book::replay(&[restored, accepted]);
        assert_eq!(book.resume(true), None);
        assert_eq!(book.resume(false), Some(HistoryAnchor::Fresh));
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
                Book::replay(&[checkpoint(0), started("t1")]).resume(false),
                Book::replay(&[
                    checkpoint(0),
                    event(EventKind::UndoHistoryRestored {
                        checkpoint: CheckpointId(1),
                        history: fork.clone(),
                    }),
                    started("t2"),
                ])
                .resume(false),
            ],
            [Some(turn("t1")), Some(fork)]
        );
    }
}
