//! Serves the protocol (`proto::ALPN`) on accepted iroh connections. The
//! protocol's rules are [`host::serve`]; this reads and writes the frames
//! and calls the log and the registry.

use host::serve::{self, Admission, Ledger, Order, Relaunch, Route, Stream};
use iroh::endpoint::{
    ClosedStream, Connection, ConnectionError, RecvStream, SendStream,
};
use iroh::protocol::{AcceptError, ProtocolHandler};
use n0_future::time::Instant;
use net::wire::{self, WireError};
use proto::{
    AuthToken, ClientName, Command, CommandResult, Event, Feed as Frame,
    HelloReply, Home, Open, Rejection, SessionId, SignIn, Subscribe, Welcome,
};
use tokio::sync::{broadcast, watch};

use crate::Stopped;
use crate::home::HomeClone;
use crate::log::{BeginCommand, Log};
use crate::registry::Registry;

#[derive(Debug, Clone)]
pub struct Conductor {
    log: Log,
    registry: Registry,
    home: HomeClone,
    /// A placeholder for architecture §4's device certificates: every client
    /// must present the token from the ticket.
    token: AuthToken,
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
}

impl ProtocolHandler for Conductor {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let peer = conn.remote_id();
        if let Some(client) = self
            .authenticate(&conn)
            .await
            .map_err(AcceptError::from_err)?
        {
            tracing::info!(%peer, client = client.0, "client connected");
            let err = self.streams(&conn, &client).await;
            tracing::info!(%peer, client = client.0, %err, "client disconnected");
        } else {
            tracing::info!(%peer, "refused a client");
            conn.close(1u32.into(), b"refused");
        }
        Ok(())
    }
}

impl Conductor {
    /// The host's protocol handler, which also notes the progress of its
    /// sessions on issues from now on.
    #[must_use]
    pub fn new(
        log: Log,
        registry: Registry,
        home: HomeClone,
        token: AuthToken,
    ) -> Self {
        crate::progress::spawn(log.clone(), home.clone());
        Self {
            log,
            registry,
            home,
            token,
        }
    }

    /// Answers the connection's first stream. Returns the client's name, or
    /// `None` if it was refused.
    async fn authenticate(
        &self,
        conn: &Connection,
    ) -> Result<Option<ClientName>, StreamError> {
        let started = Instant::now();
        let peer = conn.remote_id();
        let connection = conn.stable_id();
        tracing::debug!(%peer, connection, "waiting for hello");
        let (mut send, mut recv) = conn.accept_bi().await?;
        let first = wire::read::<Open>(&mut recv).await?;
        match serve::admit(first, &self.token) {
            Admission::Admitted(client) => {
                tracing::debug!(%peer, connection, elapsed_ms = started.elapsed().as_millis(), "hello received");
                wire::write(
                    &mut send,
                    &HelloReply::Welcome(self.welcome(conn).await?),
                )
                .await?;
                send.finish()?;
                tracing::debug!(%peer, connection, elapsed_ms = started.elapsed().as_millis(), "welcome sent");
                Ok(Some(client))
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
                tracing::info!(client = client.0, ?command, "command");
                let result = self.run(command, client).await?;
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

    /// Runs the command once: a retry answers the first run's receipt.
    async fn run(
        &self,
        command: Command,
        client: ClientName,
    ) -> Result<CommandResult, Stopped> {
        let route = serve::route(command.body, client);
        let begun = match route.ledger() {
            Ledger::Unrecorded => return self.command(route).await,
            Ledger::Session => self.log.begin(command.id).await?,
            Ledger::Receipt => self.log.begin_receipt(command.id).await?,
        };
        match begun {
            BeginCommand::Cached(result) => Ok(result),
            BeginCommand::Execute(lease) => {
                let result = self.command(route).await?;
                lease.finish(result.clone()).await?;
                Ok(result)
            }
        }
    }

    async fn command(&self, route: Route) -> Result<CommandResult, Stopped> {
        match route {
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
                self.registry.configure(configure).await
            }
            Route::SignIn(order) => self.registry.sign_in(order).await,
            Route::Home(order) => self.home.order(order).await,
            Route::Finish(session, by) => {
                match self.registry.find(session).await? {
                    Some(handle) => handle.finish(by).await,
                    None => {
                        Ok(CommandResult::Rejected(Rejection::UnknownSession))
                    }
                }
            }
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
            Ok((handle, offered)) => handle
                .order(relaunch.order(offered))
                .await
                .or_else(|stopped| relaunch.closed().ok_or(stopped)),
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
                let (running, sign_ins) = self.registry.watch().await?;
                let mut home = self.home.watch();
                let current = home.borrow_and_update().clone();
                let first = subscription
                    .replay
                    .into_iter()
                    .map(Frame::Event)
                    .chain(running.into_iter().map(Frame::SignIn))
                    .chain(current.into_iter().map(Frame::Home))
                    .collect::<Vec<Frame>>();
                let feed = Feed {
                    send,
                    first: first.into_iter(),
                    live: subscription.live,
                    sign_ins,
                    home,
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
