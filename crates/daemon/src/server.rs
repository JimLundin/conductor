//! Serves the `conductor/0` protocol on accepted iroh connections.

use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use proto::{AuthToken, CommandResult, Event, HelloReply, Open, Subscribe};
use tokio::sync::broadcast::error::RecvError;

use crate::host::Host;
use crate::wire::{self, WireError};

#[derive(Debug, Clone)]
pub struct Conductor {
    pub host: Host,
    /// S3 placeholder for §4's device certificates: every client must present
    /// the token from the ticket.
    pub token: AuthToken,
}

#[derive(Debug, thiserror::Error)]
enum StreamError {
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error("could not finish the stream: {0}")]
    Closed(#[from] iroh::endpoint::ClosedStream),
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
            let this = self.clone();
            let client = client.clone();
            let conn = conn.clone();
            tokio::spawn(async move {
                if let Err(err) = this.stream(&conn, send, recv, client).await {
                    tracing::debug!(%err, "stream ended with an error");
                }
            });
        }
    }
}

impl Conductor {
    /// The first stream must be a `Hello` with the right token. Returns the
    /// client's name, or `None` if it was refused.
    async fn authenticate(&self, conn: &Connection) -> Result<Option<String>, AcceptError> {
        let (mut send, mut recv) = conn.accept_bi().await?;
        let reply = match wire::read::<Open>(&mut recv)
            .await
            .map_err(AcceptError::from_err)?
        {
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
        wire::write(&mut send, &frame)
            .await
            .map_err(AcceptError::from_err)?;
        send.finish().map_err(AcceptError::from_err)?;
        if client.is_none() {
            // Let the refusal reach the client before the close does.
            send.stopped().await.ok();
        }
        Ok(client)
    }

    async fn stream(
        self,
        conn: &Connection,
        mut send: SendStream,
        mut recv: RecvStream,
        client: String,
    ) -> Result<(), StreamError> {
        match wire::read::<Open>(&mut recv).await? {
            Some(Open::Command(command)) => {
                tracing::info!(client, ?command, "command");
                let result: CommandResult = self.host.command(command.body, client).await?;
                wire::write(&mut send, &result).await?;
                send.finish()?;
            }
            Some(Open::Subscribe(subscribe)) => self.feed(conn, send, subscribe).await?,
            Some(Open::Hello(_)) | None => send.finish()?,
        }
        Ok(())
    }

    /// Gap-free subscribe (§8): the host attached to the live feed before it
    /// took the replay, so drain the replay, then the live feed minus what
    /// the replay already covered.
    async fn feed(
        &self,
        conn: &Connection,
        mut send: SendStream,
        subscribe: Subscribe,
    ) -> Result<(), StreamError> {
        let welcome = self.host.welcome().await?;
        if subscribe.epoch != welcome.epoch {
            send.finish()?;
            return Ok(());
        }
        let sub = self.host.subscribe(subscribe.after_seq).await?;
        tracing::info!(
            after_seq = subscribe.after_seq.0,
            replayed = sub.replay.len(),
            "subscribed"
        );
        let mut last = subscribe.after_seq;
        for event in &sub.replay {
            wire::write(&mut send, event).await?;
            last = event.seq;
        }
        let mut live = sub.live;
        loop {
            let next = tokio::select! {
                next = live.recv() => next,
                // Without this, an idle feed would outlive its connection.
                _ = conn.closed() => return Ok(()),
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
            wire::write(&mut send, &event).await?;
            last = event.seq;
        }
    }
}
