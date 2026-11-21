//! Fleet credential policy and configuration ordering.

use alloc::collections::BTreeSet;

use proto::fleet::{NodeId, Snapshot};
use proto::{
    CommandResult, Credential, Instance, InstanceId, Rejection, Roster,
};

use crate::serve::Configure;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

pub fn removal_plan<'a>(
    known: &'a BTreeSet<InstanceId>,
    current: &'a Roster,
) -> impl Iterator<Item = &'a InstanceId> {
    known.iter().filter(|id| !current.0.contains_key(*id))
}

#[must_use]
pub fn wants_update(
    applied: &Roster,
    id: &InstanceId,
    instance: &Instance,
) -> bool {
    applied.of(id) != Some(instance)
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
