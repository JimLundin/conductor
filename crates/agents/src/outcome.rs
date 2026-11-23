//! How an agent's turn, and its process, ended.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum TurnOutcome {
    Completed,
    Interrupted,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum CloseReason {
    FailedToStart {
        message: String,
    },
    Exited {
        status: String,
    },
    /// The host stopped the agent on purpose, for this reason.
    Stopped {
        reason: String,
    },
}
