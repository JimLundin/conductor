mod credentials;
mod crypto;
mod member;
mod peer;

pub(crate) use peer::{Protocol, follow_when_enrolled, refresh};

pub use credentials::Sharing;
pub(crate) use credentials::{configure, follow_credentials, share_credential};
pub use member::Membership;
pub(crate) type Slot = tokio::sync::watch::Sender<Option<Membership>>;

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use host::fleet::{HostEntry, Invite, PairRequest, Pairing, Reply};
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointId, RelayMode};

use member::clock;

/// Creates the local fleet root and a passphrase-sealed recovery file.
///
/// # Errors
///
/// If a fleet already exists, the relay is invalid, or recovery data cannot be stored.
pub fn initialize(
    data: Option<PathBuf>,
    name: String,
    relay: String,
    backup: &Path,
    passphrase: &[u8],
) -> anyhow::Result<()> {
    let data_dir = crate::start::data_dir_or_default(data)?;
    let _exclusive = exclusive(&data_dir)?;
    let host = host_entry(&data_dir, name, relay)?;
    let host_key = crate::load_or_create_key(&data_dir)?;
    Membership::initialize(&data_dir, host, &host_key, backup, passphrase)?;
    Ok(())
}

/// Signs a browser's public pairing request at the root host.
///
/// # Errors
///
/// If the pairing request or local root state is invalid.
pub async fn pair(data: Option<PathBuf>, request: &str) -> anyhow::Result<()> {
    let data_dir = crate::start::data_dir_or_default(data)?;
    let membership = local(&data_dir)?;
    let pairing_request = PairRequest::parse_text(request)?;
    let pairing = approve(&membership, pairing_request).await?.to_text()?;
    writeln!(std::io::stdout().lock(), "{pairing}")?;
    Ok(())
}

async fn approve(
    membership: &Membership,
    request: PairRequest,
) -> anyhow::Result<Pairing> {
    let PairRequest { device, name } = request;
    ensure!((1..=128).contains(&name.len()), "invalid device name");
    let certificate = membership.sign_owner(device, name).await?;
    let signed = membership.public_snapshot().await;
    let fleet =
        iroh::PublicKey::from_bytes(&membership.snapshot().await.fleet)?;
    let snapshot = crypto::open(fleet, &signed)?;
    let root = snapshot
        .hosts
        .iter()
        .find(|host| host.node == snapshot.root)
        .context("fleet has no root host")?
        .clone();
    Ok(Pairing {
        certificate,
        root,
        signed,
    })
}

/// Joins the root named by an invitation using this host's persistent identity.
///
/// # Errors
///
/// If the invite is invalid or expired, the root is offline, or membership cannot be saved.
pub async fn join(
    data: Option<PathBuf>,
    text: &str,
    name: String,
) -> anyhow::Result<()> {
    let invitation = Invite::parse_text(text)?;
    let data_dir = crate::start::data_dir_or_default(data)?;
    let _exclusive = exclusive(&data_dir)?;
    let key = crate::load_or_create_key(&data_dir)?;
    if let Some(membership) = Membership::open(&data_dir, key.public())? {
        let snapshot = membership.snapshot().await;
        ensure!(
            snapshot.fleet == invitation.fleet
                && snapshot.root == invitation.root,
            "host already belongs to another fleet"
        );
        ensure!(
            membership.is_active().await,
            "host membership has been revoked"
        );
        return Ok(());
    }
    let endpoint = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Custom(net::platform::n0_relays()))
        .secret_key(key)
        .bind()
        .await?;
    let result = join_bound(&data_dir, &endpoint, &invitation, name).await;
    endpoint.close().await;
    result
}

async fn join_bound(
    data: &Path,
    endpoint: &Endpoint,
    invite: &Invite,
    name: String,
) -> anyhow::Result<()> {
    let root = peer::address(&invite.root, &invite.relay)?;
    tokio::time::timeout(std::time::Duration::from_secs(20), endpoint.online())
        .await?;
    let relay = endpoint
        .addr()
        .relay_urls()
        .next()
        .context("host has no relay")?
        .to_string();
    let entry = host_entry(data, name, relay)?;
    let Reply::Joined {
        certificate,
        config,
    } = peer::join(endpoint, root, invite, entry).await?
    else {
        anyhow::bail!("root host refused the fleet invitation");
    };
    Membership::install_joined(
        data,
        endpoint.id(),
        invite,
        certificate,
        config,
    )?;
    Ok(())
}

/// Prints this host's fleet identity and configuration version.
///
/// # Errors
///
/// If the local membership cannot be read.
pub async fn status(data: Option<PathBuf>) -> anyhow::Result<()> {
    let data_dir = crate::start::data_dir_or_default(data)?;
    let state = local(&data_dir)?;
    let snapshot = state.snapshot().await;
    let local = state.identity().await?.device;
    let mut out = std::io::stdout().lock();
    let fleet = data_encoding::HEXLOWER.encode(&snapshot.fleet);
    let role = if snapshot.root == local {
        "root host"
    } else {
        "member host"
    };
    let version = snapshot.version;
    writeln!(out, "Fleet {fleet}, configuration {version}, {role}")?;
    for host in snapshot.hosts {
        let node = data_encoding::HEXLOWER.encode(&host.node);
        writeln!(out, "  {}  {node}", host.name)?;
    }
    Ok(())
}

/// Revokes a host or device from the root host's stored configuration.
///
/// # Errors
///
/// If the local root is unavailable or the revocation cannot be recorded.
pub async fn revoke(data: Option<PathBuf>, node: &str) -> anyhow::Result<()> {
    let data_dir = crate::start::data_dir_or_default(data)?;
    let _exclusive = exclusive(&data_dir)?;
    let membership = local(&data_dir)?;
    let identity = node.parse::<EndpointId>()?;
    membership.revoke(*identity.as_bytes()).await?;
    writeln!(
        std::io::stdout().lock(),
        "Revoked {node}. Rotate the API keys and tokens the fleet shares. For a revoked host signed in to a subscription, sign it out in that provider's account settings."
    )?;
    Ok(())
}

/// Reloads the root signing key from its passphrase-sealed backup.
///
/// # Errors
///
/// If the backup is invalid, the configuration is foreign, or the directory is occupied.
pub fn restore(
    data: Option<PathBuf>,
    backup: &Path,
    latest: Option<&Path>,
    passphrase: &[u8],
) -> anyhow::Result<()> {
    let data_dir = crate::start::data_dir_or_default(data)?;
    let _exclusive = exclusive(&data_dir)?;
    let recovery = crypto::recover(backup, passphrase)?;
    let signed = latest
        .map(|snapshot| crypto::recover(snapshot, passphrase))
        .transpose()?;
    Membership::restore_root(&data_dir, &recovery, signed)?;
    Ok(())
}

/// Seals this member's signed configuration as a recovery artifact.
///
/// # Errors
///
/// If membership cannot be read or the export cannot be sealed.
pub async fn export(
    data: Option<PathBuf>,
    output: &Path,
    passphrase: &[u8],
) -> anyhow::Result<()> {
    let data_dir = crate::start::data_dir_or_default(data)?;
    local(&data_dir)?.export_config(output, passphrase).await
}

fn local(data: &Path) -> anyhow::Result<Membership> {
    let host = crate::load_or_create_key(data)?;
    Membership::open(data, host.public())?.context("this host has no fleet")
}

fn host_entry(
    data: &Path,
    name: String,
    relay: String,
) -> anyhow::Result<HostEntry> {
    ensure!(
        (1..=128).contains(&name.len()) && relay.len() <= 1024,
        "invalid fleet host entry"
    );
    let key = crate::load_or_create_key(data)?;
    let node = *key.public().as_bytes();
    peer::address(&node, &relay)?;
    Ok(HostEntry {
        node,
        name,
        relay,
        added_at: clock()?,
    })
}

pub(crate) fn exclusive(data: &Path) -> anyhow::Result<std::fs::File> {
    std::fs::create_dir_all(data)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(data.join("fleet.lock"))?;
    file.try_lock()
        .context("another host process is using this fleet data directory")?;
    Ok(file)
}

pub(crate) async fn command(
    fleet: Option<&Membership>,
    endpoint: &Endpoint,
    command: host::fleet::FleetCommand,
) -> host::fleet::FleetResult {
    use host::fleet::{FleetCommand, FleetResult};
    let unavailable = FleetResult::Unavailable;
    let Some(membership) = fleet else {
        return unavailable;
    };
    match command {
        FleetCommand::Status => {
            FleetResult::Status(membership.public_snapshot().await)
        }
        FleetCommand::Invite => new_invite(membership)
            .await
            .map_or(unavailable, FleetResult::Invite),
        FleetCommand::Approve(request) => approve(membership, request)
            .await
            .map_or(unavailable, |pairing| {
                FleetResult::Pairing(Box::new(pairing))
            }),
        FleetCommand::Admit { ticket, name } => {
            admit_existing(membership, endpoint, ticket, name)
                .await
                .map_or(unavailable, |()| FleetResult::Accepted)
        }
        FleetCommand::Revoke(node) => membership
            .revoke(node)
            .await
            .map_or(unavailable, |()| FleetResult::Accepted),
    }
}

async fn new_invite(fleet: &Membership) -> anyhow::Result<Invite> {
    fleet.invite(clock()?).await
}

#[cfg(test)]
mod tests;

async fn admit_existing(
    fleet: &Membership,
    endpoint: &Endpoint,
    ticket: host::fleet::Ticket,
    name: String,
) -> anyhow::Result<()> {
    ensure!(
        ticket.host != *endpoint.id().as_bytes(),
        "cannot enroll the root as a member"
    );
    let address = net::address(&ticket)?;
    let entry = HostEntry {
        node: ticket.host,
        name,
        relay: ticket.relay,
        added_at: clock()?,
    };
    peer::admit_existing(endpoint, fleet, address, ticket.token, entry).await
}
