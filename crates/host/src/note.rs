//! What the host's cores decide is worth logging, as data: `daemon` logs
//! each kind with its fields.

use proto::{InstanceId, Provider};

use crate::prelude::*;
use crate::registry::OwnSignIn;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    /// An instance's CLI runs, offering `models` models.
    Ready {
        path: String,
        version: String,
        own: OwnSignIn,
        models: usize,
    },
    /// An instance's CLI does not run, so the instance is unavailable.
    Unavailable {
        reason: String,
    },
    /// The CLI listed no models, so it offers only its default.
    NoModels {
        reason: String,
    },
    CredentialStored {
        instance: InstanceId,
    },
    /// The instance stays unavailable until its next refresh.
    RefreshFailed {
        instance: InstanceId,
        reason: String,
    },
    InstanceAdded {
        instance: InstanceId,
        provider: Provider,
    },
    InstanceRenamed {
        instance: InstanceId,
    },
    InstanceRemoved {
        instance: InstanceId,
    },
}
