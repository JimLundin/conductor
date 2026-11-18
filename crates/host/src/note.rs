//! What the host's cores decide is worth logging, as data: `daemon` logs
//! each kind with its fields.

use proto::InstanceId;

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
    /// A credential or a sign-in named an account the host had no
    /// instance of.
    InstanceAdded {
        instance: InstanceId,
    },
    InstanceRemoved {
        instance: InstanceId,
    },
    /// A sign-in from the app took this step, named without its link or
    /// code.
    SignIn {
        instance: InstanceId,
        step: String,
    },
}
