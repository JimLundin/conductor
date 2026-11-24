//! A sub-agent an agent starts inside its session.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::prelude::*;
use crate::{ToolCallId, ToolPhase};

/// A sub-agent's id as its provider gave it (Claude's `task_id`).
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
)]
pub struct SubagentId(pub String);

/// One entry of a sub-agent's own transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SubagentActivity {
    Message { text: String },
    Reasoning { text: String },
    ToolCall { call: ToolCallId, phase: ToolPhase },
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum SubagentOutcome {
    Finished,
    Failed,
    Stopped,
}
