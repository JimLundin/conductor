//! Checklist snapshots and handoffs share the host's event order.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::{Event, EventKind, SessionId, Task, TaskId, TaskStatus};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskBoard {
    assigned: BTreeMap<TaskId, Task>,
    held: BTreeMap<SessionId, Vec<Task>>,
}

impl TaskBoard {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            assigned: BTreeMap::new(),
            held: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn held(&self, session: SessionId) -> &[Task] {
        self.held.get(&session).map_or(&[], Vec::as_slice)
    }

    pub fn apply(&mut self, event: &Event) {
        match &event.kind {
            EventKind::TasksChanged { tasks } => {
                self.sync(event.session, tasks);
            }
            EventKind::TaskHandedOff { task } => {
                self.transfer(event.session, task);
            }
            EventKind::SessionRemoved { .. } => {
                self.held.remove(&event.session);
            }
            _ => {}
        }
    }

    fn transfer(&mut self, to: SessionId, task: &Task) {
        if let Some(source) = self.held.get_mut(&task.holder) {
            source.retain(|held| held.id != task.id);
        }
        let moved = Task {
            holder: to,
            status: TaskStatus::Pending,
            ..task.clone()
        };
        self.assigned.insert(moved.id, moved.clone());
        self.held.entry(to).or_default().push(moved);
    }

    fn sync(&mut self, session: SessionId, snapshot: &[Task]) {
        let mut ordinary = Vec::new();
        for step in snapshot {
            if self.assigned.contains_key(&step.id) {
                continue;
            }
            if let Some(assigned) = self.matching(&step.text) {
                if assigned.holder == session {
                    assigned.status = step.status;
                }
            } else {
                ordinary.push(step.clone());
            }
        }
        ordinary.extend(
            self.assigned
                .values()
                .filter(|task| task.holder == session)
                .cloned(),
        );
        self.held.insert(session, ordinary);
    }

    fn matching(&mut self, label: &str) -> Option<&mut Task> {
        let (hex, title) =
            label.strip_prefix("[Conductor task ")?.split_once("] ")?;
        let id = TaskId(u128::from_str_radix(hex, 16).ok()?);
        self.assigned.get_mut(&id).filter(|task| task.text == title)
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec::Vec;

    use crate::{
        Event, EventKind, HostSeq, SessionId, Task, TaskId, TaskStatus,
    };

    use super::TaskBoard;

    #[test]
    fn the_receiver_updates_the_same_task_and_stale_plans_cannot_move_it_back()
    {
        let claude = SessionId(1);
        let codex = SessionId(2);
        let original = Task {
            id: TaskId(7),
            holder: claude,
            text: String::from("Write the integration test"),
            status: TaskStatus::InProgress,
        };
        let native = Task {
            id: TaskId(8),
            holder: codex,
            text: original.handoff_label(),
            status: TaskStatus::Completed,
        };
        let mut board = TaskBoard::new();
        let mut apply = |session, kind| {
            board.apply(&Event {
                session,
                kind,
                seq: HostSeq(0),
            });
            board.clone()
        };
        let _ = apply(
            claude,
            EventKind::TasksChanged {
                tasks: Vec::from([original.clone()]),
            },
        );
        let transferred = apply(
            codex,
            EventKind::TaskHandedOff {
                task: original.clone(),
            },
        );
        assert_eq!(transferred.held(claude), []);
        assert_eq!(
            transferred.held(codex),
            [Task {
                holder: codex,
                status: TaskStatus::Pending,
                ..original.clone()
            }]
        );
        let _ = apply(
            claude,
            EventKind::TasksChanged {
                tasks: Vec::from([original.clone()]),
            },
        );
        let _ = apply(
            codex,
            EventKind::TasksChanged {
                tasks: Vec::from([native]),
            },
        );
        let final_state =
            apply(codex, EventKind::TasksChanged { tasks: Vec::new() });
        assert_eq!(final_state.held(claude), []);
        assert_eq!(
            final_state.held(codex),
            [Task {
                holder: codex,
                status: TaskStatus::Completed,
                ..original
            }]
        );
    }
}
