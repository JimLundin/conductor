use alloc::string::String;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use agents::SignInCode;

use crate::registry::InstanceId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SignIn {
    pub instance: InstanceId,
    pub step: SignInStep,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SignInStep {
    Starting,
    Open {
        url: String,
        code: Option<String>,
        paste: bool,
    },
    Finishing,
    SignedIn,
    Failed {
        reason: String,
    },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SignInCommand {
    Start {
        instance: InstanceId,
    },
    SubmitCode {
        instance: InstanceId,
        code: SignInCode,
    },
    Cancel {
        instance: InstanceId,
    },
    StartCredential {
        instance: InstanceId,
        kind: String,
        label: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SignInResult {
    Accepted,
    Refused(SignInRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SignInRefusal {
    UnknownInstance,
    InstanceUnavailable { reason: String },
    SignInRunning,
    NoCodeAsked,
    InvalidCredential { reason: String },
    CredentialNotSaved { reason: String },
    InstanceInUse { sessions: u64 },
}
