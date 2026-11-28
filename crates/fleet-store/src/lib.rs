pub mod crypto;
pub mod keys;

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, ensure};
use host::fleet::{
    Certificate, Identity, InviteSecret, Role, SignedConfiguration,
    SignedSnapshot, Snapshot,
};
use iroh_base::{EndpointId, PublicKey, SecretKey};
use project_runtime::transport::SharedCredential;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub const STORE: &str = "fleet.sealed";
pub const STORE_KEY: &str = "fleet-store.key";
pub const HOST_KEY: &str = "host.key";

#[derive(Clone, Serialize, Deserialize)]
pub struct State {
    pub certificate: Certificate,
    pub current: SignedConfiguration,
    pub root_key: Option<[u8; 32]>,
    pub invites: Vec<Invitation>,
    pub revoked: Option<SignedSnapshot>,
    pub applied: BTreeSet<host::registry::InstanceId>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Invitation {
    pub secret: InviteSecret,
    pub expires: u64,
    pub admitted: Option<Certificate>,
}

impl State {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.revoked.is_none()
            && admit(
                &self.current.config.snapshot,
                &self.certificate,
                Role::Host,
            )
            .is_ok()
    }

    pub fn reserve_credentials(&mut self) {
        self.applied
            .extend(self.current.config.instances.0.keys().cloned());
    }

    fn into_project_credential(mut self, reference: &str) -> SharedCredential {
        if !self.is_active() {
            return SharedCredential::Missing;
        }
        self.current
            .config
            .project_credentials
            .remove(reference)
            .map_or(SharedCredential::Missing, |entry| {
                SharedCredential::Stored(entry.into_secret().into())
            })
    }
}

pub fn project_credential(
    data: &Path,
    reference: &str,
) -> std::io::Result<SharedCredential> {
    resolve_project_credential(data, reference).map_err(|_| {
        std::io::Error::other("fleet credential store is unavailable")
    })
}

fn resolve_project_credential(
    data: &Path,
    reference: &str,
) -> anyhow::Result<SharedCredential> {
    let membership = read(data, || {
        let key = existing_key(data, HOST_KEY)?;
        Ok(SecretKey::from_bytes(&key).public())
    })?;
    Ok(
        membership.map_or(SharedCredential::Standalone, |(_, state)| {
            state.into_project_credential(reference)
        }),
    )
}

pub fn read(
    data: &Path,
    identity: impl FnOnce() -> anyhow::Result<EndpointId>,
) -> anyhow::Result<Option<(Zeroizing<[u8; 32]>, State)>> {
    if !keys::exists(data, STORE)? {
        ensure!(
            !keys::exists(data, STORE_KEY)?,
            "missing sealed fleet membership"
        );
        return Ok(None);
    }
    let key = existing_key(data, STORE_KEY)?;
    let state = crypto::unseal(&key, &std::fs::read(data.join(STORE))?)?;
    verify_local(&state, identity()?)?;
    Ok(Some((key, state)))
}

fn existing_key(
    data: &Path,
    name: &str,
) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    keys::read_file(data, name)?
        .map(Zeroizing::new)
        .context("missing fleet key")
}

fn verify_local(state: &State, local: EndpointId) -> anyhow::Result<()> {
    let snapshot = &state.current.config.snapshot;
    let root = PublicKey::from_bytes(&snapshot.fleet)?;
    crypto::verify_config(root, &state.current)?;
    let identity = crypto::open(root, &state.certificate)?;
    ensure!(
        local_identity(&identity, snapshot, local),
        "fleet membership belongs to another host"
    );
    verify_root(state, root, &identity)
}

fn local_identity(
    identity: &Identity,
    snapshot: &Snapshot,
    local: EndpointId,
) -> bool {
    identity.fleet == snapshot.fleet
        && identity.device == *local.as_bytes()
        && identity.role == Role::Host
}

fn verify_root(
    state: &State,
    expected: PublicKey,
    identity: &Identity,
) -> anyhow::Result<()> {
    if let Some(seed) = state.root_key {
        let snapshot = &state.current.config.snapshot;
        ensure!(
            SecretKey::from_bytes(&seed).public() == expected
                && snapshot.root == identity.device,
            "invalid local root key"
        );
    }
    Ok(())
}

pub fn admit(
    snapshot: &Snapshot,
    certificate: &Certificate,
    role: Role,
) -> anyhow::Result<Identity> {
    let identity =
        crypto::open(PublicKey::from_bytes(&snapshot.fleet)?, certificate)?;
    ensure!(
        identity.fleet == snapshot.fleet && identity.role == role,
        "wrong fleet or role"
    );
    ensure!(
        !snapshot.revoked.contains(&identity.device),
        "peer has been revoked"
    );
    if role == Role::Host {
        ensure!(
            snapshot
                .hosts
                .iter()
                .any(|entry| entry.node == identity.device),
            "host is outside the roster"
        );
    }
    Ok(identity)
}
