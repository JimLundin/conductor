//! What the host's cores decide is worth logging, as data: `daemon` logs
//! each kind with its fields.

use proto::Provider;

use crate::prelude::*;
use crate::registry::OwnSignIn;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    /// A provider's CLI runs, offering `models` models.
    Ready {
        path: String,
        version: String,
        own: OwnSignIn,
        models: usize,
    },
    /// A provider's CLI does not run, so the provider is unavailable.
    Unavailable {
        reason: String,
    },
    /// The CLI listed no models, so it offers only its default.
    NoModels {
        reason: String,
    },
    CredentialStored {
        provider: Provider,
    },
    /// The provider stays unavailable until its next refresh.
    RefreshFailed {
        provider: Provider,
        reason: String,
    },
}
