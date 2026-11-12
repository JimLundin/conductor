//! Serves `conductor/1` on accepted iroh connections, and on the data
//! channels of WebRTC connections negotiated over them (architecture §5.4).

use std::sync::Arc;

use iroh::endpoint::{Connection, ConnectionError, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use proto::{
    AuthToken, ClientName, CommandBody, CommandResult, Event, HelloReply, Open, Rejection,
    RtcOffer, SessionId, Subscribe,
};
use tokio::sync::{broadcast, mpsc};

use crate::Stopped;
use crate::log::Log;
use crate::registry::Registry;
use crate::rtc::{self, RtcOptions};
use crate::session::Order;
use crate::wire::{RecvHalf, SendHalf, WireError};

#[derive(Debug, Clone)]
pub struct Conductor {
    pub log: Log,
    pub registry: Registry,
    /// A placeholder for architecture §4's device certificates: every client
    /// must present the token from the ticket.
    pub token: AuthToken,
    /// `None` refuses WebRTC upgrades.
    pub rtc: Option<Arc<RtcOptions>>,
}

/// What carries a stream. Only the logs and the feed's lifetime care.
#[derive(Debug, Clone)]
enum Transport {
    Iroh(Connection),
    WebRtc(rtc::Session),
}

impl Transport {
    fn name(&self) -> &str {
        match self {
            Self::Iroh(_) => "iroh",
            Self::WebRtc(_) => "webrtc",
        }
    }

    async fn closed(&self) {
        match self {
            Self::Iroh(conn) => {
                let _ = conn.closed().await;
            }
            Self::WebRtc(session) => session.closed().await,
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum StreamError {
    #[error("{0}")]
    Wire(#[from] WireError),
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
    send: SendHalf,
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
        let (send, recv) = conn.accept_bi().await?;
        let (mut send, mut recv) = (SendHalf::Iroh(send), RecvHalf::Iroh(recv));
        let admitted = match recv.read::<Open>().await? {
            Some(Open::Hello(hello)) => Some(hello)
                .filter(|hello| hello.token == self.token)
                .ok_or("wrong token"),
            Some(Open::Subscribe(_) | Open::Command(_) | Open::Rtc(_)) | None => {
                Err("the first stream must be a hello")
            }
        };
        match admitted {
            Ok(hello) => {
                send.write(&HelloReply::Welcome(self.log.welcome().await?))
                    .await?;
                send.finish()?;
                Ok(Some(hello.client))
            }
            Err(reason) => {
                send.write(&HelloReply::Refused {
                    reason: String::from(reason),
                })
                .await?;
                send.finish()?;
                // Let the refusal reach the client before the close does.
                send.stopped().await;
                Ok(None)
            }
        }
    }

    async fn streams(&self, conn: &Connection, client: &ClientName) -> ConnectionError {
        loop {
            match conn.accept_bi().await {
                Ok((send, recv)) => self.clone().spawn_stream(
                    Transport::Iroh(conn.clone()),
                    SendHalf::Iroh(send),
                    RecvHalf::Iroh(recv),
                    client.clone(),
                ),
                Err(err) => break err,
            }
        }
    }

    fn spawn_stream(
        self,
        transport: Transport,
        send: SendHalf,
        recv: RecvHalf,
        client: ClientName,
    ) {
        tokio::spawn(async move {
            if let Err(err) = self.stream(transport, send, recv, client).await {
                tracing::debug!(%err, "stream ended with an error");
            }
        });
    }

    async fn stream(
        self,
        transport: Transport,
        mut send: SendHalf,
        mut recv: RecvHalf,
        client: ClientName,
    ) -> Result<(), StreamError> {
        match recv.read::<Open>().await? {
            Some(Open::Command(command)) => {
                tracing::info!(
                    client = client.0,
                    transport = transport.name(),
                    ?command,
                    "command"
                );
                send.write(&self.command(command.body, client).await?)
                    .await?;
                send.finish()?;
            }
            Some(Open::Subscribe(subscribe)) => self.feed(&transport, send, subscribe).await?,
            Some(Open::Rtc(offer)) => match (transport, send, recv) {
                // Only an authenticated iroh connection may negotiate WebRTC.
                (Transport::Iroh(_), SendHalf::Iroh(send), RecvHalf::Iroh(recv)) => {
                    self.upgrade(offer, client, send, recv).await?;
                }
                (_, mut send, _) => send.finish()?,
            },
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

    /// Answers a WebRTC offer and serves every data channel the client opens
    /// on it, as long as the connection lives.
    async fn upgrade(
        self,
        offer: RtcOffer,
        client: ClientName,
        mut send: SendStream,
        recv: RecvStream,
    ) -> Result<(), StreamError> {
        match self.rtc.clone() {
            None => {
                tracing::info!(client = client.0, "webrtc offer refused: WebRTC is off");
                Ok(send.finish().map_err(WireError::from)?)
            }
            Some(options) => {
                match rtc::accept(offer, options, client.0.clone(), send, recv).await {
                    Ok(upgrade) => {
                        self.channels(Transport::WebRtc(upgrade.session), upgrade.channels, client)
                            .await
                    }
                    Err(err) => {
                        tracing::warn!(client = client.0, %err, "webrtc offer failed");
                        Ok(())
                    }
                }
            }
        }
    }

    async fn channels(
        self,
        transport: Transport,
        mut channels: mpsc::Receiver<rtc::Stream>,
        client: ClientName,
    ) -> Result<(), StreamError> {
        loop {
            match channels.recv().await {
                Some(rtc::Stream { send, recv }) => {
                    self.clone()
                        .spawn_stream(transport.clone(), send, recv, client.clone());
                }
                None => break Ok(()),
            }
        }
    }

    async fn feed(
        &self,
        transport: &Transport,
        mut send: SendHalf,
        subscribe: Subscribe,
    ) -> Result<(), StreamError> {
        let welcome = self.log.welcome().await?;
        match Some(welcome).filter(|welcome| welcome.epoch == subscribe.epoch) {
            // A cursor from another epoch is refused by closing the stream.
            None => Ok(send.finish()?),
            Some(_) => {
                let subscription = self.log.subscribe(subscribe.after_seq).await?;
                tracing::info!(
                    transport = transport.name(),
                    after_seq = subscribe.after_seq.0,
                    replayed = subscription.replay.len(),
                    "subscribed"
                );
                let feed = Feed {
                    send,
                    replay: subscription.replay.into_iter(),
                    live: subscription.live,
                };
                let result = feed.run(transport).await;
                tracing::info!(transport = transport.name(), ?result, "subscription ended");
                result
            }
        }
    }
}

impl Feed {
    async fn run(mut self, transport: &Transport) -> Result<(), StreamError> {
        loop {
            match self.next(transport).await {
                Upcoming::Event(event) => self.send.write(&event).await?,
                Upcoming::Resubscribe => break Ok(self.send.finish()?),
                Upcoming::Gone => break Ok(()),
            }
        }
    }

    async fn next(&mut self, transport: &Transport) -> Upcoming {
        match self.replay.next() {
            Some(event) => Upcoming::Event(event),
            None => tokio::select! {
                next = self.live.recv() => next.map_or(Upcoming::Resubscribe, Upcoming::Event),
                // Without this, an idle feed would outlive its connection.
                () = transport.closed() => Upcoming::Gone,
            },
        }
    }
}
