//! The progress an agent reports on its issue while it works (docs/home.md
//! §4), from data its session already carries.
//!
//! When a turn of a session on an issue ends, the checklist items its agent
//! finished in the turn become a note on the issue. The shell watches the
//! log and writes the note.

use alloc::format;

use proto::{Event, EventKind, HostSeq, InstanceId, IssueId, Task, TaskStatus};

use crate::prelude::*;

/// A note on the issue a session works on, signed by its instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub issue: IssueId,
    pub instance: InstanceId,
    pub text: String,
}

/// The last event `history` holds, after which the watch looks.
#[must_use]
pub fn latest(history: &[Event]) -> HostSeq {
    history.last().map_or(HostSeq(0), |event| event.seq)
}

/// Whether `event` ends a turn, which may then note progress.
#[must_use]
pub const fn ends_turn(event: &Event) -> bool {
    matches!(event.kind, EventKind::TurnCompleted { .. })
}

/// What the turn `ended` ended did on its session's issue, from `history`,
/// which holds it: `None` when the session is on no issue or its agent
/// finished no checklist item in the turn.
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
        issue,
        instance,
        before,
        now,
    } = mine.iter().fold(Session::default(), |session, event| {
        session.then(event, ended.seq)
    });
    let finished = now
        .iter()
        .filter(|task| {
            done(task)
                && !before.iter().any(|was| was.id == task.id && done(was))
        })
        .map(|task| task.text.as_str())
        .collect::<Vec<&str>>();
    if finished.is_empty() {
        return None;
    }
    Some(Progress {
        issue: issue?,
        instance: instance?,
        text: format!("Done: {}.", finished.join("; ")),
    })
}

fn done(task: &Task) -> bool {
    task.status == TaskStatus::Completed
}

/// A session as its log has it up to a turn's end: its issue, its
/// instance, and its checklist when its previous turn ended and now.
#[derive(Default)]
struct Session {
    issue: Option<IssueId>,
    instance: Option<InstanceId>,
    before: Vec<Task>,
    now: Vec<Task>,
}

impl Session {
    fn then(self, event: &Event, ended: HostSeq) -> Self {
        match &event.kind {
            EventKind::SessionCreated { instance, .. } => Self {
                instance: Some(instance.clone()),
                ..self
            },
            EventKind::OnIssue { issue } => Self {
                issue: Some(*issue),
                ..self
            },
            EventKind::SessionRemoved { .. } => Self {
                issue: None,
                ..self
            },
            EventKind::TasksChanged { tasks } => Self {
                now: tasks.clone(),
                ..self
            },
            EventKind::TurnCompleted { .. } if event.seq < ended => Self {
                before: self.now.clone(),
                ..self
            },
            _ => self,
        }
    }
}

#[cfg(test)]
mod tests;
