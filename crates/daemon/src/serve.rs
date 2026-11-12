//! Serves the protocol (`proto::ALPN`) on accepted iroh connections.

use iroh::endpoint::{ClosedStream, Connection, ConnectionError, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use proto::wire::{self, WireError};
use proto::{
    AuthToken, ClientName, CommandBody, CommandResult, Event, HelloReply, Open, Rejection,
    SessionId, Subscribe, Welcome,
};
use tokio::sync::broadcast;

use crate::Stopped;
use crate::log::Log;
use crate::registry::Registry;
use crate::session::Order;

#[derive(Debug, Clone)]
pub struct Conductor {
    pub log: Log,
    pub registry: Registry,
    /// A placeholder for architecture §4's device certificates: every client
    /// must present the token from the ticket.
    pub token: AuthToken,
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
    Event(Event),
    /// The client fell behind or the log stopped: end the stream, and the
    /// client resubscribes from its cursor.
    Resubscribe,
    /// The connection is gone.
    Gone,
}

/// A gap-free subscription (architecture §8): the replay, then the live feed.
/// The log attaches the feed in the same turn as it takes the replay, so the
/// two neither overlap nor leave a gap.
struct Feed {
    send: SendStream,
    replay: std::vec::IntoIter<Event>,
    live: broadcast::Receiver<Event>,
}

impl ProtocolHandler for Conductor {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let peer = conn.remote_id();
        match self
            .authenticate(&conn)
            .await
            .map_err(AcceptError::from_err)?
        {
            Some(client) => {
                tracing::info!(%peer, client = client.0, "client connected");
                let err = self.streams(&conn, &client).await;
                tracing::info!(%peer, client = client.0, %err, "client disconnected");
            }
            None => {
                tracing::info!(%peer, "refused a client");
                conn.close(1u32.into(), b"refused");
            }
        }
        Ok(())
    }
}

impl Conductor {
    /// The first stream must be a `Hello` with the right token. Returns the
    /// client's name, or `None` if it was refused.
    async fn authenticate(&self, conn: &Connection) -> Result<Option<ClientName>, StreamError> {
        let (mut send, mut recv) = conn.accept_bi().await?;
        let admitted = match wire::read::<Open>(&mut recv).await? {
            Some(Open::Hello(hello)) => hello
                .token
                .check(&self.token)
                .map(|()| hello)
                .map_err(|_| "wrong token"),
            Some(Open::Subscribe(_) | Open::Command(_)) | None => {
                Err("the first stream must be a hello")
            }
        };
        match admitted {
            Ok(hello) => {
                wire::write(&mut send, &HelloReply::Welcome(self.welcome().await?)).await?;
                send.finish()?;
                Ok(Some(hello.client))
            }
            Err(reason) => {
                let refused = HelloReply::Refused {
                    reason: String::from(reason),
                };
                wire::write(&mut send, &refused).await?;
                send.finish()?;
                // Let the refusal reach the client before the close does.
                let _ = send.stopped().await;
                Ok(None)
            }
        }
    }

    async fn welcome(&self) -> Result<Welcome, Stopped> {
        let providers = self.registry.providers().await?;
        self.log.welcome(providers).await
    }

    async fn streams(&self, conn: &Connection, client: &ClientName) -> ConnectionError {
        loop {
            match conn.accept_bi().await {
                Ok((send, recv)) => {
                    let stream = self
                        .clone()
                        .stream(conn.clone(), send, recv, client.clone());
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
        match wire::read::<Open>(&mut recv).await? {
            Some(Open::Command(command)) => {
                tracing::info!(client = client.0, ?command, "command");
                let result = self.command(command.body, client).await?;
                wire::write(&mut send, &result).await?;
                send.finish()?;
            }
            Some(Open::Subscribe(subscribe)) => self.feed(&conn, send, subscribe).await?,
            Some(Open::Hello(_)) | None => send.finish()?,
        }
        Ok(())
    }

    async fn command(
        &self,
        body: CommandBody,
        client: ClientName,
    ) -> Result<CommandResult, Stopped> {
        match body {
            CommandBody::StartSession {
                provider,
                directory,
            } => self.registry.start(provider, directory, client).await,
            CommandBody::SendMessage { session, text } => {
                self.order(session, Order::Send { text, by: client }).await
            }
            CommandBody::Interrupt { session } => self.order(session, Order::Interrupt).await,
            CommandBody::Resolve {
                session,
                request,
                decision,
            } => {
                let order = Order::Resolve {
                    request,
                    decision,
                    by: client,
                };
                self.order(session, order).await
            }
            CommandBody::SetCredential { credential } => {
                self.registry.set_credential(credential).await
            }
            CommandBody::ClearCredential { provider } => {
                self.registry.clear_credential(provider).await
            }
        }
    }

    async fn order(&self, session: SessionId, order: Order) -> Result<CommandResult, Stopped> {
        match self.registry.find(session).await? {
            None => Ok(CommandResult::Rejected(Rejection::UnknownSession)),
            Some(handle) => Ok(handle
                .order(order)
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
        match self.welcome().await? {
            Welcome { epoch, .. } if epoch == subscribe.epoch => {
                let subscription = self.log.subscribe(subscribe.after_seq).await?;
                tracing::info!(
                    after_seq = subscribe.after_seq.0,
                    replayed = subscription.replay.len(),
                    "subscribed"
                );
                let feed = Feed {
                    send,
                    replay: subscription.replay.into_iter(),
                    live: subscription.live,
                };
                let result = feed.run(conn).await;
                tracing::info!(?result, "subscription ended");
                result
            }
            // A cursor from another epoch is refused by closing the stream.
            Welcome { .. } => Ok(send.finish()?),
        }
    }
}

impl Feed {
    async fn run(mut self, conn: &Connection) -> Result<(), StreamError> {
        loop {
            match self.next(conn).await {
                Upcoming::Event(event) => wire::write(&mut self.send, &event).await?,
                Upcoming::Resubscribe => break Ok(self.send.finish()?),
                Upcoming::Gone => break Ok(()),
            }
        }
    }

    async fn next(&mut self, conn: &Connection) -> Upcoming {
        match self.replay.next() {
            Some(event) => Upcoming::Event(event),
            None => tokio::select! {
                next = self.live.recv() => next.map_or(Upcoming::Resubscribe, Upcoming::Event),
                // Without this, an idle feed would outlive its connection.
                _ = conn.closed() => Upcoming::Gone,
            },
        }
    }
}
