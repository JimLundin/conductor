//! Instances (§6.3): one account of one provider on its host. A host may
//! run several of one provider, such as Claude through a gateway and Claude
//! on a subscription, each with its own credential and its own private
//! home.

use alloc::collections::BTreeMap;
use alloc::string::String;
use core::fmt;

use serde::{Deserialize, Serialize};

use crate::{Availability, Credential, HostPath, ModelId, Provider};

/// An instance by its key, the account of a provider on this host (the
/// node). The provider names the account, not the owner, so it never
/// changes and the instance is never renamed.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct InstanceId {
    pub provider: Provider,
    pub account: Account,
}

/// Who the provider says an instance signs in as.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum Account {
    /// The provider's own sign-in before it has named an account: where a
    /// sign-in runs, and a CLI given on the host's command line, signed in
    /// however it is. Each provider has one.
    Own,
    /// As the provider names it: a subscription's email, or what the
    /// credential's form names, such as a gateway's URL.
    Named(String),
}

/// An instance as its host keeps it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roster(
    #[serde(with = "crate::json::pairs")] pub BTreeMap<InstanceId, Instance>,
);

/// What a client learns of one instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceStatus {
    pub availability: Availability,
}

/// Whether the host can run each of its instances, and how each signs in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instances(
    #[serde(with = "crate::json::pairs")]
    pub  BTreeMap<InstanceId, InstanceStatus>,
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
