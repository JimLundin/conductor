use alloc::collections::BTreeMap;

use proto::{Catalog, CommandId, Credential, InstanceId, Rejection, Roster};
use serde::{Deserialize, Serialize};

use super::Sharing;
use crate::serve::Configure;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mutation {
    Set {
        instance: InstanceId,
        credential: Credential,
        sharing: Sharing,
    },
    Remove(InstanceId),
}

impl Mutation {
    #[must_use]
    pub fn apply(&self, current: &Roster) -> Roster {
        let mut next = current.clone();
        match self {
            Self::Set {
                instance,
                credential,
                sharing,
            } => {
                super::store(&mut next, instance.clone(), credential, *sharing);
            }
            Self::Remove(instance) => {
                next.0.remove(instance);
            }
        }
        next
    }
}

/// Resolves generic credential forms before any shared state is changed.
/// # Errors
/// The credential does not match the provider's declared form.
pub fn admit(
    command: &Configure,
    catalog: &Catalog,
    current: &Roster,
    sharing: Sharing,
) -> Result<Option<Mutation>, Rejection> {
    match command {
        Configure::Retry(_) => Ok(None),
        Configure::SetCredential {
            provider,
            credential,
        } => {
            let account = catalog
                .of(*provider)
                .ok_or(Rejection::UnknownInstance)?
                .account(credential)
                .map_err(|reason| Rejection::InvalidCredential { reason })?;
            Ok((sharing != Sharing::Local).then(|| Mutation::Set {
                instance: InstanceId {
                    provider: *provider,
                    account,
                },
                credential: credential.clone(),
                sharing,
            }))
        }
        Configure::Remove(instance) => Ok(current
            .of(instance)
            .map(|_| Mutation::Remove(instance.clone()))),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Acceptance {
    pub fingerprint: [u8; 32],
    pub version: u64,
}

pub type Acceptances = BTreeMap<CommandId, Acceptance>;

#[must_use]
pub const fn written(
    durable: bool,
    uncertain: bool,
) -> Option<proto::CommandResult> {
    match (durable, uncertain) {
        (true, _) => Some(proto::CommandResult::Accepted),
        (false, false) => {
            Some(proto::CommandResult::Rejected(Rejection::FleetUnavailable))
        }
        (false, true) => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replay {
    New,
    Accepted(u64),
    Conflict,
}

impl Replay {
    #[must_use]
    pub const fn result(self) -> Option<proto::CommandResult> {
        match self {
            Self::New => None,
            Self::Accepted(_) => Some(proto::CommandResult::Accepted),
            Self::Conflict => Some(proto::CommandResult::Rejected(
                Rejection::FleetUnavailable,
            )),
        }
    }
}

#[must_use]
pub fn replay(
    receipts: &Acceptances,
    id: CommandId,
    fingerprint: [u8; 32],
) -> Replay {
    match receipts.get(&id) {
        None => Replay::New,
        Some(receipt) if receipt.fingerprint == fingerprint => {
            Replay::Accepted(receipt.version)
        }
        Some(_) => Replay::Conflict,
    }
}
