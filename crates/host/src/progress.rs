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
    Event, EventKind, FinishState, HostSeq, Task, TaskStatus, TurnOutcome,
};

use crate::assignment::{Assigned, Assignment};
use crate::prelude::*;

/// A note on the issue a session works on, signed by its instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub assignment: Assignment,
    pub text: String,
}

/// Where the watch starts: after the last event `history` holds.
#[must_use]
pub fn latest(history: &[Event]) -> HostSeq {
    history.last().map_or(HostSeq(0), |event| event.seq)
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
    let mine = history
        .iter()
        .take_while(|event| event.seq <= ended.seq)
        .filter(|event| event.session == ended.session)
        .collect::<Vec<&Event>>();
    let Session {
        assigned,
        before,
        now,
        finish,
    } = mine.iter().fold(Session::default(), |session, event| {
        session.after(event, ended.seq)
    });
    let finished = now
        .iter()
        .filter(|task| {
            done(task)
                && !before.iter().any(|was| was.id == task.id && done(was))
        })
        .map(|task| task.text.as_str())
        .collect::<Vec<&str>>();
    if finished.is_empty() || finish == FinishState::Finishing {
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
    now: Vec<Task>,
    finish: FinishState,
}

impl Session {
    fn after(self, event: &Event, ended: HostSeq) -> Self {
        let finish = self.finish.after(&event.kind);
        let next = Self { finish, ..self };
        if ends_turn(event) && event.seq < ended {
            return Self {
                before: next.now.clone(),
                ..next
            };
        }
        next.with(&event.kind)
    }

    fn with(self, kind: &EventKind) -> Self {
        let assigned = self.assigned.after(kind);
        let next = Self { assigned, ..self };
        match kind {
            EventKind::TasksChanged { tasks } => Self {
                now: tasks.clone(),
                ..next
            },
            _ => next,
        }
    }
}

#[cfg(test)]
mod tests;
