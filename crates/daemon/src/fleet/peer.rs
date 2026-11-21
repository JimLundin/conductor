use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, ensure};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl};
use net::wire;
use proto::fleet::{
    HostEntry, Invite, Reply, Request, SignedConfiguration, Sync,
};
use tokio::sync::{Mutex, Semaphore};

use super::member::clock;
use super::{Membership, Slot};

const MAX_FRAME: usize = 256 * 1024;
const RPC_LIMIT: usize = 16;
const RPC_DEADLINE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct Protocol {
    fleet: Slot,
    enrollment: Option<Arc<Enrollment>>,
    incoming: Arc<Semaphore>,
}

#[derive(Debug)]
struct Enrollment {
    data: PathBuf,
    local: EndpointId,
    token: proto::AuthToken,
    admitting: Mutex<()>,
}

impl Protocol {
    pub(crate) fn new(
        fleet: Slot,
        data: PathBuf,
        local: EndpointId,
        token: proto::AuthToken,
    ) -> Self {
        Self {
            fleet,
            enrollment: Some(Arc::new(Enrollment {
                data,
                local,
                token,
                admitting: Mutex::new(()),
            })),
            incoming: Arc::new(Semaphore::new(RPC_LIMIT)),
        }
    }

    #[cfg(test)]
    pub(super) fn for_member(membership: Membership) -> Self {
        Self {
            fleet: tokio::sync::watch::channel(Some(membership)).0,
            enrollment: None,
            incoming: Arc::new(Semaphore::new(RPC_LIMIT)),
        }
    }

    async fn can_enroll(
        &self,
        remote: EndpointId,
        token: &proto::AuthToken,
        membership: Option<&Membership>,
    ) -> bool {
        if !self.matches_token(token) {
            return false;
        }
        match membership {
            Some(current) => {
                current.snapshot().await.root == *remote.as_bytes()
            }
            None => true,
        }
    }

    fn matches_token(&self, token: &proto::AuthToken) -> bool {
        self.enrollment
            .as_ref()
            .is_some_and(|enrollment| token.check(&enrollment.token).is_ok())
    }

    async fn accept_enrollment(
        &self,
        remote: EndpointId,
        token: &proto::AuthToken,
        certificate: proto::fleet::Certificate,
        config: SignedConfiguration,
    ) -> anyhow::Result<()> {
        let enrollment = self
            .enrollment
            .as_ref()
            .context("host cannot be enrolled")?;
        let _serial = enrollment.admitting.lock().await;
        let existing = self.fleet.borrow().clone();
        ensure!(
            self.can_enroll(remote, token, existing.as_ref()).await,
            "invalid enrollment ticket"
        );
        if let Some(membership) = existing {
            ensure!(
                config.config.snapshot.fleet
                    == membership.snapshot().await.fleet,
                "host already belongs to another fleet"
            );
            return Ok(());
        }
        let membership = Membership::install_admitted(
            &enrollment.data,
            enrollment.local,
            remote,
            certificate,
            config,
        )?;
        self.fleet.send_replace(Some(membership));
        Ok(())
    }

    async fn request(&self, conn: &Connection) -> anyhow::Result<()> {
        let (mut send, mut recv) = conn.accept_bi().await?;
        let request = read::<Request>(&mut recv).await?;
        let membership = self.fleet.borrow().clone();
        match request {
            Request::Sync(peer) => {
                let fleet = membership
                    .as_ref()
                    .context("standalone host has no fleet configuration")?;
                respond_sync(
                    fleet,
                    conn.remote_id(),
                    peer,
                    &mut send,
                    &mut recv,
                )
                .await?;
            }
            other => {
                let reply = self
                    .admission_reply(
                        conn.remote_id(),
                        membership.as_ref(),
                        other,
                    )
                    .await?;
                wire::write(&mut send, &reply).await?;
            }
        }
        send.finish()?;
        let _ = send.stopped().await;
        Ok(())
    }
    async fn admission_reply(
        &self,
        remote: EndpointId,
        fleet: Option<&Membership>,
        request: Request,
    ) -> anyhow::Result<Reply> {
        match request {
            Request::EnrollmentReady(token) => Ok(enrollment_reply(
                self.can_enroll(remote, &token, fleet).await,
                Reply::EnrollmentReady,
            )),
            Request::Enroll {
                token,
                certificate,
                config,
            } => {
                let accepted = self
                    .accept_enrollment(remote, &token, certificate, *config)
                    .await
                    .is_ok();
                Ok(enrollment_reply(accepted, Reply::Enrolled))
            }
            Request::Join { invite, host } => {
                reply_join(fleet, remote, &invite, host).await
            }
            Request::Sync(_) => {
                anyhow::bail!("expected fleet admission request")
            }
        }
    }
}

fn enrollment_reply(allowed: bool, success: Reply) -> Reply {
    if allowed { success } else { Reply::Refused }
}

async fn reply_join(
    fleet: Option<&Membership>,
    remote: EndpointId,
    invite: &proto::fleet::InviteSecret,
    host: HostEntry,
) -> anyhow::Result<Reply> {
    validate_host(&host)?;
    let membership =
        fleet.context("standalone host cannot admit fleet members")?;
    Ok(membership
        .join(remote, invite, host, clock()?)
        .await
        .map_or(Reply::Refused, |(certificate, config)| Reply::Joined {
            certificate,
            config,
        }))
}

async fn respond_sync(
    fleet: &Membership,
    remote: EndpointId,
    peer: proto::fleet::Peer,
    send: &mut SendStream,
    recv: &mut RecvStream,
) -> anyhow::Result<()> {
    if let Some(revocation) =
        fleet.revocation_for(remote, &peer.certificate).await
    {
        return Ok(wire::write(send, &Reply::Revoked(revocation)).await?);
    }
    if !allowed_peer(fleet, remote, &peer.certificate).await {
        return Ok(wire::write(send, &Reply::Refused).await?);
    }
    let local = fleet.peer().await;
    let before = local.version;
    let config = fleet.config_after(peer.version).await;
    wire::write(
        send,
        &Reply::Sync(Sync {
            peer: local,
            config,
        }),
    )
    .await?;
    pull_newer(fleet, recv, before, peer.version).await?;
    fleet.mark_fresh();
    Ok(())
}

async fn allowed_peer(
    fleet: &Membership,
    remote: EndpointId,
    cert: &proto::fleet::Certificate,
) -> bool {
    fleet.admit_host(remote, cert).await.is_ok() && fleet.is_active().await
}

async fn pull_newer(
    fleet: &Membership,
    recv: &mut RecvStream,
    before: u64,
    offered: u64,
) -> anyhow::Result<()> {
    if offered > before {
        let next = read::<SignedConfiguration>(recv).await?;
        adopt_announced(fleet, next, offered).await?;
    }
    Ok(())
}

async fn adopt_announced(
    fleet: &Membership,
    candidate: SignedConfiguration,
    version: u64,
) -> anyhow::Result<()> {
    ensure!(
        candidate.config.snapshot.version >= version,
        "peer did not send its announced configuration"
    );
    fleet.adopt(candidate).await?;
    Ok(())
}

impl ProtocolHandler for Protocol {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let Ok(_slot) = self.incoming.try_acquire() else {
            conn.close(1u32.into(), b"fleet busy");
            return Ok(());
        };
        if !tokio::time::timeout(RPC_DEADLINE, self.request(&conn))
            .await
            .is_ok_and(|result| result.is_ok())
        {
            conn.close(1u32.into(), b"fleet exchange failed");
        }
        Ok(())
    }
}

pub(super) async fn admit_existing(
    endpoint: &Endpoint,
    fleet: &Membership,
    target: EndpointAddr,
    token: proto::AuthToken,
    host: HostEntry,
) -> anyhow::Result<()> {
    validate_host(&host)?;
    ensure!(
        target.id.as_bytes() == &host.node,
        "entry does not name the target"
    );
    ensure!(
        fleet.snapshot().await.root == *endpoint.id().as_bytes(),
        "fleet changes need the root host"
    );
    let probe =
        exchange(endpoint, target.clone(), &Request::EnrollmentReady(token))
            .await?;
    ensure!(
        probe == Reply::EnrollmentReady,
        "standalone host refused its ticket"
    );
    let (certificate, config) = fleet.enroll(host).await?;
    let enrolled = exchange(
        endpoint,
        target,
        &Request::Enroll {
            token,
            certificate,
            config: Box::new(config),
        },
    )
    .await?;
    ensure!(
        enrolled == Reply::Enrolled,
        "standalone host could not save its fleet admission"
    );
    Ok(())
}

async fn exchange(
    endpoint: &Endpoint,
    target: EndpointAddr,
    request: &Request,
) -> anyhow::Result<Reply> {
    tokio::time::timeout(RPC_DEADLINE, async {
        let conn = endpoint.connect(target, proto::fleet::ALPN).await?;
        let (mut send, mut recv) = conn.open_bi().await?;
        wire::write(&mut send, request).await?;
        send.finish()?;
        read(&mut recv).await
    })
    .await?
}

pub(super) async fn join(
    endpoint: &Endpoint,
    root: EndpointAddr,
    invite: &Invite,
    host: HostEntry,
) -> anyhow::Result<Reply> {
    validate_host(&host)?;
    ensure!(
        root.id.as_bytes() == &invite.root,
        "invite names another root"
    );
    let request = Request::Join {
        invite: invite.secret.clone(),
        host,
    };
    exchange(endpoint, root, &request).await
}

pub async fn refresh(endpoint: &Endpoint, membership: &Membership) {
    let local = endpoint.id();
    let hosts = membership.snapshot().await.hosts;
    let peers = hosts
        .iter()
        .filter(|host| &host.node != local.as_bytes())
        .filter_map(|host| address(&host.node, &host.relay).ok())
        .map(|peer| {
            tokio::time::timeout(
                RPC_DEADLINE,
                sync_with(endpoint, membership, peer),
            )
        });
    let _ = n0_future::join_all(peers).await;
}

pub async fn follow_when_enrolled(endpoint: Endpoint, fleet: Slot) {
    let mut observed = fleet.subscribe();
    let joined = tokio::select! {
        result = observed.wait_for(Option::is_some) => result.ok().and_then(|mode| mode.clone()),
        () = endpoint.closed() => None,
    };
    if let Some(membership) = joined {
        follow(endpoint, membership).await;
    }
}

async fn follow(endpoint: Endpoint, membership: Membership) {
    let mut versions = membership.versions();
    while !endpoint.is_closed() {
        refresh(&endpoint, &membership).await;
        tokio::select! {
            () = endpoint.closed() => return,
            () = tokio::time::sleep(POLL) => {},
            result = versions.changed() => if result.is_err() { return; },
        }
    }
}

pub(super) async fn sync_with(
    endpoint: &Endpoint,
    membership: &Membership,
    remote: EndpointAddr,
) -> anyhow::Result<()> {
    let conn = endpoint.connect(remote, proto::fleet::ALPN).await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    let local = membership.peer().await;
    let local_version = local.version;
    wire::write(&mut send, &Request::Sync(local)).await?;
    let (peer, config) = match read::<Reply>(&mut recv).await? {
        Reply::Sync(Sync { peer, config }) => (peer, config),
        Reply::Revoked(revoked) => {
            membership.adopt_revocation(revoked).await?;
            return Ok(());
        }
        _ => anyhow::bail!("the host refused fleet synchronization"),
    };
    membership
        .admit_host(conn.remote_id(), &peer.certificate)
        .await?;
    accept_offer(membership, config, peer.version, local_version).await?;
    send_update(membership, &mut send, peer.version, local_version).await?;
    send.finish()?;
    membership.mark_fresh();
    Ok(())
}

async fn accept_offer(
    fleet: &Membership,
    offered: Option<SignedConfiguration>,
    announced: u64,
    local: u64,
) -> anyhow::Result<()> {
    if let Some(config) = offered {
        return adopt_announced(fleet, config, announced).await;
    }
    ensure!(
        announced <= local,
        "peer withheld its announced configuration"
    );
    Ok(())
}

async fn send_update(
    fleet: &Membership,
    send: &mut SendStream,
    remote: u64,
    local: u64,
) -> anyhow::Result<()> {
    if local > remote
        && let Some(updated) = fleet.config_after(remote).await
    {
        wire::write(send, &updated).await?;
    }
    Ok(())
}

pub(super) fn address(
    id: &[u8; 32],
    relay: &str,
) -> anyhow::Result<EndpointAddr> {
    let endpoint = EndpointId::from_bytes(id)?;
    let relay_url = net::without_root_dot(&relay.parse::<RelayUrl>()?);
    Ok(EndpointAddr::new(endpoint).with_relay_url(relay_url))
}

fn validate_host(host: &HostEntry) -> anyhow::Result<()> {
    ensure!(
        (1..=128).contains(&host.name.len()) && host.relay.len() <= 1024,
        "invalid fleet host entry"
    );
    address(&host.node, &host.relay)?;
    Ok(())
}

async fn read<T: serde::de::DeserializeOwned>(
    recv: &mut RecvStream,
) -> anyhow::Result<T> {
    wire::read_bounded(recv, MAX_FRAME)
        .await?
        .context("fleet stream closed before its reply")
}
