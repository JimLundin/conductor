//! A session's tasks, kept in step with its agent's checklist. Each update
//! from the agent is the whole list; the host keeps its own ids across them.

use agents::PlanStep;
use proto::{SessionId, Task, TaskId};

/// The tasks `holder` holds once its agent rewrote the checklist as `steps`.
/// A step keeps the id of the first task with its text that no earlier
/// step took; failing that, when the list kept its length, of the task at
/// its position, unless a step took that one by its text; otherwise it is
/// a new task. A task no step keeps is dropped.
pub(crate) fn synced(
    held: &[Task],
    steps: Vec<PlanStep>,
    holder: SessionId,
) -> Vec<Task> {
    let ids = kept(held, &steps);
    steps
        .into_iter()
        .zip(ids)
        .map(|(step, id)| Task {
            id: id.unwrap_or_else(|| TaskId(ulid::Ulid::new().into())),
            text: step.text,
            status: step.status,
            holder,
        })
        .collect()
}

/// Each step's task id, if it keeps one.
fn kept(held: &[Task], steps: &[PlanStep]) -> Vec<Option<TaskId>> {
    let by_text =
        steps
            .iter()
            .fold(Vec::<Option<usize>>::new(), |mut taken, step| {
                let index = held
                    .iter()
                    .enumerate()
                    .find(|&(index, task)| {
                        task.text == step.text && !taken.contains(&Some(index))
                    })
                    .map(|(index, _)| index);
                taken.push(index);
                taken
            });
    by_text
        .iter()
        .enumerate()
        .map(|(position, index)| {
            index
                .or_else(|| {
                    Some(position).filter(|&position| {
                        steps.len() == held.len()
                            && !by_text.contains(&Some(position))
                    })
                })
                .and_then(|index| held.get(index))
                .map(|task| task.id)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use agents::PlanStep;
    use proto::{SessionId, Task, TaskId, TaskStatus};

    use super::{kept, synced};

    fn held(tasks: &[(u128, &str)]) -> Vec<Task> {
        tasks
            .iter()
            .map(|&(id, text)| Task {
                id: TaskId(id),
                text: String::from(text),
                status: TaskStatus::Pending,
                holder: SessionId(7),
            })
            .collect()
    }

    fn steps(texts: &[&str]) -> Vec<PlanStep> {
        texts
            .iter()
            .map(|&text| PlanStep {
                text: String::from(text),
                status: TaskStatus::Pending,
            })
            .collect()
    }

    #[test]
    fn a_step_keeps_its_task_by_text_then_by_position_in_a_list_of_the_same_length()
     {
        let a_b_c = held(&[(1, "a"), (2, "b"), (3, "c")]);
        assert_eq!(
            [
                kept(&a_b_c, &steps(&["a", "b", "c"])),
                kept(&a_b_c, &steps(&["c", "a"])),
                kept(&a_b_c, &steps(&["a", "b2", "c"])),
                kept(&a_b_c, &steps(&["a", "c2"])),
                kept(&a_b_c, &steps(&["a", "b2", "c", "d"])),
                kept(&a_b_c, &steps(&["x", "a"])),
                kept(&held(&[(1, "a"), (2, "a")]), &steps(&["a", "a", "a"])),
                kept(&[], &steps(&["a"])),
            ],
            [
                Vec::from([Some(TaskId(1)), Some(TaskId(2)), Some(TaskId(3))]),
                Vec::from([Some(TaskId(3)), Some(TaskId(1))]),
                Vec::from([Some(TaskId(1)), Some(TaskId(2)), Some(TaskId(3))]),
                Vec::from([Some(TaskId(1)), None]),
                Vec::from([Some(TaskId(1)), None, Some(TaskId(3)), None]),
                Vec::from([None, Some(TaskId(1))]),
                Vec::from([Some(TaskId(1)), Some(TaskId(2)), None]),
                Vec::from([None]),
            ]
        );
    }

    #[test]
    fn synced_tasks_take_the_steps_text_status_and_holder() {
        let done = PlanStep {
            text: String::from("b"),
            status: TaskStatus::Completed,
        };
        let tasks = synced(
            &held(&[(1, "a"), (2, "b")]),
            Vec::from([done]),
            SessionId(7),
        );
        assert_eq!(
            tasks,
            [Task {
                id: TaskId(2),
                text: String::from("b"),
                status: TaskStatus::Completed,
                holder: SessionId(7),
            }]
        );
    }
}
