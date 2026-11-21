//! The progress an agent reports on its issue while it works (docs/home.md
//! §4), from data its session already carries.
//!
//! When a turn of a session on an issue completes, the checklist items its
//! agent finished since the last one that did become a note on the issue;
//! an interrupted or failed turn's carry on to the next. Its summary turn,
//! once it is finishing, notes nothing, so the summary stays the last note.
//! The shell watches the log and writes the note.

use alloc::format;

use proto::{
    CommandId, Epoch, Event, EventKind, FinishState, HostSeq, Task, TaskBoard,
    TaskStatus, TurnOutcome,
};

use sha2::{Digest, Sha256};

use crate::assignment::{Assigned, Assignment};
use crate::prelude::*;

/// A note on the issue a session works on, signed by its instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub assignment: Assignment,
    pub text: String,
}

/// The same turn uses the same durable home edit identity after a retry or restart.
#[must_use]
pub fn edit_id(epoch: Epoch, seq: HostSeq) -> CommandId {
    let mut hash = Sha256::new();
    hash.update(b"conductor turn progress");
    hash.update(epoch.0.to_be_bytes());
    hash.update(seq.0.to_be_bytes());
    let mut id = [0; 16];
    id.copy_from_slice(&hash.finalize()[..16]);
    CommandId(u128::from_be_bytes(id))
}

#[must_use]
pub const fn ends_turn(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::TurnCompleted {
            outcome: TurnOutcome::Completed
        }
    )
}

/// What the turn `ended` ended did on its session's issue, from `history`,
/// which holds it: `None` when the session is on no issue, is finishing,
/// or its agent finished no checklist item.
#[must_use]
pub fn progress(history: &[Event], ended: &Event) -> Option<Progress> {
    if !ends_turn(ended) {
        return None;
    }
    let mut board = TaskBoard::new();
    let mut session = Session::default();
    for event in history.iter().take_while(|event| event.seq <= ended.seq) {
        board.apply(event);
        if event.session == ended.session {
            session =
                session.after(event, ended.seq, board.held(ended.session));
        }
    }
    let Session {
        assigned,
        before,
        finish,
        importing,
    } = session;
    let now = board.held(ended.session);
    let finished = now
        .iter()
        .filter(|task| {
            done(task)
                && !before.iter().any(|was| was.id == task.id && done(was))
        })
        .map(|task| task.text.as_str())
        .collect::<Vec<&str>>();
    if importing || finished.is_empty() || finish == FinishState::Finishing {
        return None;
    }
    Some(Progress {
        assignment: assigned.assignment()?,
        text: format!("Done: {}.", finished.join("; ")),
    })
}

fn done(task: &Task) -> bool {
    task.status == TaskStatus::Completed
}

/// A session as its log has it up to a turn's end: its issue and its
/// instance, its checklist when its previous completed turn ended and now,
/// and where it stands with finishing.
#[derive(Default)]
struct Session {
    assigned: Assigned,
    before: Vec<Task>,
    finish: FinishState,
    importing: bool,
}

impl Session {
    fn after(self, event: &Event, ended: HostSeq, tasks: &[Task]) -> Self {
        let finish = self.finish.after(&event.kind);
        if matches!(event.kind, EventKind::SessionArrived { .. }) {
            return Self {
                importing: true,
                ..Self::default()
            };
        }
        let importing = self.importing
            && !matches!(event.kind, EventKind::SessionArrivalReady { .. });
        let assigned = self.assigned.after(&event.kind);
        let before = if ends_turn(event) && event.seq < ended {
            tasks.to_vec()
        } else {
            self.before
        };
        Self {
            assigned,
            before,
            finish,
            importing,
        }
    }
}

#[cfg(test)]
mod tests;
