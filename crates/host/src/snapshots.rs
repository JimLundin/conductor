//! Turn records and pending captures reconstructed from session events.
use crate::log::{Event, HostSeq};
use crate::prelude::*;
use crate::session::EventKind;
use crate::workspace::{Snapshot, Workspace};
use agents::TurnOutcome;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotPlan {
    pub workspace: Workspace,
    pub(crate) turn_start: Option<HostSeq>,
}

impl SnapshotPlan {
    #[must_use]
    pub fn snapshot(self, commit: String) -> Snapshot {
        Snapshot {
            turn_start: self.turn_start,
            commit,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compared {
    pub workspace: Workspace,
    pub snapshot: HostSeq,
    pub commit: String,
    pub previous: String,
}

enum PendingTurn {
    Accepted(HostSeq),
    Started(Option<HostSeq>),
}

#[derive(Default)]
pub struct Book {
    workspace: Option<Workspace>,
    snapshots: Vec<(HostSeq, Snapshot)>,
    pending: Option<PendingTurn>,
}

impl Book {
    #[must_use]
    pub fn replay(events: &[Event]) -> Self {
        let mut book = Self::default();
        for event in events {
            match &event.kind {
                EventKind::UserMessage { .. } => book.accept_message(event.seq),
                EventKind::SnapshotRecorded { snapshot } => {
                    book.record(event.seq, snapshot.clone());
                }
                kind => book.apply_kind(kind),
            }
        }
        book
    }

    #[must_use]
    pub const fn workspace(&self) -> Option<&Workspace> {
        self.workspace.as_ref()
    }

    pub const fn accept_message(&mut self, seq: HostSeq) {
        self.pending = Some(PendingTurn::Accepted(seq));
    }

    pub fn apply_kind(&mut self, kind: &EventKind) {
        match kind {
            EventKind::WorkspaceCreated { workspace } => {
                self.workspace = Some(workspace.clone());
            }
            EventKind::NativeTurnStarted { turn_start } => {
                self.pending = Some(PendingTurn::Started(*turn_start));
            }
            EventKind::TurnCompleted {
                outcome: TurnOutcome::Failed { .. },
            } if matches!(self.pending, Some(PendingTurn::Accepted(_))) => {
                self.pending = None;
            }
            _ => {}
        }
    }

    pub fn record(&mut self, seq: HostSeq, snapshot: Snapshot) {
        self.snapshots.push((seq, snapshot));
        self.pending = None;
    }

    #[must_use]
    pub fn plan_snapshot(&self) -> Option<SnapshotPlan> {
        Some(SnapshotPlan {
            workspace: self.workspace.clone()?,
            turn_start: match self.pending.as_ref()? {
                PendingTurn::Accepted(seq) => Some(*seq),
                PendingTurn::Started(seq) => *seq,
            },
        })
    }

    #[must_use]
    pub fn diff(&self, seq: HostSeq) -> Option<Compared> {
        let pair = self.snapshots.windows(2).find(|pair| pair[1].0 == seq)?;
        Some(Compared {
            workspace: self.workspace.clone()?,
            snapshot: seq,
            commit: pair[1].1.commit.clone(),
            previous: pair[0].1.commit.clone(),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::Book;
    use crate::HostPath;
    use crate::log::{Event, HostSeq};
    use crate::session::{EventKind, SessionId};
    use crate::workspace::{Snapshot, Workspace};
    use agents::{ClientName, TurnOutcome};

    fn event(seq: u64, kind: EventKind) -> Event {
        Event {
            seq: HostSeq(seq),
            session: SessionId(1),
            kind,
        }
    }

    fn opening() -> Vec<Event> {
        let path = HostPath(String::from("/work"));
        Vec::from([
            event(
                1,
                EventKind::WorkspaceCreated {
                    workspace: Workspace {
                        repository: path.clone(),
                        original: path.clone(),
                        worktree: path.clone(),
                        directory: path,
                        branch: String::from("conductor/session"),
                    },
                },
            ),
            snapshot(2, None, "initial"),
        ])
    }

    fn snapshot(seq: u64, turn: Option<u64>, commit: &str) -> Event {
        event(
            seq,
            EventKind::SnapshotRecorded {
                snapshot: Snapshot {
                    turn_start: turn.map(HostSeq),
                    commit: String::from(commit),
                },
            },
        )
    }

    #[test]
    fn unchanged_turns_keep_distinct_records_and_compare_recorded_commits() {
        let mut events = opening();
        events.extend([
            snapshot(8, Some(3), "agent-tip"),
            snapshot(14, Some(10), "agent-tip"),
            snapshot(20, Some(15), "captured"),
        ]);
        let book = Book::replay(&events);
        let comparisons = [8, 14, 20].map(|seq| {
            book.diff(HostSeq(seq))
                .map(|diff| (diff.snapshot, diff.previous, diff.commit))
        });
        assert_eq!(
            comparisons,
            [
                Some((
                    HostSeq(8),
                    String::from("initial"),
                    String::from("agent-tip")
                )),
                Some((
                    HostSeq(14),
                    String::from("agent-tip"),
                    String::from("agent-tip")
                )),
                Some((
                    HostSeq(20),
                    String::from("agent-tip"),
                    String::from("captured")
                )),
            ]
        );
        assert!(book.diff(HostSeq(2)).is_none());
        assert!(book.diff(HostSeq(99)).is_none());
    }

    #[test]
    fn replay_keeps_uncaptured_work_until_its_commit_is_recorded() {
        let mut events = opening();
        events.extend([
            event(
                3,
                EventKind::UserMessage {
                    text: String::from("work"),
                    by: ClientName(String::from("owner")),
                },
            ),
            event(
                4,
                EventKind::NativeTurnStarted {
                    turn_start: Some(HostSeq(3)),
                },
            ),
            event(
                5,
                EventKind::TurnCompleted {
                    outcome: TurnOutcome::Completed,
                },
            ),
        ]);
        assert_eq!(
            Book::replay(&events)
                .plan_snapshot()
                .map(|plan| plan.turn_start),
            Some(Some(HostSeq(3)))
        );
        events.push(snapshot(6, Some(3), "captured"));
        assert!(Book::replay(&events).plan_snapshot().is_none());
    }
}
