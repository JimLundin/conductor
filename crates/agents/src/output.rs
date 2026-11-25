use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::prelude::*;
use crate::{ToolCallId, ToolPhase};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Output {
    AssistantDelta { text: String },
    AssistantMessage { text: String },
    ReasoningDelta { text: String },
    Reasoning { text: String },
    ToolCall { call: ToolCallId, phase: ToolPhase },
    Error { message: String },
}
