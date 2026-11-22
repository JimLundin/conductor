//! A certified peer hands off one stopped session.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use host::movement::{Incoming, Ownership, Reception};
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr, EndpointId};
use net::wire;
use proto::fleet::{Certificate, HostEntry, NodeId};
use proto::movement::{MoveId, Phase};
use proto::{
    CommandId, CommandResult, EventKind, InstanceId, Rejection, SessionId,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Semaphore};

use crate::fleet::{Membership, Slot};
use crate::home::HomeClone;
use crate::log::{BeginCommand, Log};
use crate::registry::Registry;

mod package;

use self::package::{Header, Prepared, Received, Scratch, Target};

pub const ALPN: &[u8] = b"conductor-move/2";
const DEADLINE: Duration = Duration::from_secs(600);
const CONTROL_LIMIT: usize = 64 * 1024;
const PEER_HELLO: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct Mover {
    endpoint: Endpoint,
    member: Slot,
    log: Log,
    registry: Registry,
    home: HomeClone,
    data: PathBuf,
    sending: Arc<Mutex<()>>,
    receiving: Arc<Semaphore>,
    workspace_changes: Arc<Mutex<()>>,
}

#[derive(Clone)]
pub struct ProviderFiles {
    pub home: PathBuf,
    pub version: String,
    pub io: runtime::TranscriptFiles,
}

#[derive(Serialize, Deserialize)]
enum Request {
    Status { session: SessionId, id: MoveId },
    Transfer(Header),
}

#[derive(Serialize, Deserialize)]
enum Reply {
    Ready { known: Option<String> },
    Committed,
    Missing,
    Refused(String),
}

impl Mover {
    pub(crate) fn new(
        endpoint: Endpoint,
        member: Slot,
        log: Log,
        registry: Registry,
        home: HomeClone,
        data: PathBuf,
    ) -> Self {
        Self {
            endpoint,
            member,
            log,
            registry,
            home,
            data,
            sending: Arc::new(Mutex::new(())),
            receiving: Arc::new(Semaphore::new(1)),
            workspace_changes: Arc::new(Mutex::new(())),
        }
    }

    fn membership(&self) -> Result<Membership> {
        self.member
            .borrow()
            .clone()
            .context("session moves require fleet membership")
    }

    async fn active_membership(&self) -> Result<Membership> {
        let member = self.membership()?;
        ensure!(
            member.accepts_work().await,
            "the local fleet membership is not current"
        );
        Ok(member)
    }

    pub(crate) async fn recover_when_enrolled(self) {
        tokio::select! {
            () = self.endpoint.closed() => {},
            result = self.recover_when_ready() => {
                if result.is_err() {
                    tracing::warn!("could not recover pending session moves");
                }
            },
        }
    }

    async fn recover_when_ready(&self) -> Result<()> {
        let mut observed = self.member.subscribe();
        let member = observed
            .wait_for(Option::is_some)
            .await?
            .clone()
            .context("fleet enrollment is unavailable")?;
        let mut versions = member.versions();
        while !member.accepts_work().await {
            versions.changed().await?;
        }
        self.recover().await
    }

    pub(crate) async fn targets(&self) -> Vec<NodeId> {
        self.hosts()
            .await
            .into_iter()
            .map(|host| host.node)
            .collect()
    }

    async fn hosts(&self) -> Vec<HostEntry> {
        let Ok(member) = self.active_membership().await else {
            return Vec::new();
        };
        let known = member.snapshot().await;
        let local = self.endpoint.id();
        known
            .hosts
            .into_iter()
            .filter(|host| {
                host.node != *local.as_bytes()
                    && !known.revoked.contains(&host.node)
            })
            .collect()
    }

    async fn address(&self, to: NodeId) -> Result<EndpointAddr> {
        let host = self
            .hosts()
            .await
            .into_iter()
            .find(|host| host.node == to)
            .context("the target is not an admitted fleet host")?;
        let id = EndpointId::from_bytes(&host.node)?;
        let addr = EndpointAddr::new(id);
        Ok(match host.relay.parse() {
            Ok(relay) => addr.with_relay_url(relay),
            Err(_) => addr,
        })
    }

    pub(crate) async fn order(
        &self,
        session: SessionId,
        to: NodeId,
        id: MoveId,
    ) -> CommandResult {
        let _sending = self.sending.lock().await;
        match self.drive(session, to, id).await {
            Ok(()) => CommandResult::Accepted,
            Err(reason) => {
                self.failed(session, to, id, &reason).await;
                CommandResult::Rejected(Rejection::MoveRefused {
                    reason: reason.to_string(),
                })
            }
        }
    }

    async fn failed(
        &self,
        session: SessionId,
        to: NodeId,
        id: MoveId,
        reason: &anyhow::Error,
    ) {
        if let Err(error) =
            self.note_failure(session, to, id, reason.to_string()).await
        {
            tracing::warn!(?session, %error, "could not record move failure");
        }
    }

    async fn note_failure(
        &self,
        session: SessionId,
        to: NodeId,
        id: MoveId,
        reason: String,
    ) -> Result<()> {
        let events = self.log.finish_history(session).await?;
        let Ownership::Moving {
            to: known,
            id: active,
            phase,
        } = Ownership::replay(&events)
        else {
            return Ok(());
        };
        ensure!((known, active) == (to, id), "another handoff is pending");
        if matches!(phase, Phase::Settling | Phase::CheckingTarget) {
            let handle = self
                .registry
                .find(session)
                .await?
                .context("no such local session")?;
            let lease = self.log_move().await?;
            let result = handle.abort_move(id, reason).await?;
            lease.finish(result).await?;
        } else {
            self.record(
                session,
                EventKind::SessionMoveFailed {
                    move_id: id,
                    reason,
                    resumable: false,
                },
            )
            .await?;
        }
        Ok(())
    }

    pub(crate) async fn recover(&self) -> Result<()> {
        let events = self.log.history().await?;
        for session in host::restore::created(&events) {
            let history = host::restore::events_of(session.session, &events);
            match Ownership::replay(&history) {
                Ownership::Moving { to, id, .. }
                | Ownership::Released { to, id } => {
                    let _ = self.order(session.session, to, id).await;
                }
                Ownership::Local => {}
            }
        }
        Ok(())
    }

    async fn drive(
        &self,
        session: SessionId,
        to: NodeId,
        id: MoveId,
    ) -> Result<()> {
        let before = self.log.finish_history(session).await?;
        if matches!(Ownership::replay(&before), Ownership::Released { to: target, id: known }
            if (to, id) == (target, known))
        {
            return self.clean_released(session, id).await;
        }
        let address = self.address(to).await?;
        self.settle(session, to, id).await?;
        tokio::time::timeout(DEADLINE, self.transfer(address, session, id))
            .await
            .context("the handoff timed out; retry the same move")??;
        self.release(session, to, id).await?;
        if let Err(reason) = self.clean_released(session, id).await {
            tracing::warn!(?session, %reason, "move acknowledged; local cleanup will retry on restart");
        }
        Ok(())
    }

    async fn transfer(
        &self,
        address: EndpointAddr,
        session: SessionId,
        id: MoveId,
    ) -> Result<()> {
        if self.status(address.clone(), session, id).await? {
            return Ok(());
        }
        let events = self.log.finish_history(session).await?;
        let staged = Scratch::new(&self.data)?;
        let prepared = Prepared::of(self, session, id, events, staged).await?;
        self.send(address, prepared).await
    }

    async fn connect(
        &self,
        address: EndpointAddr,
    ) -> Result<(
        Connection,
        iroh::endpoint::SendStream,
        iroh::endpoint::RecvStream,
    )> {
        let member = self.active_membership().await?;
        let conn = self.endpoint.connect(address, ALPN).await?;
        let (mut send, mut recv) = conn.open_bi().await?;
        wire::write(&mut send, &member.peer().await.certificate).await?;
        let peer: Certificate = required(&mut recv).await?;
        member.admit_host(conn.remote_id(), &peer).await?;
        Ok((conn, send, recv))
    }

    async fn status(
        &self,
        address: EndpointAddr,
        session: SessionId,
        id: MoveId,
    ) -> Result<bool> {
        let (_conn, mut send, mut recv) = self.connect(address).await?;
        wire::write(&mut send, &Request::Status { session, id }).await?;
        send.finish()?;
        match required(&mut recv).await? {
            Reply::Committed => Ok(true),
            Reply::Missing => Ok(false),
            Reply::Refused(reason) => anyhow::bail!("{reason}"),
            Reply::Ready { .. } => anyhow::bail!(
                "the peer returned readiness in place of move status"
            ),
        }
    }

    async fn settle(
        &self,
        session: SessionId,
        to: NodeId,
        id: MoveId,
    ) -> Result<()> {
        let handle = self
            .registry
            .find(session)
            .await?
            .context("no such local session")?;
        let lease = self.log_move().await?;
        let result = handle.prepare_move(to, id).await?;
        lease.finish(result.clone()).await?;
        match result {
            CommandResult::Accepted => Ok(()),
            CommandResult::Rejected(reason) => {
                anyhow::bail!("{}", reason.label())
            }
            _ => anyhow::bail!("the session did not settle"),
        }
    }

    async fn release(
        &self,
        session: SessionId,
        to: NodeId,
        id: MoveId,
    ) -> Result<()> {
        self.record(session, EventKind::SessionMoved { to, move_id: id })
            .await
    }

    async fn log_move(&self) -> Result<crate::log::CommandLease> {
        match self.log.begin(fresh_key()).await? {
            BeginCommand::Execute(lease) => Ok(lease),
            BeginCommand::Cached(_) => {
                anyhow::bail!("a local move transaction already used that key")
            }
        }
    }

    async fn record(&self, session: SessionId, kind: EventKind) -> Result<()> {
        let lease = self.log_move().await?;
        self.log.append(session, kind).await?;
        lease.finish(CommandResult::Accepted).await?;
        Ok(())
    }

    async fn clean_released(
        &self,
        session: SessionId,
        id: MoveId,
    ) -> Result<()> {
        let _workspace = self.workspace_changes.lock().await;
        let current = self.log.finish_history(session).await?;
        if matches!(Ownership::replay(&current), Ownership::Released { id: known, .. } if known == id)
        {
            self.clean_once(session, id, &current).await?;
        }
        Ok(())
    }

    async fn clean_once(
        &self,
        session: SessionId,
        id: MoveId,
        events: &[proto::Event],
    ) -> Result<()> {
        let dir = self.data.join("released-moves");
        std::fs::create_dir_all(&dir)?;
        let marker = dir.join(format!(
            "{}-{}",
            ulid::Ulid::from(session.0),
            ulid::Ulid::from(id.0)
        ));
        if marker.try_exists()? {
            return Ok(crate::sync_dir(&dir)?);
        }
        self.clean(session, events).await?;
        crate::write_private(&marker, b"")?;
        crate::sync_dir(&dir)?;
        Ok(())
    }

    async fn clean(
        &self,
        session: SessionId,
        events: &[proto::Event],
    ) -> Result<()> {
        let mut retained = host::movement::SessionFiles::of(events)
            .map_err(anyhow::Error::msg)?;
        let current = self.log.saved_conversation(session).await?;
        retained.conversations =
            host::movement::conversations(events, current.as_deref());
        let (_, provider) =
            self.provider(retained.instance.clone(), false).await?;
        package::clean(session, retained, provider).await
    }

    async fn provider(
        &self,
        instance: InstanceId,
        incoming: bool,
    ) -> Result<(InstanceId, ProviderFiles)> {
        let (id, settings, providers) = self
            .registry
            .move_instance(instance, incoming)
            .await
            .map_err(anyhow::Error::msg)?;
        let files = providers
            .move_files(&id, &settings)
            .await
            .map_err(anyhow::Error::msg)?;
        let fleet = self.membership()?.pins().await;
        let pin = fleet.version(id.provider).ok_or_else(|| {
            anyhow::anyhow!(
                "the provider has no pin in the signed fleet configuration",
            )
        })?;
        ensure!(
            files.version == pin,
            "the local provider pin differs from the signed fleet configuration"
        );
        Ok((id, files))
    }

    async fn phase(
        &self,
        session: SessionId,
        to: NodeId,
        id: MoveId,
        phase: Phase,
    ) -> Result<()> {
        self.record(
            session,
            EventKind::SessionMoving {
                to,
                move_id: id,
                phase,
            },
        )
        .await
    }

    async fn send(
        &self,
        address: EndpointAddr,
        prepared: Prepared,
    ) -> Result<()> {
        let to = *address.id.as_bytes();
        let (_conn, mut send, mut recv) = self.connect(address).await?;
        let Header { session, id, .. } = prepared.header;
        wire::write(&mut send, &Request::Transfer(prepared.header.clone()))
            .await?;
        let known = match required(&mut recv).await? {
            Reply::Committed => return Ok(()),
            Reply::Refused(reason) => anyhow::bail!("{reason}"),
            Reply::Ready { known } => known,
            Reply::Missing => {
                anyhow::bail!("the target did not prepare the move")
            }
        };
        let transfer = prepared.make_transfer(known.as_deref()).await?;
        self.phase(session, to, id, Phase::SendingWorkspace).await?;
        transfer.send_bundle(&mut send).await?;
        self.phase(session, to, id, Phase::SendingHistory).await?;
        transfer.send_history(&mut send).await?;
        send.finish()?;
        self.phase(session, to, id, Phase::Committing).await?;
        match required(&mut recv).await? {
            Reply::Committed => Ok(()),
            Reply::Refused(reason) => anyhow::bail!("{reason}"),
            Reply::Ready { .. } | Reply::Missing => {
                anyhow::bail!("the target did not commit the move")
            }
        }
    }

    async fn receive(&self, conn: &Connection) -> Result<()> {
        let member = self.active_membership().await?;
        let (mut send, mut recv, source) =
            tokio::time::timeout(PEER_HELLO, peer_hello(conn)).await??;
        let remote = conn.remote_id();
        member.admit_host(remote, &source).await?;
        let permit = self
            .receiving
            .try_acquire()
            .context("the target is receiving another session")?;
        wire::write(&mut send, &member.peer().await.certificate).await?;
        let from = *remote.as_bytes();
        let reply = match required(&mut recv).await? {
            Request::Status { session, id } => self
                .receipt(Incoming { from, session, id })
                .await
                .map(|stored| {
                    if stored {
                        Reply::Committed
                    } else {
                        Reply::Missing
                    }
                }),
            Request::Transfer(header) => {
                let incoming = Incoming {
                    from,
                    id: header.id,
                    session: header.session,
                };
                self.receive_checked(
                    &mut send, &mut recv, header, incoming, remote, &source,
                )
                .await
            }
        }
        .unwrap_or_else(|err| Reply::Refused(err.to_string()));
        drop(permit);
        wire::write(&mut send, &reply).await?;
        send.finish()?;
        let _ = send.stopped().await;
        Ok(())
    }

    async fn receipt(&self, incoming: Incoming) -> Result<bool> {
        Ok(incoming.receipt(&self.log.history().await?))
    }

    async fn receive_checked(
        &self,
        send: &mut iroh::endpoint::SendStream,
        recv: &mut iroh::endpoint::RecvStream,
        header: Header,
        incoming: Incoming,
        remote: EndpointId,
        certificate: &Certificate,
    ) -> Result<Reply> {
        if self.receipt(incoming).await? {
            return Ok(Reply::Committed);
        }
        let history = self.log.finish_history(incoming.session).await?;
        ensure!(
            Reception::after(&history) != Reception::Owned,
            "the target already owns that session"
        );
        let target = Target::ready(self, header, &history).await?;
        let scratch = Scratch::new(&self.data)?;
        wire::write(
            send,
            &Reply::Ready {
                known: target.known().await,
            },
        )
        .await?;
        let received = Received::receive(scratch, recv).await?;
        self.active_membership()
            .await?
            .admit_host(remote, certificate)
            .await?;
        self.complete(target, received, incoming).await?;
        Ok(Reply::Committed)
    }

    async fn complete(
        &self,
        target: Target,
        received: Received,
        incoming: Incoming,
    ) -> Result<()> {
        let _workspace = self.workspace_changes.lock().await;
        let prior = self.log.finish_history(incoming.session).await?;
        target.verify(incoming, &received, &prior)?;
        let previous = self.prior_release(incoming.session, &prior).await?;
        let arrival = target
            .install(self, received, incoming, previous.as_deref())
            .await?;
        self.log.arrive(arrival).await.map_err(anyhow::Error::msg)?;
        self.registry.arrived(incoming.session, incoming.id).await?;
        Ok(())
    }

    async fn prior_release(
        &self,
        session: SessionId,
        history: &[proto::Event],
    ) -> Result<Option<String>> {
        match Reception::after(history) {
            Reception::Vacant => Ok(None),
            Reception::Returning => {
                let Ownership::Released { id, .. } = Ownership::replay(history)
                else {
                    anyhow::bail!(
                        "no completed local move preceded this return"
                    );
                };
                self.clean_once(session, id, history).await?;
                host::movement::SessionFiles::of(history)
                    .map(|files| Some(files.checkpoint))
                    .map_err(anyhow::Error::msg)
            }
            Reception::Owned => {
                anyhow::bail!("the target already owns that session")
            }
        }
    }
}

impl ProtocolHandler for Mover {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let result = tokio::time::timeout(DEADLINE, self.receive(&conn)).await;
        result.map_err(AcceptError::from_err)?.map_err(|error| {
            AcceptError::from_err(std::io::Error::other(
                error.into_boxed_dyn_error(),
            ))
        })
    }
}

async fn peer_hello(
    conn: &Connection,
) -> Result<(
    iroh::endpoint::SendStream,
    iroh::endpoint::RecvStream,
    Certificate,
)> {
    let (send, mut recv) = conn.accept_bi().await?;
    let source = required(&mut recv).await?;
    Ok((send, recv, source))
}

async fn required<T: serde::de::DeserializeOwned>(
    recv: &mut iroh::endpoint::RecvStream,
) -> Result<T> {
    wire::read_bounded(recv, CONTROL_LIMIT)
        .await?
        .context("the peer closed the move stream")
}

fn fresh_key() -> CommandId {
    CommandId(ulid::Ulid::new().0)
}

fn data_path(data: &Path, category: &str, session: SessionId) -> PathBuf {
    data.join(category)
        .join(ulid::Ulid::from(session.0).to_string())
}

#[cfg(all(test, unix))]
mod tests;
