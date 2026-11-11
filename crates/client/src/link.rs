//! [`HostLink`]: one client's connection to one host (architecture §9).
//!
//! A single task owns the connection and walks
//! `Disconnected → Connecting → Authenticating → CatchingUp → Live`, falling
//! back to `Disconnected` and retrying with jittered backoff whenever the
//! connection drops. It keeps the cursor (epoch and last applied seq), so
//! every reconnect resubscribes with `after_seq` and the event stream it hands
//! out has no duplicates and no gaps.
//!
//! In the browser, once the relayed link is live, [`Upgrade`] negotiates a
//! WebRTC connection over it. The link then subscribes again on WebRTC from
//! its cursor and drops the relayed subscription, so the host's replay covers
//! any overlap and [`Link::apply`] skips what it already has. If WebRTC
//! drops, the link subscribes on iroh again the same way and retries later.

use std::time::Duration;

use iroh::endpoint::{Connection, PathList};
use iroh::{Endpoint, EndpointAddr};
use n0_future::time::sleep;
use n0_future::{Stream, StreamExt, task};
use proto::{
    AuthToken, CandidateType, Command, CommandBody, CommandId, CommandResult, Epoch, Event, Hello,
    HelloReply, HostSeq, Open, Subscribe, Ticket, Welcome,
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::upgrade::Upgrade;
use crate::wire::{RecvHalf, SendHalf, Transport, WireError};

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
            addr: EndpointAddr::new(ticket.host)
                .with_relay_url(crate::platform::without_root_dot(&ticket.relay)),
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

/// How the protocol's traffic reaches the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    Unknown,
    /// iroh, hole-punched.
    Direct,
    /// iroh, through a relay.
    Relayed,
    /// A WebRTC data channel, with the selected candidate pair if the
    /// browser reported it.
    WebRtc(Option<CandidatePair>),
}

/// Host/host is the same LAN; srflx or prflx on either end means the path
/// crosses a NAT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidatePair {
    pub local: CandidateType,
    pub remote: CandidateType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub state: LinkState,
    pub path: PathKind,
    /// The host's name from its last `Welcome`.
    pub host: Option<String>,
    /// Why the last connection ended, until the next one is live.
    pub error: Option<String>,
    /// Why the last WebRTC upgrade failed or dropped, until one succeeds.
    pub upgrade_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LinkOptions {
    /// Shown as `by` when this client resolves an approval.
    pub client: String,
    /// Continues an earlier link's cursor, so the first subscription replays
    /// only what that link had not seen.
    pub resume: Option<Cursor>,
    /// `stun:` URLs for the browser's WebRTC upgrade.
    pub stun_servers: Vec<String>,
}

impl LinkOptions {
    pub fn new(client: impl Into<String>) -> Self {
        Self {
            client: client.into(),
            resume: None,
            stun_servers: proto::DEFAULT_STUN_SERVERS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
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
    pub fn spawn(
        endpoint: Endpoint,
        target: Target,
        options: LinkOptions,
    ) -> (Self, mpsc::Receiver<Update>) {
        let (requests_tx, requests_rx) = mpsc::channel(16);
        let (updates_tx, updates_rx) = mpsc::channel(UPDATE_BUFFER);
        let initial = Status {
            state: LinkState::Disconnected,
            path: PathKind::Unknown,
            host: None,
            error: None,
            upgrade_error: None,
        };
        let (status_tx, status_rx) = watch::channel(initial);
        let link = Link {
            endpoint,
            target,
            client: options.client,
            stun_servers: options.stun_servers,
            cursor: options.resume,
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
    stun_servers: Vec<String>,
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
            let delay = backoff(failures, random_u64(), BACKOFF_FLOOR, BACKOFF_CEILING);
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
        self.adopt(&welcome).await?;
        let live_at = welcome.head_seq;
        self.set(|s| {
            s.host = Some(welcome.host.clone());
            s.state = LinkState::CatchingUp;
        })
        .await?;
        if live_at.0 <= self.cursor_seq().0 {
            self.go_live(failures).await?;
        }

        let iroh = Transport::Iroh(conn.clone());
        let mut carrier = iroh.clone();
        let mut feed = std::pin::pin!(frames(self.subscribe(&carrier, welcome.epoch).await?));
        let mut paths = std::pin::pin!(conn.paths_stream());
        let mut iroh_path = PathKind::Unknown;
        let mut upgrade = Upgrade::new(&self.stun_servers);

        loop {
            if self.status.borrow().state == LinkState::Live {
                upgrade.arm();
            }
            let step = tokio::select! {
                frame = feed.next() => Step::Frame(frame.expect("frames() never ends")),
                Some(list) = paths.next() => Step::Path(path_kind(&list)),
                () = carrier.dropped() => Step::Dropped("the WebRTC connection closed".into()),
                upgraded = upgrade.next(conn) => Step::Upgraded(upgraded),
                request = self.requests.recv() => match request {
                    None => return Err(Lost::Stopped),
                    Some(Request::Reconnect) => return Err(Lost::Reconnect),
                    Some(Request::Send { command, reply }) => {
                        let carrier = carrier.clone();
                        task::spawn(async move {
                            reply.send(rpc(carrier, command).await).ok();
                        });
                        continue;
                    }
                },
            };
            let dropped = match step {
                Step::Frame(Ok(Some(event))) => {
                    self.apply(event, live_at, failures).await?;
                    continue;
                }
                Step::Frame(Ok(None)) if carrier.is_webrtc() => {
                    "the host closed the WebRTC subscription".to_string()
                }
                Step::Frame(Err(err)) if carrier.is_webrtc() => err.to_string(),
                Step::Frame(Ok(None)) => return Err(Lost::HostClosed("subscription")),
                Step::Frame(Err(err)) => return Err(err.into()),
                Step::Path(path) => {
                    iroh_path = path;
                    self.set(|s| s.path = carrier.path(iroh_path)).await?;
                    continue;
                }
                Step::Dropped(reason) => reason,
                Step::Upgraded(Ok(rtc)) => {
                    match self.subscribe(&rtc, welcome.epoch).await {
                        Ok(halves) => {
                            // Replaces, and so retires, the relayed subscription.
                            feed.set(frames(halves));
                            carrier = rtc;
                            tracing::info!(path = ?carrier.path(iroh_path), "now on WebRTC");
                            self.set(|s| {
                                s.path = carrier.path(iroh_path);
                                s.upgrade_error = None;
                            })
                            .await?;
                        }
                        Err(err) => {
                            upgrade.retry_later();
                            let reason = format!("could not subscribe over WebRTC: {err}");
                            self.set(|s| s.upgrade_error = Some(reason)).await?;
                        }
                    }
                    continue;
                }
                Step::Upgraded(Err(reason)) => {
                    self.set(|s| s.upgrade_error = Some(reason)).await?;
                    continue;
                }
            };
            tracing::warn!(reason = dropped, "WebRTC dropped; back on iroh");
            carrier = iroh.clone();
            feed.set(frames(self.subscribe(&carrier, welcome.epoch).await?));
            upgrade.retry_later();
            self.set(|s| {
                s.path = carrier.path(iroh_path);
                s.upgrade_error = Some(dropped);
            })
            .await?;
        }
    }

    /// Opens a subscription on `carrier` from the cursor.
    async fn subscribe(
        &self,
        carrier: &Transport,
        epoch: Epoch,
    ) -> Result<(SendHalf, RecvHalf), Lost> {
        let (mut send, recv) = carrier.open_bi().await?;
        let after_seq = self.cursor_seq();
        send.write(&Open::Subscribe(Subscribe { epoch, after_seq }))
            .await?;
        Ok((send, recv))
    }

    fn cursor_seq(&self) -> HostSeq {
        self.cursor.map_or(HostSeq(0), |c| c.seq)
    }

    async fn hello(&mut self, conn: &Connection) -> Result<Welcome, Lost> {
        let (mut send, mut recv) = Transport::Iroh(conn.clone()).open_bi().await?;
        let hello = Hello {
            client: self.client.clone(),
            token: self.target.token,
        };
        send.write(&Open::Hello(hello)).await?;
        match recv.read::<HelloReply>().await? {
            Some(HelloReply::Welcome(welcome)) => Ok(welcome),
            Some(HelloReply::Refused { reason }) => Err(Lost::Refused(reason)),
            None => Err(Lost::HostClosed("hello")),
        }
    }

    /// Keeps the cursor if the host's log is the one it points into;
    /// otherwise starts over from the beginning of the new epoch.
    async fn adopt(&mut self, welcome: &Welcome) -> Result<(), Lost> {
        if let Some(cursor) = self.cursor
            && cursor.epoch == welcome.epoch
        {
            return Ok(());
        }
        self.cursor = Some(Cursor {
            epoch: welcome.epoch,
            seq: HostSeq(0),
        });
        self.emit(Update::Reset(welcome.epoch)).await
    }

    async fn apply(
        &mut self,
        event: Event,
        live_at: HostSeq,
        failures: &mut u32,
    ) -> Result<(), Lost> {
        let cursor = self.cursor.as_mut().expect("adopt() sets the cursor");
        if event.seq.0 <= cursor.seq.0 {
            return Ok(());
        }
        if event.seq.0 != cursor.seq.0 + 1 {
            return Err(Lost::Gap {
                after: cursor.seq.0,
                got: event.seq.0,
            });
        }
        cursor.seq = event.seq;
        let reached = event.seq.0 >= live_at.0;
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

enum Step {
    Frame(Result<Option<Event>, WireError>),
    Path(PathKind),
    Dropped(String),
    Upgraded(Result<Transport, String>),
}

/// A cancel-safe stream of a subscription's events: the read in progress
/// lives inside the stream, so a `select!` that picks another branch doesn't
/// lose bytes. It owns both halves, so dropping it ends the subscription.
fn frames(
    (send, recv): (SendHalf, RecvHalf),
) -> impl Stream<Item = Result<Option<Event>, WireError>> {
    n0_future::stream::unfold((send, recv), |(send, mut recv)| async move {
        let frame = recv.read::<Event>().await;
        Some((frame, (send, recv)))
    })
}

async fn rpc(carrier: Transport, command: Command) -> Result<CommandResult, SendError> {
    let (mut send, mut recv) = carrier.open_bi().await?;
    send.write(&Open::Command(command)).await?;
    send.finish()?;
    recv.read().await?.ok_or(SendError::NoResult)
}

fn path_kind(paths: &PathList<'_>) -> PathKind {
    match paths.iter().find(|p| p.is_selected()) {
        Some(p) if p.is_ip() => PathKind::Direct,
        Some(p) if p.is_relay() => PathKind::Relayed,
        _ => PathKind::Unknown,
    }
}

/// Exponential from `floor` to `ceiling`, with the upper half jittered so
/// many clients don't retry in step.
pub(crate) fn backoff(failures: u32, random: u64, floor: Duration, ceiling: Duration) -> Duration {
    let ceiling = floor
        .saturating_mul(1u32.checked_shl(failures).unwrap_or(u32::MAX))
        .min(ceiling);
    let half = ceiling / 2;
    let jitter_ms = random % (half.as_millis() as u64 + 1);
    half + Duration::from_millis(jitter_ms)
}

pub(crate) fn random_u64() -> u64 {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("the platform has a random source");
    u64::from_le_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_ten_seconds_with_jitter_in_the_upper_half() {
        let ms = |failures, random| {
            backoff(failures, random, BACKOFF_FLOOR, BACKOFF_CEILING).as_millis()
        };
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
