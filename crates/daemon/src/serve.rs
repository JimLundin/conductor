use std::sync::Arc;
use std::time::Duration;

use agents::ClientName;
use home::{Home, HomeResult};
use host::fleet::FleetResult;
use host::log::Event;
use host::push::{PushCommand, PushResult};
use host::registry::{InstanceResult, Instances};
use host::serve::{self, Relaunch, SessionRoute};
use host::session::Order;
use host::session::{SessionId, SessionRefusal, SessionResult};
use host::sign_in::{SignIn, SignInResult};
use iroh::endpoint::{
    ClosedStream, Connection, ConnectionError, RecvStream, SendStream,
};
use iroh::protocol::{AcceptError, ProtocolHandler};
use link::AuthToken;
use link::CommandId;
use link::log::Completion;
use link::serve::{Admission, Stream};
use link::{
    Command, CommandResult, Feed as Frame, HelloReply, Open, Subscribe, Welcome,
};
use n0_future::time::Instant;
use net::wire::{self, WireError};
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
    #[error("malformed frame body: {0}")]
    Frame(#[from] serde_json::Error),
    #[error("could not finish the stream: {0}")]
    Finish(#[from] ClosedStream),
    #[error("{0}")]
    Stopped(#[from] Stopped),
    #[error("{0}")]
    Connection(#[from] ConnectionError),
}

struct Authenticated {
    name: ClientName,
    certificate: Option<host::fleet::Certificate>,
}

enum Upcoming {
    Frame(Frame),
    Quiet,
    Resubscribe,
    Gone,
}

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
    certificate: &host::fleet::Certificate,
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
    certificate: &host::fleet::Certificate,
) -> bool {
    fleet.is_active().await && fleet.admit_owner(certificate).await.is_ok()
}

async fn admit_fleet(
    fleet: &crate::fleet::Membership,
    peer: iroh::EndpointId,
    first: Option<Open>,
) -> (Admission, Option<host::fleet::Certificate>) {
    match link::serve::device_hello(first) {
        Ok(hello) => {
            let owner = fleet.admit_device(peer, &hello).await.ok();
            let admission = link::serve::device(owner);
            let certificate = matches!(admission, Admission::Admitted(_))
                .then_some(hello.certificate);
            (admission, certificate)
        }
        Err(refused) => (refused, None),
    }
}

async fn write<T: link::frame::Frame + Sync>(
    send: &mut SendStream,
    msg: &T,
) -> Result<(), StreamError> {
    Ok(wire::write(send, &link::frame::encode(msg)?).await?)
}

async fn read(
    recv: &mut RecvStream,
    limit: usize,
) -> Result<Option<Open>, StreamError> {
    Ok(wire::read(recv, limit)
        .await?
        .map(|body| link::frame::decode(&body))
        .transpose()?)
}

fn trace_command(id: CommandId, command: &Command) {
    let operation = link::serve::operation(command);
    tracing::info!(command = %id.0, operation, "command received");
}

const fn command_outcome(result: &CommandResult) -> &'static str {
    match result {
        CommandResult::Session(SessionResult::Accepted)
        | CommandResult::Instance(InstanceResult::Accepted)
        | CommandResult::SignIn(SignInResult::Accepted)
        | CommandResult::Home(HomeResult::Accepted)
        | CommandResult::Push(PushResult::Accepted)
        | CommandResult::Fleet(FleetResult::Accepted) => "accepted",
        CommandResult::Session(SessionResult::Started { .. }) => "started",
        CommandResult::Session(SessionResult::Refused(_))
        | CommandResult::Instance(InstanceResult::Refused(_))
        | CommandResult::SignIn(SignInResult::Refused(_))
        | CommandResult::Home(HomeResult::Refused(_))
        | CommandResult::Fleet(FleetResult::Unavailable) => "rejected",
        CommandResult::Session(SessionResult::Diff(_)) => "diff",
        CommandResult::Home(HomeResult::IssueWritten { .. }) => "issue_written",
        CommandResult::Home(HomeResult::Canvas(_)) => "canvas",
        CommandResult::Push(PushResult::PublicKey(_)) => "push_public_key",
        CommandResult::Push(PushResult::Unavailable) => "push_unavailable",
        CommandResult::Fleet(FleetResult::Status(_)) => "fleet_status",
        CommandResult::Fleet(FleetResult::Invite(_)) => "fleet_invite",
        CommandResult::Fleet(FleetResult::Pairing(_)) => "fleet_pairing",
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
    ) -> (Admission, Option<host::fleet::Certificate>) {
        let membership = self.fleet.borrow().clone();
        match membership {
            Some(fleet) => admit_fleet(&fleet, peer, first).await,
            None => (link::serve::admit(first, &self.token), None),
        }
    }

    async fn authenticate(
        &self,
        conn: &Connection,
    ) -> Result<Option<Authenticated>, StreamError> {
        let started = Instant::now();
        let peer = conn.remote_id();
        let connection = conn.stable_id();
        tracing::debug!(%peer, connection, "waiting for hello");
        let (mut send, mut recv) = conn.accept_bi().await?;
        let first = read(&mut recv, HELLO_BODY_LIMIT).await?;
        let (admission, certificate) = self.admission(peer, first).await;
        match admission {
            Admission::Admitted(client) => {
                tracing::debug!(%peer, connection, elapsed_ms = started.elapsed().as_millis(), "hello received");
                write(
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
                write(&mut send, &HelloReply::Refused { reason }).await?;
                finish_and_wait_for_delivery(&mut send).await?;
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
        match link::serve::stream(read(&mut recv, wire::MAX_FRAME_LEN).await?) {
            Stream::Command(id, command) => {
                trace_command(id, &command);
                let result = self.run(id, command, client).await?;
                tracing::info!(
                    command = %id.0,
                    outcome = command_outcome(&result),
                    "command finished"
                );
                write(&mut send, &result).await?;
                send.finish()?;
            }
            Stream::Subscribe(subscribe) => {
                self.feed(&conn, send, subscribe).await?;
            }
            Stream::Finish => send.finish()?,
        }
        Ok(())
    }

    async fn run(
        &self,
        id: CommandId,
        command: Command,
        client: ClientName,
    ) -> Result<CommandResult, Stopped> {
        let begun = match link::serve::ledger(&command) {
            None => {
                return self
                    .command(command, client, id)
                    .await
                    .map(Completion::into_result);
            }
            Some(kind) => self.log.begin(id, kind).await?,
        };
        match begun {
            BeginCommand::Cached(result) => Ok(result),
            BeginCommand::Execute(lease) => {
                let completion = self.command(command, client, id).await?;
                let result = completion.clone().into_result();
                lease.finish(completion).await?;
                Ok(result)
            }
        }
    }

    async fn command(
        &self,
        command: Command,
        by: ClientName,
        id: CommandId,
    ) -> Result<Completion, Stopped> {
        let membership = self.fleet.borrow().clone();
        let result = match command {
            Command::Session(asked) => CommandResult::Session(
                self.session(serve::session(asked, by)).await?,
            ),
            Command::Instance(asked) => CommandResult::Instance(
                crate::fleet::configure(
                    &self.registry,
                    membership.as_ref(),
                    &self.plugins,
                    asked,
                    id,
                )
                .await?,
            ),
            Command::SignIn(asked) => {
                CommandResult::SignIn(self.registry.sign_in(asked).await?)
            }
            Command::Home(asked) => {
                return self.home.order(asked, by).await;
            }
            Command::Push(asked) => {
                CommandResult::Push(self.push_command(asked).await)
            }
            Command::Fleet(asked) => CommandResult::Fleet(
                crate::fleet::command(
                    membership.as_ref(),
                    &self.endpoint,
                    asked,
                )
                .await,
            ),
        };
        Ok(result.into())
    }

    async fn session(
        &self,
        route: SessionRoute,
    ) -> Result<SessionResult, Stopped> {
        match route {
            SessionRoute::Order(session, order) => {
                self.order(session, order).await
            }
            SessionRoute::Relaunch(session, relaunch) => {
                self.relaunch(session, relaunch).await
            }
            SessionRoute::Start {
                instance,
                at,
                choice,
                by,
            } => self.registry.start(instance, at, choice, by).await,
            SessionRoute::Finish(session, by) => {
                match self.registry.find(session).await? {
                    Some(handle) => handle.finish(by).await,
                    None => Ok(SessionResult::Refused(
                        SessionRefusal::UnknownSession,
                    )),
                }
            }
        }
    }

    async fn push_command(&self, command: PushCommand) -> PushResult {
        match &self.push {
            Some(push) => push.command(command).await,
            None => PushResult::Unavailable,
        }
    }

    async fn order(
        &self,
        session: SessionId,
        order: Order,
    ) -> Result<SessionResult, Stopped> {
        match self.registry.find(session).await? {
            None => Ok(SessionResult::Refused(SessionRefusal::UnknownSession)),
            Some(handle) => Ok(handle.order(order).await.unwrap_or(
                SessionResult::Refused(SessionRefusal::SessionClosed),
            )),
        }
    }

    async fn relaunch(
        &self,
        session: SessionId,
        relaunch: Relaunch,
    ) -> Result<SessionResult, Stopped> {
        match self.registry.relaunch(session).await? {
            Err(refusal) => Ok(SessionResult::Refused(refusal)),
            Ok((handle, adapter, offered, reservation)) => Ok(handle
                .relaunch(relaunch.order(offered), adapter, reservation)
                .await
                .unwrap_or(SessionResult::Refused(
                    SessionRefusal::SessionClosed,
                ))),
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
            None => Ok(send.finish()?),
        }
    }
}

impl Feed {
    async fn run(mut self, conn: &Connection) -> Result<(), StreamError> {
        loop {
            match self.next(conn).await {
                Upcoming::Frame(frame) => {
                    write(&mut self.send, &frame).await?;
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
                _ = conn.closed() => Upcoming::Gone,
            }
        }
    }
}

async fn finish_and_wait_for_delivery(
    send: &mut SendStream,
) -> Result<(), StreamError> {
    send.finish()?;
    let _ = send.stopped().await;
    Ok(())
}

fn upcoming(next: Result<Frame, broadcast::error::RecvError>) -> Upcoming {
    next.map_or(Upcoming::Resubscribe, Upcoming::Frame)
}

#[cfg(test)]
mod diagnostics_tests {
    use std::path::Path;
    use std::sync::Arc;

    use agents::Answer;
    use host::session::{RequestId, SessionCommand, SessionId};
    use link::Command;
    use link::CommandId;
    use vcs::Kept;

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
                super::trace_command(
                    CommandId(uuid::Uuid::from_u128(1)),
                    &Command::Session(SessionCommand::SendMessage {
                        session: SessionId(1),
                        text: String::from("synthetic-message-marker"),
                    }),
                );
                super::trace_command(
                    CommandId(uuid::Uuid::from_u128(2)),
                    &Command::Session(SessionCommand::Answer {
                        session: SessionId(1),
                        request: RequestId(1),
                        answers: vec![Answer(vec![String::from(
                            "synthetic-answer-marker",
                        )])],
                    }),
                );
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
        assert!(logged.contains(
            r#"command=00000000-0000-0000-0000-000000000001 operation="send_message""#
        ));
        assert!(logged.contains(
            r#"command=00000000-0000-0000-0000-000000000002 operation="answer""#
        ));
        assert!(logged.contains("pushed"));
        assert!(!logged.contains("synthetic-"));
        Ok(())
    }
}
