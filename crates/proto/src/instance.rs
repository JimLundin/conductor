//! Instances (§6.3): one account of one provider on its host. A host may
//! run several of one provider, such as Claude through a gateway and Claude
//! on a subscription, each with its own credential and its own private
//! home.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Account, Availability, Credential, HostPath, ModelId, Provider};

/// An instance by its key, the account of a provider on this host (the
/// node). The provider names the account, not the owner, so it never
/// changes and the instance is never renamed.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
)]
pub struct InstanceId {
    pub provider: Provider,
    pub account: Account,
}

/// An instance as its host keeps it.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub struct Instance {
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
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub struct Roster(
    #[serde(with = "crate::json::pairs")]
    #[schemars(with = "Vec<(InstanceId, Instance)>")]
    pub BTreeMap<InstanceId, Instance>,
);

/// What a client learns of one instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstanceStatus {
    pub availability: Availability,
}

/// Whether the host can run each of its instances, and how each signs in.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub struct Instances(
    #[serde(with = "crate::json::pairs")]
    #[schemars(with = "Vec<(InstanceId, InstanceStatus)>")]
    pub BTreeMap<InstanceId, InstanceStatus>,
);

impl InstanceId {
    /// The provider's own sign-in.
    #[must_use]
    pub const fn own(provider: Provider) -> Self {
        Self {
            provider,
            account: Account::Own,
        }
    }

    #[must_use]
    pub fn named(provider: Provider, account: &str) -> Self {
        Self {
            provider,
            account: Account::Named(String::from(account)),
        }
    }
}

/// The provider and the account, as a person reads it: `claude
/// me@example.com`, or `claude` for its own sign-in.
impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.account {
            Account::Own => f.write_str(self.provider.name()),
            Account::Named(account) => {
                write!(f, "{} {account}", self.provider.name())
            }
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

/// A command that changes the host's instances.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum InstanceCommand {
    /// Stores a credential of `provider` on the host. The account its form
    /// names makes the instance: one that has it takes the credential in
    /// place of its own, and otherwise the host adds the instance and
    /// starts its CLI before it answers. The host never sends it back.
    /// Like every command it needs a client admitted by the ticket's
    /// token, which device certificates replace with the fleet (§6.1).
    SetCredential {
        provider: Provider,
        credential: Credential,
    },
    /// Refused while a session runs on the instance. Its closed sessions
    /// stay, and can't continue. A provider's own sign-in stays; on a host
    /// whose store doesn't open, removing it moves the store aside.
    Remove { instance: InstanceId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum InstanceResult {
    Accepted,
    Refused(InstanceRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum InstanceRefusal {
    /// The host could not store the credential, and kept the one it had.
    CredentialNotSaved {
        reason: String,
    },
    /// The credential is malformed, for this reason, and was not stored.
    InvalidCredential {
        reason: String,
    },
    UnknownInstance,
    /// The instance can't be removed while sessions run on it.
    InstanceInUse {
        sessions: u64,
    },
    /// Only the fleet's root host changes a fleet's instances.
    FleetUnavailable,
}
