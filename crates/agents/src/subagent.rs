use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::prelude::*;
use crate::{ToolCallId, ToolPhase};

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
