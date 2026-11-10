//! [`HostLink`]: one client's connection to one host (architecture §9).
//!
//! A single task owns the connection and walks
//! `Disconnected → Connecting → Authenticating → CatchingUp → Live`, falling
//! back to `Disconnected` and retrying with jittered backoff whenever the
//! connection drops. It keeps the cursor (epoch and last applied seq), so
//! every reconnect resubscribes with `after_seq` and the event stream it hands
//! out has no duplicates and no gaps.

use std::time::Duration;

use iroh::endpoint::{Connection, PathList};
use iroh::{Endpoint, EndpointAddr};
use n0_future::time::sleep;
use n0_future::{Stream, StreamExt, task};
use proto::{
    AuthToken, Command, CommandBody, CommandId, CommandResult, Epoch, Event, Hello, HelloReply,
    HostSeq, Open, Subscribe, Ticket, Welcome,
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::wire::{self, WireError};

const UPDATE_BUFFER: usize = 1024;
const BACKOFF_FLOOR: Duration = Duration::from_millis(250);
const BACKOFF_CEILING: Duration = Duration::from_secs(10);

/// Where a host is and the secret it expects in `Hello`.
#[derive(Debug, Clone)]
pub struct Target {
    pub addr: EndpointAddr,
    pub token: AuthToken,
}

impl From<&Ticket> for Target {
    fn from(ticket: &Ticket) -> Self {
        Self {
            addr: EndpointAddr::new(ticket.host).with_relay_url(ticket.relay.clone()),
            token: ticket.token,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Disconnected,
    Connecting,
    Authenticating,
    CatchingUp,
    Live,
}

/// How the connection's selected path reaches the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    Unknown,
    Direct,
    Relayed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub state: LinkState,
    pub path: PathKind,
    /// The host's name from its last `Welcome`.
    pub host: Option<String>,
    /// Why the last connection ended, until the next one is live.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub epoch: Epoch,
    pub seq: HostSeq,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    Status(Status),
    /// The events that follow belong to this epoch, starting after seq 0.
    /// Anything folded from an earlier epoch is stale.
    Reset(Epoch),
    Event(Event),
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("not connected to the host")]
    NotConnected,
    #[error("the link has stopped")]
    Stopped,
    #[error("the host closed the stream without a result")]
    NoResult,
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error("could not open a stream: {0}")]
    Connection(#[from] iroh::endpoint::ConnectionError),
    #[error("could not finish the stream: {0}")]
    Closed(#[from] iroh::endpoint::ClosedStream),
}

/// A handle to the link's task. Dropping every handle stops the task and
/// closes the connection.
#[derive(Debug)]
pub struct HostLink {
    requests: mpsc::Sender<Request>,
    status: watch::Receiver<Status>,
    next_id: std::sync::atomic::AtomicU64,
}

#[derive(Debug)]
enum Request {
    Send {
        command: Command,
        reply: oneshot::Sender<Result<CommandResult, SendError>>,
    },
    Reconnect,
}

impl HostLink {
    /// Starts connecting. `resume` continues an earlier link's cursor, so the
    /// first subscription replays only what that link had not seen.
    pub fn spawn(
        endpoint: Endpoint,
        target: Target,
        client: String,
        resume: Option<Cursor>,
    ) -> (Self, mpsc::Receiver<Update>) {
        let (requests_tx, requests_rx) = mpsc::channel(16);
        let (updates_tx, updates_rx) = mpsc::channel(UPDATE_BUFFER);
        let initial = Status {
            state: LinkState::Disconnected,
            path: PathKind::Unknown,
            host: None,
            error: None,
        };
        let (status_tx, status_rx) = watch::channel(initial);
        let link = Link {
            endpoint,
            target,
            client,
            cursor: resume,
            requests: requests_rx,
            updates: updates_tx,
            status: status_tx,
        };
        task::spawn(link.run());
        let handle = Self {
            requests: requests_tx,
            status: status_rx,
            next_id: random_u64().into(),
        };
        (handle, updates_rx)
    }

    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    /// Runs one command on its own stream. Fails fast while not connected;
    /// the caller decides whether to retry.
    pub async fn send(&self, body: CommandBody) -> Result<CommandResult, SendError> {
        let id = CommandId(
            self.next_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        );
        let (reply, result) = oneshot::channel();
        let command = Command { id, body };
        self.requests
            .send(Request::Send { command, reply })
            .await
            .map_err(|_| SendError::Stopped)?;
        result.await.map_err(|_| SendError::Stopped)?
    }

    /// Drops the current connection and connects again at once, as after a
    /// network change.
    pub async fn reconnect(&self) {
        self.requests.send(Request::Reconnect).await.ok();
    }
}

/// Why a connection ended.
#[derive(Debug, thiserror::Error)]
enum Lost {
    #[error("the link was dropped")]
    Stopped,
    #[error("reconnect requested")]
    Reconnect,
    #[error("connect failed: {0}")]
    Connect(#[from] iroh::endpoint::ConnectError),
    #[error("connection lost: {0}")]
    Connection(#[from] iroh::endpoint::ConnectionError),
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error("the host refused us: {0}")]
    Refused(String),
    #[error("the host closed the {0} stream")]
    HostClosed(&'static str),
    #[error("event {got} arrived after {after}: the feed has a gap")]
    Gap { after: u64, got: u64 },
}

struct Link {
    endpoint: Endpoint,
    target: Target,
    client: String,
    cursor: Option<Cursor>,
    requests: mpsc::Receiver<Request>,
    updates: mpsc::Sender<Update>,
    status: watch::Sender<Status>,
}

impl Link {
    async fn run(mut self) {
        let mut failures = 0u32;
        loop {
            let Err(lost) = self.connect_and_pump(&mut failures).await;
            tracing::debug!(%lost, "host link down");
            let error = match lost {
                Lost::Stopped => return,
                Lost::Reconnect => None,
                lost => Some(lost.to_string()),
            };
            let retry_now = error.is_none();
            if self
                .set(|s| {
                    s.state = LinkState::Disconnected;
                    s.path = PathKind::Unknown;
                    s.error = error;
                })
                .await
                .is_err()
            {
                return;
            }
            if retry_now {
                continue;
            }
            let delay = backoff(failures, random_u64());
            failures = failures.saturating_add(1);
            if !self.wait(delay).await {
                return;
            }
        }
    }

    async fn connect_and_pump(
        &mut self,
        failures: &mut u32,
    ) -> Result<std::convert::Infallible, Lost> {
        self.set_state(LinkState::Connecting).await?;
        let conn = self
            .endpoint
            .connect(self.target.addr.clone(), proto::ALPN)
            .await?;
        let result = self.pump(&conn, failures).await;
        conn.close(0u32.into(), b"bye");
        result
    }

    async fn pump(
        &mut self,
        conn: &Connection,
        failures: &mut u32,
    ) -> Result<std::convert::Infallible, Lost> {
        self.set_state(LinkState::Authenticating).await?;
        let welcome = self.hello(conn).await?;
        let after_seq = self.adopt(&welcome).await?;
        let live_at = welcome.head_seq;
        self.set(|s| {
            s.host = Some(welcome.host.clone());
            s.state = LinkState::CatchingUp;
        })
        .await?;
        if live_at <= after_seq {
            self.go_live(failures).await?;
        }

        let (mut send, recv) = conn.open_bi().await?;
        wire::write(
            &mut send,
            &Open::Subscribe(Subscribe {
                epoch: welcome.epoch,
                after_seq,
            }),
        )
        .await?;
        let mut feed = std::pin::pin!(frames(recv));
        let mut paths = std::pin::pin!(conn.paths_stream());

        loop {
            tokio::select! {
                frame = feed.next() => {
                    let event = frame.expect("frames() never ends")?.ok_or(Lost::HostClosed("subscription"))?;
                    self.apply(event, live_at, failures).await?;
                }
                Some(list) = paths.next() => {
                    let path = path_kind(&list);
                    self.set(|s| s.path = path).await?;
                }
                request = self.requests.recv() => match request {
                    None => return Err(Lost::Stopped),
                    Some(Request::Reconnect) => return Err(Lost::Reconnect),
                    Some(Request::Send { command, reply }) => {
                        let conn = conn.clone();
                        task::spawn(async move {
                            reply.send(rpc(conn, command).await).ok();
                        });
                    }
                },
            }
        }
    }

    async fn hello(&mut self, conn: &Connection) -> Result<Welcome, Lost> {
        let (mut send, mut recv) = conn.open_bi().await?;
        let hello = Hello {
            client: self.client.clone(),
            token: self.target.token,
        };
        wire::write(&mut send, &Open::Hello(hello)).await?;
        match wire::read::<HelloReply>(&mut recv).await? {
            Some(HelloReply::Welcome(welcome)) => Ok(welcome),
            Some(HelloReply::Refused { reason }) => Err(Lost::Refused(reason)),
            None => Err(Lost::HostClosed("hello")),
        }
    }

    /// Keeps the cursor if the host's log is the one it points into;
    /// otherwise starts over from the beginning of the new epoch.
    async fn adopt(&mut self, welcome: &Welcome) -> Result<HostSeq, Lost> {
        if let Some(cursor) = self.cursor
            && cursor.epoch == welcome.epoch
        {
            return Ok(cursor.seq);
        }
        self.cursor = Some(Cursor {
            epoch: welcome.epoch,
            seq: HostSeq(0),
        });
        self.emit(Update::Reset(welcome.epoch)).await?;
        Ok(HostSeq(0))
    }

    async fn apply(
        &mut self,
        event: Event,
        live_at: HostSeq,
        failures: &mut u32,
    ) -> Result<(), Lost> {
        let cursor = self.cursor.as_mut().expect("adopt() sets the cursor");
        if event.seq <= cursor.seq {
            return Ok(());
        }
        if event.seq.0 != cursor.seq.0 + 1 {
            return Err(Lost::Gap {
                after: cursor.seq.0,
                got: event.seq.0,
            });
        }
        cursor.seq = event.seq;
        let reached = event.seq >= live_at;
        self.emit(Update::Event(event)).await?;
        if reached && self.status.borrow().state == LinkState::CatchingUp {
            self.go_live(failures).await?;
        }
        Ok(())
    }

    async fn go_live(&mut self, failures: &mut u32) -> Result<(), Lost> {
        *failures = 0;
        self.set(|s| {
            s.state = LinkState::Live;
            s.error = None;
        })
        .await
    }

    /// Sleeps out the backoff while refusing commands. Returns `false` if the
    /// link should stop.
    async fn wait(&mut self, delay: Duration) -> bool {
        let mut timer = std::pin::pin!(sleep(delay));
        loop {
            tokio::select! {
                () = &mut timer => return true,
                request = self.requests.recv() => match request {
                    None => return false,
                    Some(Request::Reconnect) => return true,
                    Some(Request::Send { reply, .. }) => {
                        reply.send(Err(SendError::NotConnected)).ok();
                    }
                },
            }
        }
    }

    async fn set_state(&mut self, state: LinkState) -> Result<(), Lost> {
        self.set(|s| s.state = state).await
    }

    async fn set(&mut self, change: impl FnOnce(&mut Status)) -> Result<(), Lost> {
        let mut next = self.status.borrow().clone();
        change(&mut next);
        if *self.status.borrow() == next {
            return Ok(());
        }
        self.status.send_replace(next.clone());
        self.emit(Update::Status(next)).await
    }

    async fn emit(&self, update: Update) -> Result<(), Lost> {
        self.updates.send(update).await.map_err(|_| Lost::Stopped)
    }
}

/// A cancel-safe stream of frames: the read in progress lives inside the
/// stream, so a `select!` that picks another branch doesn't lose bytes.
fn frames(
    recv: iroh::endpoint::RecvStream,
) -> impl Stream<Item = Result<Option<Event>, WireError>> {
    n0_future::stream::unfold(recv, |mut recv| async move {
        let frame = wire::read::<Event>(&mut recv).await;
        Some((frame, recv))
    })
}

async fn rpc(conn: Connection, command: Command) -> Result<CommandResult, SendError> {
    let (mut send, mut recv) = conn.open_bi().await?;
    wire::write(&mut send, &Open::Command(command)).await?;
    send.finish()?;
    wire::read(&mut recv).await?.ok_or(SendError::NoResult)
}

fn path_kind(paths: &PathList<'_>) -> PathKind {
    match paths.iter().find(|p| p.is_selected()) {
        Some(p) if p.is_ip() => PathKind::Direct,
        Some(p) if p.is_relay() => PathKind::Relayed,
        _ => PathKind::Unknown,
    }
}

/// Exponential from 250ms to 10s, with the upper half jittered so many
/// clients don't retry in step.
fn backoff(failures: u32, random: u64) -> Duration {
    let ceiling = BACKOFF_FLOOR
        .saturating_mul(1u32.checked_shl(failures).unwrap_or(u32::MAX))
        .min(BACKOFF_CEILING);
    let half = ceiling / 2;
    let jitter_ms = random % (half.as_millis() as u64 + 1);
    half + Duration::from_millis(jitter_ms)
}

fn random_u64() -> u64 {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("the platform has a random source");
    u64::from_le_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_ten_seconds_with_jitter_in_the_upper_half() {
        let ms = |failures, random| backoff(failures, random).as_millis();
        assert_eq!(ms(0, 0), 125);
        assert_eq!(ms(0, 125), 250);
        assert_eq!(ms(1, 0), 250);
        assert_eq!(ms(1, 250), 500);
        assert_eq!(ms(3, 0), 1000);
        assert_eq!(ms(6, 0), 5000);
        assert_eq!(ms(6, 5000), 10_000);
        assert_eq!(ms(40, 0), 5000);
    }
}
