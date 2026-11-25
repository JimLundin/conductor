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
    FailedToStart { message: String },
    Exited { status: String },
    Stopped { reason: String },
}
