mod enrollment;
pub mod frame;
mod protocol;

use alloc::collections::BTreeSet;

use agents::Credential;

pub use enrollment::{EnrollmentToken, WrongEnrollmentToken};
pub use protocol::{
    ALPN, Certificate, Configuration, DeviceHello, FleetCommand, FleetResult,
    HostEntry, Identity, Invite, InviteSecret, NodeId, PairRequest, Pairing,
    Payload, Peer, ProjectCredential, Reply, Request, Role, Signed,
    SignedConfiguration, SignedSnapshot, Snapshot, Sync, TextError,
    decode_saved, device_proof, encode_saved,
};

use crate::registry::{
    Instance, InstanceCommand, InstanceId, InstanceRefusal, InstanceResult,
    Roster,
};

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
pub const fn applied(
    command: &InstanceCommand,
    result: &InstanceResult,
) -> bool {
    matches!(result, InstanceResult::Accepted)
        || matches!(
            (command, result),
            (
                InstanceCommand::Remove { .. },
                InstanceResult::Refused(InstanceRefusal::UnknownInstance)
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
