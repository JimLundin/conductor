//! Fleet credential policy and configuration ordering.

use alloc::collections::BTreeSet;
use serde::{Deserialize, Serialize};

pub mod acceptance;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Desired {
    pub version: u64,
    pub revision: proto::fleet::Revision,
    pub credentials: Roster,
    pub owned: BTreeSet<InstanceId>,
}

use proto::fleet::{NodeId, Snapshot};
use proto::{
    CommandResult, Credential, Instance, InstanceId, Rejection, Roster,
};

use crate::serve::Configure;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sharing {
    Local,
    Shared,
    OnePerProvider,
}

pub fn store(
    roster: &mut Roster,
    id: InstanceId,
    credential: &Credential,
    sharing: Sharing,
) {
    if sharing == Sharing::OnePerProvider {
        roster.0.retain(|key, entry| {
            key.provider != id.provider
                || entry
                    .credential
                    .as_ref()
                    .is_none_or(|current| current.kind != credential.kind)
        });
    }
    roster.0.insert(
        id,
        Instance {
            credential: Some(credential.clone()),
            ..Instance::default()
        },
    );
}

#[must_use]
pub const fn applied(command: &Configure, result: &CommandResult) -> bool {
    matches!(result, CommandResult::Instances(_))
        || matches!(
            (command, result),
            (
                Configure::Remove(_),
                CommandResult::Rejected(Rejection::UnknownInstance)
            )
        )
}

#[must_use]
pub fn same_fleet(previous: &Snapshot, next: &Snapshot) -> bool {
    previous.fleet == next.fleet && previous.root == next.root
}

#[must_use]
pub fn capacity_for(snapshot: &Snapshot, node: &NodeId) -> bool {
    snapshot.hosts.len() < 32
        || snapshot.hosts.iter().any(|host| &host.node == node)
}

#[must_use]
pub fn retains_revocations(previous: &Snapshot, next: &Snapshot) -> bool {
    previous.revoked.iter().all(|id| next.revoked.contains(id))
}

#[must_use]
pub fn revision_order(current: u64, incoming: u64) -> core::cmp::Ordering {
    incoming.cmp(&current)
}

#[must_use]
pub fn changed_providers(
    previous: Option<&proto::fleet::Revision>,
    next: &proto::fleet::Revision,
) -> BTreeSet<proto::Provider> {
    next.pins
        .0
        .keys()
        .chain(next.settings.models.keys())
        .chain(next.settings.flags.keys())
        .chain(previous.into_iter().flat_map(|before| {
            before
                .pins
                .0
                .keys()
                .chain(before.settings.models.keys())
                .chain(before.settings.flags.keys())
        }))
        .copied()
        .filter(|provider| {
            previous.is_none_or(|before| {
                before.pins.get(*provider) != next.pins.get(*provider)
                    || before.settings.models.get(provider)
                        != next.settings.models.get(provider)
                    || before.settings.flags.get(provider)
                        != next.settings.flags.get(provider)
            })
        })
        .collect()
}
