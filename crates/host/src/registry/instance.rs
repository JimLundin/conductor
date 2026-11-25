use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use agents::{
    Account, Auth, Credential, ModelId, Models, Provider, ProviderInfo,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub struct Instance {
    pub credential: Option<Credential>,
    pub model: Option<ModelId>,
    pub cli: Option<Cli>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Cli(pub String);

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub struct Roster(
    #[serde(with = "crate::json::pairs")]
    #[schemars(with = "Vec<(InstanceId, Instance)>")]
    pub BTreeMap<InstanceId, Instance>,
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstanceStatus {
    pub availability: Availability,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub struct Instances(
    #[serde(with = "crate::json::pairs")]
    #[schemars(with = "Vec<(InstanceId, InstanceStatus)>")]
    pub BTreeMap<InstanceId, InstanceStatus>,
);

impl InstanceId {
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
    #[must_use]
    pub fn of(&self, id: &InstanceId) -> Option<&InstanceStatus> {
        self.0.get(id)
    }

    #[must_use]
    pub fn availability(&self, id: &InstanceId) -> Option<&Availability> {
        self.of(id).map(|status| &status.availability)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum InstanceCommand {
    SetCredential {
        provider: Provider,
        credential: Credential,
    },
    Remove {
        instance: InstanceId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum InstanceResult {
    Accepted,
    Refused(InstanceRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum InstanceRefusal {
    CredentialNotSaved { reason: String },
    InvalidCredential { reason: String },
    UnknownInstance,
    InstanceInUse { sessions: u64 },
    FleetUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Availability {
    Available { auth: Auth, models: Models },
    Unavailable { reason: String },
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub struct Catalog(pub BTreeMap<Provider, ProviderInfo>);

impl Catalog {
    #[must_use]
    pub fn of(&self, provider: &Provider) -> Option<&ProviderInfo> {
        self.0.get(provider)
    }

    #[must_use]
    pub fn label(&self, provider: &Provider) -> String {
        self.of(provider).map_or_else(
            || String::from(provider.name()),
            |info| info.label.clone(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderNote {
    Nothing,
    Unavailable(String),
    Says(String),
}

impl Availability {
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
