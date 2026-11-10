//! Serves the `conductor/0` protocol on accepted iroh connections, and on the
//! data channels of WebRTC connections negotiated over them (§5.4).

use std::sync::Arc;

use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use proto::{AuthToken, CommandResult, Event, HelloReply, Open, RtcOffer, Subscribe};
use tokio::sync::broadcast::error::RecvError;

use crate::host::Host;
use crate::rtc::{self, RtcOptions};
use crate::wire::{RecvHalf, SendHalf, WireError};

#[derive(Debug, Clone)]
pub struct Conductor {
    pub host: Host,
    /// S3 placeholder for §4's device certificates: every client must present
    /// the token from the ticket.
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
    fn name(&self) -> &'static str {
        match self {
            Self::Iroh(_) => "iroh",
            Self::WebRtc(_) => "webrtc",
        }
    }

    async fn closed(&self) {
        match self {
            Self::Iroh(conn) => {
                conn.closed().await;
            }
            Self::WebRtc(session) => session.closed().await,
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum StreamError {
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error(transparent)]
    Stopped(#[from] crate::host::Stopped),
}

impl ProtocolHandler for Conductor {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let peer = conn.remote_id();
        let Some(client) = self.authenticate(&conn).await? else {
            tracing::info!(%peer, "refused a client");
            conn.close(1u32.into(), b"refused");
            return Ok(());
        };
        tracing::info!(%peer, client, "client connected");
        loop {
            let (send, recv) = match conn.accept_bi().await {
                Ok(streams) => streams,
                Err(err) => {
                    tracing::info!(%peer, client, %err, "client disconnected");
                    return Ok(());
                }
            };
            self.clone().spawn_stream(
                Transport::Iroh(conn.clone()),
                SendHalf::Iroh(send),
                RecvHalf::Iroh(recv),
                client.clone(),
            );
        }
    }
}

impl Conductor {
    /// The first stream must be a `Hello` with the right token. Returns the
    /// client's name, or `None` if it was refused.
    async fn authenticate(&self, conn: &Connection) -> Result<Option<String>, AcceptError> {
        let (send, recv) = conn.accept_bi().await?;
        let (mut send, mut recv) = (SendHalf::Iroh(send), RecvHalf::Iroh(recv));
        let reply = match recv.read::<Open>().await.map_err(AcceptError::from_err)? {
            Some(Open::Hello(hello)) if hello.token == self.token => {
                let welcome = self.host.welcome().await.map_err(AcceptError::from_err)?;
                Ok((hello.client, welcome))
            }
            Some(Open::Hello(_)) => Err("wrong token"),
            _ => Err("the first stream must be a hello"),
        };
        let (client, frame) = match reply {
            Ok((client, welcome)) => (Some(client), HelloReply::Welcome(welcome)),
            Err(reason) => (
                None,
                HelloReply::Refused {
                    reason: reason.into(),
                },
            ),
        };
        send.write(&frame).await.map_err(AcceptError::from_err)?;
        send.finish().map_err(AcceptError::from_err)?;
        if client.is_none() {
            // Let the refusal reach the client before the close does.
            send.stopped().await;
        }
        Ok(client)
    }

    fn spawn_stream(self, transport: Transport, send: SendHalf, recv: RecvHalf, client: String) {
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
        client: String,
    ) -> Result<(), StreamError> {
        match recv.read::<Open>().await? {
            Some(Open::Command(command)) => {
                tracing::info!(client, transport = transport.name(), ?command, "command");
                let result: CommandResult = self.host.command(command.body, client).await?;
                send.write(&result).await?;
                send.finish()?;
            }
            Some(Open::Subscribe(subscribe)) => self.feed(&transport, send, subscribe).await?,
            Some(Open::Rtc(offer)) => match (&transport, send, recv) {
                // Only an authenticated iroh connection may negotiate WebRTC.
                (Transport::Iroh(_), SendHalf::Iroh(send), RecvHalf::Iroh(recv)) => {
                    self.upgrade(offer, client, send, recv).await;
                }
                (_, mut send, _) => send.finish()?,
            },
            Some(Open::Hello(_)) | None => send.finish()?,
        }
        Ok(())
    }

    /// Answers a WebRTC offer and serves every data channel the client opens
    /// on it, as long as the connection lives.
    async fn upgrade(
        self,
        offer: RtcOffer,
        client: String,
        mut send: iroh::endpoint::SendStream,
        recv: iroh::endpoint::RecvStream,
    ) {
        let Some(options) = self.rtc.clone() else {
            tracing::info!(client, "webrtc offer refused: WebRTC is off");
            send.finish().ok();
            return;
        };
        let upgrade = match rtc::accept(offer, options, client.clone(), send, recv).await {
            Ok(upgrade) => upgrade,
            Err(err) => {
                tracing::warn!(client, %err, "webrtc offer failed");
                return;
            }
        };
        let transport = Transport::WebRtc(upgrade.session);
        let mut channels = upgrade.channels;
        while let Some((send, recv)) = channels.recv().await {
            self.clone()
                .spawn_stream(transport.clone(), send, recv, client.clone());
        }
    }

    /// Gap-free subscribe (§8): the host attached to the live feed before it
    /// took the replay, so drain the replay, then the live feed minus what
    /// the replay already covered.
    async fn feed(
        &self,
        transport: &Transport,
        mut send: SendHalf,
        subscribe: Subscribe,
    ) -> Result<(), StreamError> {
        let welcome = self.host.welcome().await?;
        if subscribe.epoch != welcome.epoch {
            send.finish()?;
            return Ok(());
        }
        let sub = self.host.subscribe(subscribe.after_seq).await?;
        tracing::info!(
            transport = transport.name(),
            after_seq = subscribe.after_seq.0,
            replayed = sub.replay.len(),
            "subscribed"
        );
        let mut sent = 0u64;
        let result: Result<(), StreamError> = async {
            let mut last = subscribe.after_seq;
            for event in &sub.replay {
                send.write(event).await?;
                sent += 1;
                last = event.seq;
            }
            let mut live = sub.live;
            loop {
                let next = tokio::select! {
                    next = live.recv() => next,
                    // Without this, an idle feed would outlive its connection.
                    () = transport.closed() => return Ok(()),
                };
                let event: Event = match next {
                    Ok(event) => event,
                    Err(RecvError::Lagged(_)) | Err(RecvError::Closed) => {
                        // The client resubscribes from its cursor.
                        send.finish()?;
                        return Ok(());
                    }
                };
                if event.seq <= last {
                    continue;
                }
                send.write(&event).await?;
                sent += 1;
                last = event.seq;
            }
        }
        .await;
        tracing::info!(
            transport = transport.name(),
            events_sent = sent,
            ended = match &result {
                Ok(()) => "closed".to_string(),
                Err(err) => err.to_string(),
            },
            "subscription ended"
        );
        result
    }
}
