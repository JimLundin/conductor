use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, ensure};
use host::fleet::{
    Certificate, Configuration, DeviceHello, HostEntry, Identity, Invite,
    InviteSecret, NodeId, Peer, Role, SignedConfiguration, SignedSnapshot,
    Snapshot,
};
use iroh::{EndpointId, PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tokio::sync::{RwLock, watch};
use zeroize::Zeroizing;

use super::crypto;

const STORE: &str = "fleet.sealed";
const INVITE_LIMIT: usize = 64;
const INVITE_SECONDS: u64 = 3600;

#[derive(Clone)]
pub struct Membership {
    path: PathBuf,
    store_key: Arc<Zeroizing<[u8; 32]>>,
    state: Arc<RwLock<State>>,
    versions: watch::Sender<u64>,
    fresh: watch::Sender<bool>,
}

#[derive(Clone, Serialize, Deserialize)]
struct State {
    certificate: Certificate,
    current: SignedConfiguration,
    root_key: Option<[u8; 32]>,
    invites: Vec<Invitation>,
    revoked: Option<SignedSnapshot>,
    applied: BTreeSet<host::registry::InstanceId>,
}

impl State {
    fn reserve_credentials(&mut self) {
        self.applied
            .extend(self.current.config.instances.0.keys().cloned());
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct Recovery {
    pub host_key: [u8; 32],
    pub root_key: [u8; 32],
    pub initial: SignedConfiguration,
}

impl Drop for Recovery {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.host_key.zeroize();
        self.root_key.zeroize();
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Invitation {
    secret: InviteSecret,
    expires: u64,
    admitted: Option<Certificate>,
}

impl core::fmt::Debug for Membership {
    fn fmt(&self, output: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        output.write_str("Membership")
    }
}

impl Membership {
    pub fn open(
        data: &Path,
        local: EndpointId,
    ) -> anyhow::Result<Option<Self>> {
        let path = data.join(STORE);
        let file = match std::fs::read(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        let key = store_key(data)?;
        let state: State = crypto::unseal(&key, &file)?;
        verify_local(&state, local)?;
        Ok(Some(Self::new(path, key, state)))
    }

    pub async fn admit_host(
        &self,
        remote: EndpointId,
        certificate: &Certificate,
    ) -> anyhow::Result<()> {
        let state = self.state.read().await;
        let identity =
            admit(&state.current.config.snapshot, certificate, Role::Host)?;
        drop(state);
        ensure!(
            identity.device == *remote.as_bytes(),
            "certificate does not name the peer"
        );
        Ok(())
    }

    #[must_use]
    pub async fn accepts_work(&self) -> bool {
        let fresh = self.fresh();
        fresh && self.is_active().await
    }

    pub async fn admit_owner(
        &self,
        certificate: &Certificate,
    ) -> anyhow::Result<()> {
        let state = self.state.read().await;
        admit(&state.current.config.snapshot, certificate, Role::Owner)
            .map(drop)
    }

    pub(crate) async fn admit_device(
        &self,
        remote: EndpointId,
        hello: &DeviceHello,
    ) -> anyhow::Result<Identity> {
        use iroh::Signature;

        let state = self.state.read().await;
        let snapshot = &state.current.config.snapshot;
        ensure!(self.fresh(), "waiting to check fleet configuration");
        ensure!(state.revoked.is_none(), "host membership has been revoked");
        let host = admit(snapshot, &state.certificate, Role::Host)?.device;
        let owner = admit(snapshot, &hello.certificate, Role::Owner)?;
        let fleet = snapshot.fleet;
        drop(state);
        let proof = host::fleet::device_proof(&fleet, &host, remote.as_bytes());
        let signature = Signature::from_bytes(
            hello
                .proof
                .as_slice()
                .try_into()
                .context("invalid device proof")?,
        );
        PublicKey::from_bytes(&owner.device)?.verify(&proof, &signature)?;
        Ok(owner)
    }

    #[must_use]
    pub async fn snapshot(&self) -> Snapshot {
        self.state.read().await.current.config.snapshot.clone()
    }

    pub(super) async fn public_snapshot(&self) -> SignedSnapshot {
        let state = self.state.read().await;
        state
            .revoked
            .as_ref()
            .unwrap_or(&state.current.public)
            .clone()
    }

    pub async fn identity(&self) -> anyhow::Result<Identity> {
        let state = self.state.read().await;
        local(&state)
    }

    pub async fn peer(&self) -> Peer {
        let state = self.state.read().await;
        Peer {
            certificate: state.certificate.clone(),
            version: state.current.config.snapshot.version,
        }
    }

    pub(super) async fn revocation_for(
        &self,
        remote: EndpointId,
        certificate: &Certificate,
    ) -> Option<SignedSnapshot> {
        let state = self.state.read().await;
        let config = &state.current;
        let snapshot = &config.config.snapshot;
        let fleet = PublicKey::from_bytes(&snapshot.fleet).ok()?;
        let revoked = snapshot.revoked.contains(remote.as_bytes());
        let public = config.public.clone();
        drop(state);
        let identity = crypto::open(fleet, certificate).ok()?;
        (revoked
            && identity.device == *remote.as_bytes()
            && identity.fleet == *fleet.as_bytes()
            && identity.role == Role::Host)
            .then_some(public)
    }

    pub(super) async fn adopt_revocation(
        &self,
        revoked: SignedSnapshot,
    ) -> anyhow::Result<()> {
        let mut state = self.state.write().await;
        let current = &state.current.config.snapshot;
        let snapshot =
            crypto::open(PublicKey::from_bytes(&current.fleet)?, &revoked)?;
        let device = local(&state)?.device;
        ensure!(
            current.fleet == snapshot.fleet
                && current.root == snapshot.root
                && snapshot.version > current.version
                && snapshot.revoked.contains(&device),
            "invalid fleet revocation"
        );
        let mut updated = state.clone();
        updated.revoked = Some(revoked);
        self.persist(&updated)?;
        self.replace(&mut state, updated);
        drop(state);
        Ok(())
    }

    pub(super) async fn config_after(
        &self,
        version: u64,
    ) -> Option<SignedConfiguration> {
        let state = self.state.read().await;
        let config = &state.current;
        let result =
            (config.config.snapshot.version > version).then(|| config.clone());
        drop(state);
        result
    }

    pub(super) async fn adopt(
        &self,
        candidate: SignedConfiguration,
    ) -> anyhow::Result<bool> {
        let mut state = self.state.write().await;
        if !new_configuration(&state, &candidate)? {
            return Ok(false);
        }
        let mut updated = state.clone();
        updated.current = candidate;
        updated.reserve_credentials();
        self.persist(&updated)?;
        self.replace(&mut state, updated);
        drop(state);
        Ok(true)
    }

    pub(super) async fn invite(&self, now: u64) -> anyhow::Result<Invite> {
        let mut state = self.state.write().await;
        ensure!(
            state.root_key.is_some(),
            "fleet changes require the root host"
        );
        let mut updated = state.clone();
        updated.invites.retain(|invite| invite.expires > now);
        ensure!(
            updated.invites.len() < INVITE_LIMIT,
            "too many active fleet invitations"
        );
        let snapshot = &state.current.config.snapshot;
        let root = snapshot
            .hosts
            .iter()
            .find(|host| host.node == snapshot.root)
            .context("fleet configuration has no root host")?;
        let secret = InviteSecret(crate::random_key()?);
        updated.invites.push(Invitation {
            secret: secret.clone(),
            expires: now.saturating_add(INVITE_SECONDS),
            admitted: None,
        });
        self.persist(&updated)?;
        let invite = Invite {
            fleet: snapshot.fleet,
            root: snapshot.root,
            relay: root.relay.clone(),
            secret,
        };
        self.replace(&mut state, updated);
        drop(state);
        Ok(invite)
    }

    pub(super) async fn join(
        &self,
        remote: EndpointId,
        secret: &InviteSecret,
        mut host: HostEntry,
        now: u64,
    ) -> anyhow::Result<(Certificate, SignedConfiguration)> {
        ensure!(
            host.node == *remote.as_bytes(),
            "host entry does not name the peer"
        );
        let mut state = self.state.write().await;
        let key = root(&state)?;
        let invite = find_invite(&state, secret, now)?;
        let snapshot = &state.current.config.snapshot;
        ensure!(
            !snapshot.revoked.contains(&host.node),
            "host has been revoked"
        );
        let entry = state
            .invites
            .get(invite)
            .context("invitation disappeared")?;
        if let Some(existing) = &entry.admitted {
            ensure!(
                crypto::open(
                    PublicKey::from_bytes(&snapshot.fleet)?,
                    existing
                )?
                .device
                    == host.node,
                "invitation was already used"
            );
            return Ok((existing.clone(), state.current.clone()));
        }
        ensure!(
            host::fleet::capacity_for(snapshot, &host.node),
            "fleet roster is full"
        );
        let certificate = crypto::sign_identity(
            &key,
            &Identity {
                fleet: snapshot.fleet,
                device: host.node,
                name: host.name.clone(),
                role: Role::Host,
            },
        )?;
        host.added_at = now;
        let mut updated = state.clone();
        let mut config = next_config(&state)?;
        config
            .snapshot
            .hosts
            .retain(|entry| entry.node != host.node);
        config.snapshot.hosts.push(host);
        updated.current = crypto::sign_config(&key, config)?;
        updated
            .invites
            .get_mut(invite)
            .context("invitation disappeared")?
            .admitted = Some(certificate.clone());
        self.persist(&updated)?;
        let result = (certificate, updated.current.clone());
        self.replace(&mut state, updated);
        drop(state);
        Ok(result)
    }

    pub(super) async fn enroll(
        &self,
        mut host: HostEntry,
    ) -> anyhow::Result<(Certificate, SignedConfiguration)> {
        let mut state = self.state.write().await;
        let key = root(&state)?;
        let snapshot = &state.current.config.snapshot;
        ensure!(
            !snapshot.revoked.contains(&host.node),
            "host has been revoked"
        );
        if let Some(known) =
            snapshot.hosts.iter().find(|entry| entry.node == host.node)
        {
            host.added_at = known.added_at;
        }
        let certificate = crypto::sign_identity(
            &key,
            &Identity {
                fleet: snapshot.fleet,
                device: host.node,
                name: host.name.clone(),
                role: Role::Host,
            },
        )?;
        if snapshot.hosts.contains(&host) {
            return Ok((certificate, state.current.clone()));
        }
        ensure!(
            host::fleet::capacity_for(snapshot, &host.node),
            "fleet roster is full"
        );
        let mut updated = state.clone();
        let mut config = next_config(&state)?;
        config
            .snapshot
            .hosts
            .retain(|entry| entry.node != host.node);
        config.snapshot.hosts.push(host);
        updated.current = crypto::sign_config(&key, config)?;
        self.persist(&updated)?;
        let result = (certificate, updated.current.clone());
        self.replace(&mut state, updated);
        drop(state);
        Ok(result)
    }

    pub(super) fn install_admitted(
        data: &Path,
        local: EndpointId,
        remote: EndpointId,
        certificate: Certificate,
        signed: SignedConfiguration,
    ) -> anyhow::Result<Self> {
        let snapshot = &signed.config.snapshot;
        ensure!(
            snapshot.root == *remote.as_bytes(),
            "fleet offer does not name the admission peer"
        );
        crypto::verify_config(
            PublicKey::from_bytes(&snapshot.fleet)?,
            &signed,
        )?;
        ensure!(
            admit(snapshot, &certificate, Role::Host)?.device
                == *local.as_bytes(),
            "certificate does not name this host"
        );
        Self::install(
            data,
            State {
                certificate,
                current: signed,
                root_key: None,
                invites: Vec::new(),
                revoked: None,
                applied: BTreeSet::new(),
            },
        )
    }

    pub(super) async fn revoke(&self, device: NodeId) -> anyhow::Result<()> {
        let mut state = self.state.write().await;
        let key = root(&state)?;
        let mut config = next_config(&state)?;
        ensure!(
            device != config.snapshot.root,
            "cannot revoke the root host"
        );
        if config.snapshot.revoked.contains(&device) {
            return Ok(());
        }
        config.snapshot.hosts.retain(|entry| entry.node != device);
        config.snapshot.revoked.push(device);
        let mut updated = state.clone();
        updated.current = crypto::sign_config(&key, config)?;
        self.persist(&updated)?;
        self.replace(&mut state, updated);
        drop(state);
        Ok(())
    }

    pub(super) async fn applied(&self) -> BTreeSet<host::registry::InstanceId> {
        self.state.read().await.applied.clone()
    }

    pub(super) async fn record_applied(
        &self,
        applied: &BTreeSet<host::registry::InstanceId>,
    ) -> anyhow::Result<()> {
        let mut state = self.state.write().await;
        if &state.applied != applied {
            let mut updated = state.clone();
            updated.applied.clone_from(applied);
            self.persist(&updated)?;
            self.replace(&mut state, updated);
        }
        drop(state);
        Ok(())
    }

    pub(super) async fn credentials(&self) -> host::registry::Roster {
        self.state.read().await.current.config.instances.clone()
    }

    pub(super) async fn change_credentials(
        &self,
        change: impl FnOnce(&mut host::registry::Roster),
    ) -> anyhow::Result<()> {
        let mut state = self.state.write().await;
        let key = root(&state)?;
        let mut config = next_config(&state)?;
        change(&mut config.instances);
        if config.instances == state.current.config.instances {
            return Ok(());
        }
        let mut updated = state.clone();
        updated.current = crypto::sign_config(&key, config)?;
        updated.reserve_credentials();
        self.persist(&updated)?;
        self.replace(&mut state, updated);
        drop(state);
        Ok(())
    }

    pub(crate) async fn is_active(&self) -> bool {
        let state = self.state.read().await;
        state.revoked.is_none()
            && admit(
                &state.current.config.snapshot,
                &state.certificate,
                Role::Host,
            )
            .is_ok()
    }

    pub(super) async fn sign_owner(
        &self,
        device: NodeId,
        name: String,
    ) -> anyhow::Result<Certificate> {
        let state = self.state.read().await;
        let key = root(&state)?;
        let snapshot = &state.current.config.snapshot;
        ensure!(
            !snapshot.revoked.contains(&device),
            "device has been revoked"
        );
        let fleet = snapshot.fleet;
        drop(state);
        PublicKey::from_bytes(&device)?;
        crypto::sign_identity(
            &key,
            &Identity {
                fleet,
                device,
                name,
                role: Role::Owner,
            },
        )
    }

    pub(super) fn initialize(
        data: &Path,
        host: HostEntry,
        host_key: &SecretKey,
        backup: &Path,
        passphrase: &[u8],
    ) -> anyhow::Result<Self> {
        ensure!(
            !data.join(STORE).exists(),
            "this host already has fleet membership"
        );
        let seed = Zeroizing::new(crate::random_key()?);
        let root = SecretKey::from_bytes(&seed);
        let fleet = *root.public().as_bytes();
        let identity = Identity {
            fleet,
            device: host.node,
            name: host.name.clone(),
            role: Role::Host,
        };
        let config = Configuration {
            snapshot: Snapshot {
                fleet,
                root: host.node,
                version: 1,
                hosts: vec![host],
                revoked: Vec::new(),
            },
            instances: host::registry::Roster::default(),
        };
        let state = State {
            certificate: crypto::sign_identity(&root, &identity)?,
            current: crypto::sign_config(&root, config)?,
            root_key: Some(*seed),
            invites: Vec::new(),
            revoked: None,
            applied: BTreeSet::new(),
        };
        let recovery = Recovery {
            host_key: host_key.to_bytes(),
            root_key: *seed,
            initial: state.current.clone(),
        };
        crypto::backup(backup, passphrase, &recovery)?;
        Self::install(data, state)
    }

    pub(super) fn install_joined(
        data: &Path,
        local: EndpointId,
        invite: &Invite,
        certificate: Certificate,
        signed: SignedConfiguration,
    ) -> anyhow::Result<Self> {
        let snapshot = &signed.config.snapshot;
        ensure!(
            snapshot.fleet == invite.fleet && snapshot.root == invite.root,
            "unexpected fleet"
        );
        crypto::verify_config(PublicKey::from_bytes(&invite.fleet)?, &signed)?;
        ensure!(
            admit(snapshot, &certificate, Role::Host)?.device
                == *local.as_bytes(),
            "certificate does not name this host"
        );
        Self::install(
            data,
            State {
                certificate,
                current: signed,
                root_key: None,
                invites: Vec::new(),
                revoked: None,
                applied: BTreeSet::new(),
            },
        )
    }

    pub(super) async fn export_config(
        &self,
        path: &Path,
        passphrase: &[u8],
    ) -> anyhow::Result<()> {
        let state = self.state.read().await;
        crypto::backup(path, passphrase, &state.current)
    }

    pub(super) fn restore_root(
        data: &Path,
        recovery: &Recovery,
        latest: Option<SignedConfiguration>,
    ) -> anyhow::Result<Self> {
        let root = SecretKey::from_bytes(&recovery.root_key);
        let local = SecretKey::from_bytes(&recovery.host_key).public();
        let config = latest.unwrap_or_else(|| recovery.initial.clone());
        crypto::verify_config(root.public(), &config)?;
        let snapshot = &config.config.snapshot;
        ensure!(
            matches_recovery(snapshot, root.public(), local),
            "this configuration belongs to another fleet"
        );
        let host = snapshot
            .hosts
            .iter()
            .find(|entry| entry.node == snapshot.root)
            .context("missing root host")?;
        let certificate = crypto::sign_identity(
            &root,
            &Identity {
                fleet: snapshot.fleet,
                device: snapshot.root,
                name: host.name.clone(),
                role: Role::Host,
            },
        )?;
        restore_identity(data, &recovery.host_key)?;
        Self::install(
            data,
            State {
                certificate,
                current: config,
                root_key: Some(recovery.root_key),
                invites: Vec::new(),
                revoked: None,
                applied: BTreeSet::new(),
            },
        )
    }

    fn install(data: &Path, mut state: State) -> anyhow::Result<Self> {
        state.reserve_credentials();
        let key = store_key(data)?;
        let path = data.join(STORE);
        crypto::create_file(&path, &crypto::seal(&key, &state)?)?;
        let installed = Self::new(path, key, state);
        installed.mark_fresh();
        Ok(installed)
    }

    fn new(path: PathBuf, key: Zeroizing<[u8; 32]>, state: State) -> Self {
        let (versions, _) =
            watch::channel(state.current.config.snapshot.version);
        let (fresh, _) = watch::channel(state.root_key.is_some());
        Self {
            path,
            store_key: Arc::new(key),
            state: Arc::new(RwLock::new(state)),
            versions,
            fresh,
        }
    }

    fn fresh(&self) -> bool {
        *self.fresh.borrow()
    }

    pub(super) fn mark_fresh(&self) {
        if self
            .fresh
            .send_if_modified(|ready| !std::mem::replace(ready, true))
        {
            self.versions.send_modify(|_| {});
        }
    }

    pub(crate) fn versions(&self) -> watch::Receiver<u64> {
        self.versions.subscribe()
    }

    fn replace(&self, state: &mut State, updated: State) {
        let current = &updated.current.config.snapshot;
        let version = updated
            .revoked
            .as_ref()
            .and_then(|revoked| {
                let key = PublicKey::from_bytes(&current.fleet).ok()?;
                crypto::open(key, revoked).ok()
            })
            .map_or(current.version, |revoked| revoked.version);
        *state = updated;
        self.versions.send_if_modified(|observed| {
            if *observed == version {
                false
            } else {
                *observed = version;
                true
            }
        });
    }

    fn persist(&self, state: &State) -> anyhow::Result<()> {
        crate::write_private(
            &self.path,
            &crypto::seal(&self.store_key, state)?,
        )?;
        Ok(())
    }
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

fn local(state: &State) -> anyhow::Result<Identity> {
    crypto::open(
        PublicKey::from_bytes(&state.current.config.snapshot.fleet)?,
        &state.certificate,
    )
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

fn new_configuration(
    state: &State,
    candidate: &SignedConfiguration,
) -> anyhow::Result<bool> {
    use std::cmp::Ordering;

    let previous = &state.current.config.snapshot;
    let next = &candidate.config.snapshot;
    ensure!(host::fleet::same_fleet(previous, next), "other fleet");
    crypto::verify_config(PublicKey::from_bytes(&previous.fleet)?, candidate)?;
    match host::fleet::revision_order(previous.version, next.version) {
        Ordering::Less => Ok(false),
        Ordering::Equal => {
            ensure!(
                candidate == &state.current,
                "fleet configurations forked at one version"
            );
            Ok(false)
        }
        Ordering::Greater => {
            ensure!(
                state.root_key.is_none(),
                "the fleet root is the only configuration writer"
            );
            ensure!(
                host::fleet::retains_revocations(previous, next),
                "configuration removes a revocation"
            );
            Ok(true)
        }
    }
}

fn find_invite(
    state: &State,
    secret: &InviteSecret,
    now: u64,
) -> anyhow::Result<usize> {
    state
        .invites
        .iter()
        .position(|entry| {
            entry.expires > now && bool::from(entry.secret.0.ct_eq(&secret.0))
        })
        .context("invalid fleet invitation")
}

fn matches_recovery(
    snapshot: &Snapshot,
    fleet: PublicKey,
    local: EndpointId,
) -> bool {
    snapshot.fleet == *fleet.as_bytes() && snapshot.root == *local.as_bytes()
}

fn restore_identity(data: &Path, key: &[u8; 32]) -> anyhow::Result<()> {
    let key_path = data.join("host.key");
    ensure!(
        !data.join(STORE).exists(),
        "restore into an empty data directory"
    );
    if let Some(text) = crate::read_if_exists(&key_path)? {
        ensure!(
            host::secret::stored(Some(&text))
                == host::secret::Secret::Key(*key),
            "the data directory holds a different host key"
        );
    } else {
        let bytes = Zeroizing::new(data_encoding::HEXLOWER.encode(key));
        crypto::create_file(&key_path, bytes.as_bytes())?;
    }
    Ok(())
}

fn store_key(data: &Path) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    crate::load_or_create_secret(data, "fleet-store.key").map(Zeroizing::new)
}

fn admit(
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

fn root(state: &State) -> anyhow::Result<SecretKey> {
    let seed = state
        .root_key
        .as_ref()
        .context("fleet changes require the root host")?;
    Ok(SecretKey::from_bytes(seed))
}

fn next_config(state: &State) -> anyhow::Result<Configuration> {
    let mut config = state.current.config.clone();
    config.snapshot.version = config
        .snapshot
        .version
        .checked_add(1)
        .context("fleet version exhausted")?;
    Ok(config)
}

pub(super) fn clock() -> anyhow::Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}
