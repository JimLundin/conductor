//! What an agent produces for its session to show, as the session's log
//! carries it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::prelude::*;
use crate::{ToolCallId, ToolPhase};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Output {
    /// A fragment of the assistant message being streamed. The
    /// `AssistantMessage` that follows replaces them.
    AssistantDelta {
        text: String,
    },
    /// The finished text of one assistant message. It replaces the deltas
    /// since the previous message.
    AssistantMessage {
        text: String,
    },
    /// A fragment of the agent's reasoning as it streams. The `Reasoning`
    /// that follows replaces them.
    ReasoningDelta {
        text: String,
    },
    /// The finished text of one stretch of reasoning: the summary the
    /// provider gives, not the model's raw thoughts.
    Reasoning {
        text: String,
    },
    ToolCall {
        call: ToolCallId,
        phase: ToolPhase,
    },
    /// Something went wrong that did not end the session.
    Error {
        message: String,
    },
}
