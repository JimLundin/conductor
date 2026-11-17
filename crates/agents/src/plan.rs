use proto::TaskStatus;

use crate::prelude::*;

/// One step of an agent's checklist, as its CLI wrote it. Each update
/// carries the whole list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    pub text: String,
    pub status: TaskStatus,
}
