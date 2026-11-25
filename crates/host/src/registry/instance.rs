//! Instances (§6.3): one account of one provider on its host. A host may
//! run several of one provider, such as Claude through a gateway and Claude
//! on a subscription, each with its own credential and its own private
//! home. The roster is the instances a host keeps; what a client learns of
//! each is whether the host can run it, and the catalog says what each
//! provider's plugin describes.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use agents::{
    Account, Auth, Credential, ModelId, Models, Provider, ProviderInfo,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
    pub cli: Option<Cli>,
}

/// A CLI an instance runs instead of its provider's pinned one, where the
/// host's OS finds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Cli(pub String);

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Availability {
    Available {
        auth: Auth,
        models: Models,
    },
    /// The host can't run this instance's sessions, for this reason.
    Unavailable {
        reason: String,
    },
}

/// What the host's plugins describe, by provider.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub struct Catalog(pub BTreeMap<Provider, ProviderInfo>);

impl Catalog {
    /// `None` when the host runs no plugin for `provider`.
    #[must_use]
    pub fn of(&self, provider: &Provider) -> Option<&ProviderInfo> {
        self.0.get(provider)
    }

    /// What the app calls `provider`: its plugin's label, else its name.
    #[must_use]
    pub fn label(&self, provider: &Provider) -> String {
        self.of(provider).map_or_else(
            || String::from(provider.name()),
            |info| info.label.clone(),
        )
    }
}

/// The note to show for an instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderNote {
    Nothing,
    /// The host can't run it, for this reason.
    Unavailable(String),
    /// What the provider's plugin says of it, such as what its agent does
    /// without asking.
    Says(String),
}

impl Availability {
    /// `info` and `availability` are `None` until the host has said.
    #[must_use]
    pub fn note(
        info: Option<&ProviderInfo>,
        availability: Option<&Self>,
    ) -> ProviderNote {
        match (availability, info.and_then(|known| known.note.clone())) {
            (Some(Self::Unavailable { reason }), _) => {
                ProviderNote::Unavailable(reason.clone())
            }
            (Some(Self::Available { .. }) | None, Some(text)) => {
                ProviderNote::Says(text)
            }
            (Some(Self::Available { .. }) | None, None) => {
                ProviderNote::Nothing
            }
        }
    }

    /// The models the host offers when it can run the instance.
    #[must_use]
    pub fn models(&self) -> Models {
        match self {
            Self::Available { models, .. } => models.clone(),
            Self::Unavailable { .. } => Models::none(),
        }
    }
}

#[cfg(test)]
mod tests;
