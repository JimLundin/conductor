//! Instances (§6.3): a named, configured use of a provider. A host may run
//! several of one provider, such as Claude through a gateway and Claude on a
//! subscription, each with its own credential and its own private home.

use alloc::collections::BTreeMap;
use alloc::string::String;

use serde::{Deserialize, Serialize};

use crate::{Availability, Credential, HostPath, ModelId, Provider};

/// An instance, by the id its host gave it. The id is a slug of lowercase
/// letters, digits and dashes, and names the instance's private home, so it
/// never changes.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct InstanceId(pub String);

/// An instance as its host keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub label: String,
    pub provider: Provider,
    /// `None` leaves the CLI to sign in on its own, in the instance's home.
    pub credential: Option<Credential>,
    /// The model its sessions get when they name none and the credential
    /// names none; `None` takes the provider's (`--model`).
    pub model: Option<ModelId>,
    /// A CLI to run instead of the pinned one; `None` takes the provider's
    /// (`--cli`).
    pub cli: Option<HostPath>,
}

/// The instances a host has, with their credentials.
///
/// This is the unit the fleet config (§6.5) carries to every host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roster(pub BTreeMap<InstanceId, Instance>);

/// What a client learns of one instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceStatus {
    pub label: String,
    pub provider: Provider,
    pub availability: Availability,
}

/// Whether the host can run each of its instances, and how each signs in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instances(pub BTreeMap<InstanceId, InstanceStatus>);

impl InstanceId {
    #[must_use]
    pub fn named(id: &str) -> Self {
        Self(String::from(id))
    }
}

impl Instance {
    /// A new instance of `provider`, with no credential and the provider's
    /// settings.
    #[must_use]
    pub const fn new(label: String, provider: Provider) -> Self {
        Self {
            label,
            provider,
            credential: None,
            model: None,
            cli: None,
        }
    }
}

impl Roster {
    #[must_use]
    pub fn of(&self, id: &InstanceId) -> Option<&Instance> {
        self.0.get(id)
    }
}

impl Instances {
    /// `None` when the host has no such instance.
    #[must_use]
    pub fn of(&self, id: &InstanceId) -> Option<&InstanceStatus> {
        self.0.get(id)
    }

    #[must_use]
    pub fn availability(&self, id: &InstanceId) -> Option<&Availability> {
        self.of(id).map(|status| &status.availability)
    }
}
