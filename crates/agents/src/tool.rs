use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::prelude::*;

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct ToolCallId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ToolPhase {
    Started { tool: ToolKind },
    Finished { outcome: ToolOutcome },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ToolKind {
    Command { command: String },
    FileChange { files: Vec<FileChange> },
    Other { name: String, detail: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileChange {
    pub path: String,
    pub change: Change,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum Change {
    Write,
    Edit,
    Delete,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum ToolOutcome {
    Succeeded,
    Failed,
    Declined,
}
