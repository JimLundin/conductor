//! The checklist an agent keeps of its work.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChecklistEntry {
    pub text: String,
    pub status: ChecklistStatus,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum ChecklistStatus {
    Pending,
    InProgress,
    Completed,
}
