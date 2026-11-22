//! Serves the protocol (`proto::ALPN`) on accepted iroh connections. The
//! protocol's rules are [`host::serve`]; this reads and writes the frames
//! and calls the log and the registry.

use std::sync::Arc;
use std::time::Duration;

use host::log::Completion;
use host::serve::{self, Admission, Order, Relaunch, Route, Stream};
use iroh::endpoint::{
    ClosedStream, Connection, ConnectionError, RecvStream, SendStream,
};
use iroh::protocol::{AcceptError, ProtocolHandler};
use n0_future::time::Instant;
use net::wire::{self, WireError};
use proto::{
    AuthToken, ClientName, Command, CommandBody, CommandResult, Event,
    Feed as Frame, HelloReply, Home, Instances, Open, Rejection, SessionId,
    SignIn, Subscribe, Welcome,
};
use tokio::sync::{Semaphore, broadcast, watch};

use crate::Stopped;
use crate::home::HomeClone;
use crate::log::{BeginCommand, Log};
use crate::registry::{Registry, Watch};

const HANDSHAKE_LIMIT: usize = 32;
const HELLO_BODY_LIMIT: usize = 4096;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct Conductor {
    log: Log,
    registry: Registry,
    home: HomeClone,
    /// Standalone clients present the token from the host ticket.
    token: AuthToken,
    handshakes: Arc<Semaphore>,
    push: Option<crate::push::Push>,
    fleet: crate::fleet::Slot,
    endpoint: iroh::Endpoint,
    plugins: crate::Plugins,
}

pub struct ConnectionOptions {
    pub token: AuthToken,
    pub push: Option<crate::push::Push>,
    pub fleet: crate::fleet::Slot,
    pub endpoint: iroh::Endpoint,
    pub plugins: crate::Plugins,
}

#[derive(Debug, thiserror::Error)]
enum StreamError {
    #[error("{0}")]
    Wire(#[from] WireError),
    #[error("could not finish the stream: {0}")]
    Finish(#[from] ClosedStream),
    #[error("{0}")]
    Stopped(#[from] Stopped),
    #[error("{0}")]
    Connection(#[from] ConnectionError),
}

struct Authenticated {
    name: ClientName,
    certificate: Option<proto::fleet::Certificate>,
}

/// What a subscription does next.
enum Upcoming {
    Frame(Frame),
    /// Nothing to send: the home changed to none.
    Quiet,
    /// The client fell behind or the log or the registry stopped: end the
    /// stream, and the client resubscribes from its cursor.
    Resubscribe,
    /// The connection is gone.
    Gone,
}

/// A gap-free subscription (architecture §8): the replay and the sign-ins
/// running, then the live feed and the sign-ins' steps. The log attaches
/// its feed in the same turn as it takes the replay, and the registry its
/// steps in the turn it lists the sign-ins, so neither overlaps or leaves
/// a gap.
struct Feed {
    send: SendStream,
    first: std::vec::IntoIter<Frame>,
    live: broadcast::Receiver<Event>,
    sign_ins: broadcast::Receiver<SignIn>,
    home: watch::Receiver<Option<Home>>,
    offers: watch::Receiver<Instances>,
}

impl ProtocolHandler for Conductor {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let peer = conn.remote_id();
        let Ok(slot) = self.handshakes.try_acquire() else {
            conn.close(1u32.into(), b"handshake budget exhausted");
            return Ok(());
        };
        let admitted =
            tokio::time::timeout(HANDSHAKE_TIMEOUT, self.authenticate(&conn))
                .await;
        drop(slot);
        let client = match admitted {
            Ok(Ok(client)) => client,
            Ok(Err(err)) => {
                conn.close(1u32.into(), b"handshake failed");
                return Err(AcceptError::from_err(err));
            }
            Err(_) => {
                conn.close(1u32.into(), b"handshake timed out");
                tracing::debug!(%peer, "handshake timed out");
                return Ok(());
            }
        };
        self.serve_client(&conn, client).await;
        Ok(())
    }
}

impl Conductor {
    async fn serve_client(
        &self,
        conn: &Connection,
        authenticated: Option<Authenticated>,
    ) {
        let peer = conn.remote_id();
        if let Some(client) = authenticated {
            tracing::info!(%peer, client = client.name.0, "client connected");
            let membership = self.fleet.borrow().clone();
            if let (Some(fleet), Some(certificate)) =
                (membership.as_ref(), &client.certificate)
            {
                tokio::select! {
                    err = self.streams(conn, &client.name) => tracing::info!(%peer, %err, "client disconnected"),
                    () = wait_revoked(fleet, certificate) => conn.close(1u32.into(), b"fleet device revoked"),
                }
            } else {
                let mut modes = self.fleet.subscribe();
                tokio::select! {
                    err = self.streams(conn, &client.name) => tracing::info!(%peer, %err, "client disconnected"),
                    _ = modes.wait_for(Option::is_some) => conn.close(1u32.into(), b"host joined a fleet"),
                }
            }
        } else {
            tracing::info!(%peer, "refused a client");
            conn.close(1u32.into(), b"refused");
        }
    }
}

async fn wait_revoked(
    fleet: &crate::fleet::Membership,
    certificate: &proto::fleet::Certificate,
) {
    let mut versions = fleet.versions();
    while admitted_owner(fleet, certificate).await {
        if versions.changed().await.is_err() {
            break;
        }
    }
}

async fn admitted_owner(
    fleet: &crate::fleet::Membership,
    certificate: &proto::fleet::Certificate,
) -> bool {
    fleet.is_active().await && fleet.admit_owner(certificate).await.is_ok()
}

async fn admit_fleet(
    fleet: &crate::fleet::Membership,
    peer: iroh::EndpointId,
    first: Option<Open>,
) -> (Admission, Option<proto::fleet::Certificate>) {
    match first {
        Some(Open::FleetHello(hello))
            if fleet.admit_device(peer, &hello).await.is_ok() =>
        {
            let certificate = hello.certificate;
            let name = ClientName(certificate.identity.name.clone());
            (Admission::Admitted(name), Some(certificate))
        }
        _ => (
            Admission::Refused {
                reason: String::from(
                    "a current fleet device certificate is required",
                ),
            },
            None,
        ),
    }
}

fn trace_command(command: &Command) {
    let operation = match &command.body {
        CommandBody::StartSession { .. } => "start_session",
        CommandBody::SendMessage { .. } => "send_message",
        CommandBody::Interrupt { .. } => "interrupt",
        CommandBody::Continue { .. } => "continue",
        CommandBody::Resolve { .. } => "resolve",
        CommandBody::Answer { .. } => "answer",
        CommandBody::SetCredential { .. } => "set_credential",
        CommandBody::GetDiff { .. } => "get_diff",
        CommandBody::SetEffort { .. } => "set_effort",
        CommandBody::RemoveInstance { .. } => "remove_instance",
        CommandBody::StartSignIn { .. } => "start_sign_in",
        CommandBody::StartCredentialSignIn { .. } => "start_credential_sign_in",
        CommandBody::SubmitSignInCode { .. } => "submit_sign_in_code",
        CommandBody::CancelSignIn { .. } => "cancel_sign_in",
        CommandBody::SyncHome => "sync_home",
        CommandBody::CloneProject { .. } => "clone_project",
        CommandBody::RemoveSession { .. } => "remove_session",
        CommandBody::CreateIssue { .. } => "create_issue",
        CommandBody::ChangeIssue { .. } => "change_issue",
        CommandBody::FinishSession { .. } => "finish_session",
        CommandBody::PushPublicKey => "push_public_key",
        CommandBody::RegisterPush(_) => "register_push",
        CommandBody::FleetStatus => "fleet_status",
        CommandBody::FleetInvite => "fleet_invite",
        CommandBody::FleetRevoke(_) => "fleet_revoke",
        CommandBody::FleetApprove(_) => "fleet_approve",
        CommandBody::FleetAdmit { .. } => "fleet_admit",
    };
    tracing::info!(command = command.id.0, operation, "command received");
}

const fn command_outcome(result: &CommandResult) -> &'static str {
    match result {
        CommandResult::Accepted => "accepted",
        CommandResult::Started { .. } => "started",
        CommandResult::Rejected(_) => "rejected",
        CommandResult::Diff(_) => "diff",
        CommandResult::IssueWritten { .. } => "issue_written",
        CommandResult::PushPublicKey(_) => "push_public_key",
        CommandResult::PushUnavailable => "push_unavailable",
        CommandResult::FleetStatus(_) => "fleet_status",
        CommandResult::FleetInvite(_) => "fleet_invite",
        CommandResult::FleetPairing(_) => "fleet_pairing",
    }
}

impl Conductor {
    #[must_use]
    pub(crate) fn new(
        log: Log,
        registry: Registry,
        home: HomeClone,
        options: ConnectionOptions,
    ) -> Self {
        let ConnectionOptions {
            token,
            push,
            fleet,
            endpoint,
            plugins,
        } = options;
        Self {
            log,
            registry,
            home,
            token,
            handshakes: Arc::new(Semaphore::new(HANDSHAKE_LIMIT)),
            push,
            fleet,
            endpoint,
            plugins,
        }
    }

    async fn admission(
        &self,
        peer: iroh::EndpointId,
        first: Option<Open>,
    ) -> (Admission, Option<proto::fleet::Certificate>) {
        let membership = self.fleet.borrow().clone();
        match membership {
            Some(fleet) => admit_fleet(&fleet, peer, first).await,
            None => (serve::admit(first, &self.token), None),
        }
    }

    /// Answers the connection's first stream. Returns the client's name, or
    /// `None` if it was refused.
    async fn authenticate(
        &self,
        conn: &Connection,
    ) -> Result<Option<Authenticated>, StreamError> {
        let started = Instant::now();
        let peer = conn.remote_id();
        let connection = conn.stable_id();
        tracing::debug!(%peer, connection, "waiting for hello");
        let (mut send, mut recv) = conn.accept_bi().await?;
        let first =
            wire::read_bounded::<Open>(&mut recv, HELLO_BODY_LIMIT).await?;
        let (admission, certificate) = self.admission(peer, first).await;
        match admission {
            Admission::Admitted(client) => {
                tracing::debug!(%peer, connection, elapsed_ms = started.elapsed().as_millis(), "hello received");
                wire::write(
                    &mut send,
                    &HelloReply::Welcome(self.welcome(conn).await?),
                )
                .await?;
                send.finish()?;
                tracing::debug!(%peer, connection, elapsed_ms = started.elapsed().as_millis(), "welcome sent");
                Ok(Some(Authenticated {
                    name: client,
                    certificate,
                }))
            }
            Admission::Refused { reason } => {
                wire::write(&mut send, &HelloReply::Refused { reason }).await?;
                send.finish()?;
                // Let the refusal reach the client before the close does.
                let _ = send.stopped().await;
                Ok(None)
            }
        }
    }

    async fn welcome(&self, conn: &Connection) -> Result<Welcome, Stopped> {
        let started = Instant::now();
        let peer = conn.remote_id();
        let connection = conn.stable_id();
        let (instances, catalog) = self.registry.instances().await?;
        tracing::debug!(%peer, connection, elapsed_ms = started.elapsed().as_millis(), "welcome instances ready");
        let welcome = self
            .log
            .welcome(instances, catalog, self.home.shown())
            .await?;
        tracing::debug!(%peer, connection, elapsed_ms = started.elapsed().as_millis(), "welcome log ready");
        Ok(welcome)
    }

    async fn streams(
        &self,
        conn: &Connection,
        client: &ClientName,
    ) -> ConnectionError {
        loop {
            match conn.accept_bi().await {
                Ok((send, recv)) => {
                    let stream = self.clone().stream(
                        conn.clone(),
                        send,
                        recv,
                        client.clone(),
                    );
                    tokio::spawn(async move {
                        if let Err(err) = stream.await {
                            tracing::debug!(%err, "stream ended with an error");
                        }
                    });
                }
                Err(err) => break err,
            }
        }
    }

    async fn stream(
        self,
        conn: Connection,
        mut send: SendStream,
        mut recv: RecvStream,
        client: ClientName,
    ) -> Result<(), StreamError> {
        match serve::stream(wire::read::<Open>(&mut recv).await?) {
            Stream::Command(command) => {
                trace_command(&command);
                let id = command.id;
                let result = self.run(command, client).await?;
                tracing::info!(
                    command = id.0,
                    outcome = command_outcome(&result),
                    "command finished"
                );
                wire::write(&mut send, &result).await?;
                send.finish()?;
            }
            Stream::Subscribe(subscribe) => {
                self.feed(&conn, send, subscribe).await?;
            }
            Stream::Finish => send.finish()?,
        }
        Ok(())
    }

    /// Runs the command or returns its receipt when it has completed.
    async fn run(
        &self,
        command: Command,
        client: ClientName,
    ) -> Result<CommandResult, Stopped> {
        let route = serve::route(command.body, client);
        let begun = match route.ledger() {
            None => {
                return self
                    .command(route, command.id)
                    .await
                    .map(Completion::into_result);
            }
            Some(kind) => self.log.begin(command.id, kind).await?,
        };
        match begun {
            BeginCommand::Cached(result) => Ok(result),
            BeginCommand::Execute(lease) => {
                let completion = self.command(route, command.id).await?;
                let result = completion.clone().into_result();
                lease.finish(completion).await?;
                Ok(result)
            }
        }
    }

    async fn command(
        &self,
        route: Route,
        id: proto::CommandId,
    ) -> Result<Completion, Stopped> {
        let membership = self.fleet.borrow().clone();
        let result = match route {
            Route::Order(session, order) => self.order(session, order).await,
            Route::Relaunch(session, relaunch) => {
                self.relaunch(session, relaunch).await
            }
            Route::Start {
                instance,
                at,
                choice,
                by,
            } => self.registry.start(instance, at, choice, by).await,
            Route::Configure(configure) => {
                crate::fleet::configure(
                    &self.registry,
                    membership.as_ref(),
                    &self.plugins,
                    configure,
                    id,
                )
                .await
            }
            Route::Push(order) => Ok(self.push_order(order).await),
            Route::Fleet(action) => Ok(crate::fleet::command(
                membership.as_ref(),
                &self.endpoint,
                action,
            )
            .await),
            Route::SignIn(order) => self.registry.sign_in(order).await,
            Route::Home(order) => return self.home.order(order, id).await,
            Route::Finish(session, by) => {
                match self.registry.find(session).await? {
                    Some(handle) => handle.finish(by).await,
                    None => {
                        Ok(CommandResult::Rejected(Rejection::UnknownSession))
                    }
                }
            }
        }?;
        Ok(result.into())
    }

    async fn push_order(&self, order: host::serve::PushOrder) -> CommandResult {
        match &self.push {
            Some(push) => push.command(order).await,
            None => CommandResult::PushUnavailable,
        }
    }

    async fn order(
        &self,
        session: SessionId,
        order: Order,
    ) -> Result<CommandResult, Stopped> {
        match self.registry.find(session).await? {
            None => Ok(CommandResult::Rejected(Rejection::UnknownSession)),
            Some(handle) => Ok(handle
                .order(order)
                .await
                .unwrap_or(CommandResult::Rejected(Rejection::SessionClosed))),
        }
    }

    async fn relaunch(
        &self,
        session: SessionId,
        relaunch: Relaunch,
    ) -> Result<CommandResult, Stopped> {
        match self.registry.relaunch(session).await? {
            Err(rejection) => Ok(CommandResult::Rejected(rejection)),
            Ok((handle, adapter, offered, reservation)) => Ok(handle
                .relaunch(relaunch.order(offered), adapter, reservation)
                .await
                .unwrap_or(CommandResult::Rejected(Rejection::SessionClosed))),
        }
    }

    async fn feed(
        &self,
        conn: &Connection,
        mut send: SendStream,
        subscribe: Subscribe,
    ) -> Result<(), StreamError> {
        match self.log.resume(subscribe).await? {
            Some(subscription) => {
                tracing::info!(
                    after_seq = subscribe.after_seq.0,
                    replayed = subscription.replay.len(),
                    "subscribed"
                );
                let Watch {
                    running,
                    sign_ins,
                    mut offers,
                } = self.registry.watch().await?;
                let snapshot = offers.borrow_and_update().clone();
                let mut home = self.home.watch();
                let current = home.borrow_and_update().clone();
                let first = subscription
                    .replay
                    .into_iter()
                    .map(Frame::Event)
                    .chain([Frame::Instances(snapshot)])
                    .chain(running.into_iter().map(Frame::SignIn))
                    .chain(current.into_iter().map(Frame::Home))
                    .collect::<Vec<Frame>>();
                let feed = Feed {
                    send,
                    first: first.into_iter(),
                    live: subscription.live,
                    sign_ins,
                    home,
                    offers,
                };
                let result = feed.run(conn).await;
                tracing::info!(?result, "subscription ended");
                result
            }
            // A cursor from another epoch is refused by closing the stream.
            None => Ok(send.finish()?),
        }
    }
}

impl Feed {
    async fn run(mut self, conn: &Connection) -> Result<(), StreamError> {
        loop {
            match self.next(conn).await {
                Upcoming::Frame(frame) => {
                    wire::write(&mut self.send, &frame).await?;
                }
                Upcoming::Quiet => {}
                Upcoming::Resubscribe => break Ok(self.send.finish()?),
                Upcoming::Gone => break Ok(()),
            }
        }
    }

    async fn next(&mut self, conn: &Connection) -> Upcoming {
        if let Some(frame) = self.first.next() {
            Upcoming::Frame(frame)
        } else {
            tokio::select! {
                next = self.live.recv() => upcoming(next.map(Frame::Event)),
                next = self.sign_ins.recv() => upcoming(next.map(Frame::SignIn)),
                changed = self.offers.changed() => changed.map_or(Upcoming::Resubscribe, |()| Upcoming::Frame(Frame::Instances(self.offers.borrow_and_update().clone()))),
                changed = self.home.changed() => changed.map_or(
                    Upcoming::Resubscribe,
                    |()| self.home.borrow_and_update().clone().map_or(Upcoming::Quiet, |home| Upcoming::Frame(Frame::Home(home))),
                ),
                // Without this, an idle feed would outlive its connection.
                _ = conn.closed() => Upcoming::Gone,
            }
        }
    }
}

/// A live frame, or a resubscribe if the subscriber fell behind or the
/// sender stopped.
fn upcoming(next: Result<Frame, broadcast::error::RecvError>) -> Upcoming {
    next.map_or(Upcoming::Resubscribe, Upcoming::Frame)
}

#[cfg(test)]
mod diagnostics_tests {
    use std::path::Path;
    use std::sync::Arc;

    use proto::{
        Answer, Command, CommandBody, CommandId, Kept, RequestId, SessionId,
    };

    #[test]
    fn command_and_workspace_diagnostics_omit_payloads() -> anyhow::Result<()> {
        let directory =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        std::fs::create_dir_all(&directory)?;
        let path = directory
            .join(format!("daemon-diagnostics-{}.log", std::process::id()));
        let output = std::fs::File::create(&path)?;
        tracing::subscriber::with_default(
            tracing_subscriber::fmt()
                .with_ansi(false)
                .with_writer(Arc::new(output))
                .finish(),
            || {
                super::trace_command(&Command {
                    id: CommandId(1),
                    body: CommandBody::SendMessage {
                        session: SessionId(1),
                        text: String::from("synthetic-message-marker"),
                    },
                });
                super::trace_command(&Command {
                    id: CommandId(2),
                    body: CommandBody::Answer {
                        session: SessionId(1),
                        request: RequestId(1),
                        answers: vec![Answer(vec![String::from(
                            "synthetic-answer-marker",
                        )])],
                    },
                });
                crate::workspace::trace_removed(
                    "conductor/1",
                    &Kept::Pushed {
                        remote: String::from(
                            "https://synthetic-credential-marker@example.invalid/repo",
                        ),
                    },
                );
            },
        );
        let logged = std::fs::read_to_string(&path)?;
        std::fs::remove_file(path)?;
        assert!(logged.contains(r#"command=1 operation="send_message""#));
        assert!(logged.contains(r#"command=2 operation="answer""#));
        assert!(logged.contains("pushed"));
        assert!(!logged.contains("synthetic-"));
        Ok(())
    }
}
