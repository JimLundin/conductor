use crate::registry::InstanceId;

use crate::prelude::*;
use crate::registry::OwnSignIn;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    Ready {
        path: String,
        version: String,
        own: OwnSignIn,
        models: usize,
    },
    Unavailable {
        reason: String,
    },
    NoModels {
        reason: String,
    },
    CredentialStored {
        instance: InstanceId,
    },
    RefreshFailed {
        instance: InstanceId,
        reason: String,
    },
    InstanceAdded {
        instance: InstanceId,
    },
    InstanceRemoved {
        instance: InstanceId,
    },
    SignIn {
        instance: InstanceId,
        step: String,
    },
}
