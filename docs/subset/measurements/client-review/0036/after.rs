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

use std::cmp::Ordering;
use std::convert::Infallible;
use std::time::Duration;

use iroh::endpoint::{Connection, PathEvent, PathList};
use iroh::{Endpoint, EndpointAddr, TransportAddr};
use n0_future::boxed::{BoxFuture, BoxStream};
use n0_future::time::sleep;
use n0_future::{StreamExt, task};
use proto::{
    AuthToken, CandidateType, Command, CommandBody, CommandId, CommandResult, Epoch, Event, Hello,
    HelloReply, HostSeq, Open, Subscribe, Ticket, Welcome,
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::platform::Session;
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

impl Target {
    pub fn of_ticket(ticket: &Ticket) -> Target {
        Target {
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
    pub fn new(client: String) -> LinkOptions {
        LinkOptions {
            client,
            resume: None,
            stun_servers: proto::DEFAULT_STUN_SERVERS
                .iter()
                .copied()
                .map(String::from)
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
    #[error("{0}")]
    Wire(#[from] WireError),
}

/// A handle to the link's task. Dropping every handle stops the task and
/// closes the connection.
#[derive(Debug)]
pub struct HostLink {
    requests: mpsc::Sender<Request>,
    status: watch::Receiver<Status>,
}

#[derive(Debug)]
enum Request {
    Send {
        body: CommandBody,
        reply: oneshot::Sender<Result<CommandResult, SendError>>,
    },
    Reconnect,
}

impl HostLink {
    pub fn spawn(
        endpoint: Endpoint,
        target: Target,
        options: LinkOptions,
    ) -> (HostLink, mpsc::Receiver<Update>) {
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
            failures: 0,
            next_id: CommandId(random_u64()),
            requests: requests_rx,
            outbox: Outbox {
                status: status_tx,
                updates: updates_tx,
            },
            phase: Phase::Dialing,
        };
        task::spawn(link.run());
        let handle = HostLink {
            requests: requests_tx,
            status: status_rx,
        };
        (handle, updates_rx)
    }

    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    /// Runs one command on its own stream. Fails fast while not connected;
    /// the caller decides whether to retry.
    pub async fn send(&self, body: CommandBody) -> Result<CommandResult, SendError> {
        self.ask(|reply| Request::Send { body, reply }).await?
    }

    /// Drops the current connection and connects again at once, as after a
    /// network change.
    pub async fn reconnect(&self) {
        let _ = self.requests.send(Request::Reconnect).await;
    }

    async fn ask<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> Request,
    ) -> Result<T, SendError> {
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(build(reply))
            .await
            .map_err(|_| SendError::Stopped)?;
        answer.await.map_err(|_| SendError::Stopped)
    }
}

/// Every [`HostLink`] was dropped: nobody can reach the link any more.
#[derive(Debug, thiserror::Error)]
#[error("the link was dropped")]
struct Stopped {}

/// Why a connection ended.
#[derive(Debug, thiserror::Error)]
enum Lost {
    #[error("{0}")]
    Stopped(#[from] Stopped),
    #[error("reconnect requested")]
    Reconnect,
    #[error("connect failed: {0}")]
    Connect(#[from] iroh::endpoint::ConnectError),
    #[error("connection lost: {0}")]
    Connection(#[from] iroh::endpoint::ConnectionError),
    #[error("{0}")]
    Wire(#[from] WireError),
    #[error("the host refused us: {0}")]
    Refused(String),
    #[error("the host closed the {0} stream")]
    HostClosed(String),
    #[error("event {got} arrived after {after}: the feed has a gap")]
    Gap { after: u64, got: u64 },
}

struct Link {
    endpoint: Endpoint,
    target: Target,
    client: String,
    stun_servers: Vec<String>,
    cursor: Option<Cursor>,
    failures: u32,
    next_id: CommandId,
    requests: mpsc::Receiver<Request>,
    outbox: Outbox,
    phase: Phase,
}

/// What the handles see: the status, and the updates that report it.
struct Outbox {
    status: watch::Sender<Status>,
    updates: mpsc::Sender<Update>,
}

enum Phase {
    /// Connect at once: at the start, and when asked to reconnect.
    Dialing,
    /// Sleeping out the backoff while refusing commands.
    Waiting(BoxFuture<()>),
    Connected(Connected),
}

/// One iroh connection, authenticated and subscribed.
struct Connected {
    conn: Connection,
    epoch: Epoch,
    live_at: HostSeq,
    carrier: Carrier,
    /// The subscription on `carrier`. Its read in progress lives inside the
    /// stream, so a `select!` that picks another branch doesn't lose bytes.
    /// It owns both halves, so dropping it ends the subscription.
    feed: BoxStream<Result<Option<Event>, WireError>>,
    /// `None` now, then each change to the iroh paths.
    paths: BoxStream<Option<PathEvent>>,
    iroh_path: PathKind,
    upgrade: Upgrade,
}

/// What carries the subscription and commands: the iroh connection, or
/// WebRTC once the upgrade succeeds.
enum Carrier {
    Iroh,
    WebRtc(Session),
}

impl Carrier {
    /// Resolves when a WebRTC connection fails or closes. An iroh connection
    /// reports its end through its streams instead, so this never resolves.
    async fn dropped(&self) {
        match self {
            Carrier::Iroh => std::future::pending().await,
            Carrier::WebRtc(session) => session.closed().await,
        }
    }

    fn transport(&self, conn: &Connection) -> Transport {
        match self {
            Carrier::Iroh => Transport::Iroh(conn.clone()),
            Carrier::WebRtc(session) => Transport::WebRtc(session.clone()),
        }
    }

    /// The path to show, given the iroh connection's selected path.
    fn path(&self, iroh: PathKind) -> PathKind {
        match self {
            Carrier::Iroh => iroh,
            Carrier::WebRtc(session) => PathKind::WebRtc(session.pair()),
        }
    }
}

impl Connected {
    /// Drops the WebRTC connection, if any, at once.
    fn on_iroh(self) -> Connected {
        Connected {
            carrier: Carrier::Iroh,
            ..self
        }
    }

    fn transport(&self) -> Transport {
        self.carrier.transport(&self.conn)
    }
}

enum Input {
    Dial,
    Request(Option<Request>),
    Frame(Result<Option<Event>, WireError>),
    PathsChanged,
    /// The WebRTC connection failed or closed.
    Dropped,
    Upgraded(Result<Session, String>),
}

impl Link {
    async fn run(mut self) -> Result<Infallible, Stopped> {
        loop {
            let input = self.next_input().await;
            self.on(input).await?;
        }
    }

    async fn next_input(&mut self) -> Input {
        match &mut self.phase {
            Phase::Dialing => Input::Dial,
            Phase::Waiting(timer) => tokio::select! {
                () = timer => Input::Dial,
                request = self.requests.recv() => Input::Request(request),
            },
            Phase::Connected(connected) => tokio::select! {
                frame = connected.feed.next() => Input::Frame(frame.expect("frames() never ends")),
                Some(_) = connected.paths.next() => Input::PathsChanged,
                () = connected.carrier.dropped() => Input::Dropped,
                upgraded = &mut connected.upgrade.attempt => Input::Upgraded(upgraded),
                request = self.requests.recv() => Input::Request(request),
            },
        }
    }

    async fn on(&mut self, input: Input) -> Result<(), Stopped> {
        let phase = std::mem::replace(&mut self.phase, Phase::Dialing);
        let phase = self.step(phase, input).await?;
        self.phase = self.armed(phase);
        Ok(())
    }

    async fn step(&mut self, phase: Phase, input: Input) -> Result<Phase, Stopped> {
        match (phase, input) {
            (Phase::Connected(connected), input) => self.carry_on(connected, input).await,
            (
                Phase::Dialing | Phase::Waiting(_),
                Input::Dial | Input::Request(Some(Request::Reconnect)),
            ) => self.dial().await,
            (idle, Input::Request(Some(Request::Send { reply, .. }))) => {
                let _ = reply.send(Err(SendError::NotConnected));
                Ok(idle)
            }
            (_, Input::Request(None)) => Err(Stopped {}),
            (idle, Input::Frame(_) | Input::PathsChanged | Input::Dropped | Input::Upgraded(_)) => {
                Ok(idle)
            }
        }
    }

    /// Starts the upgrade once the link is live.
    fn armed(&self, phase: Phase) -> Phase {
        let state = self.outbox.state();
        match phase {
            Phase::Connected(connected) => Phase::Connected(Connected {
                upgrade: connected.upgrade.arm(state, &connected.conn),
                ..connected
            }),
            idle => idle,
        }
    }

    async fn dial(&mut self) -> Result<Phase, Stopped> {
        match self.connect().await {
            Ok(connected) => Ok(Phase::Connected(connected)),
            Err(lost) => self.down(lost).await,
        }
    }

    async fn carry_on(&mut self, connected: Connected, input: Input) -> Result<Phase, Stopped> {
        let conn = connected.conn.clone();
        match self.carry(connected, input).await {
            Ok(connected) => Ok(Phase::Connected(connected)),
            Err(lost) => {
                conn.close(0u32.into(), b"bye");
                self.down(lost).await
            }
        }
    }

    /// After a connection ended: report it, then reconnect at once or after
    /// the backoff.
    async fn down(&mut self, lost: Lost) -> Result<Phase, Stopped> {
        tracing::debug!(%lost, "host link down");
        match lost {
            Lost::Stopped(stopped) => Err(stopped),
            Lost::Reconnect => {
                self.outbox.disconnected(None).await?;
                Ok(Phase::Dialing)
            }
            lost => {
                self.outbox.disconnected(Some(lost.to_string())).await?;
                let delay = backoff(self.failures, random_u64(), BACKOFF_FLOOR, BACKOFF_CEILING);
                self.failures = self.failures.saturating_add(1);
                Ok(Phase::Waiting(Box::pin(sleep(delay))))
            }
        }
    }

    async fn connect(&mut self) -> Result<Connected, Lost> {
        self.outbox.set_state(LinkState::Connecting).await?;
        let conn = self
            .endpoint
            .connect(self.target.addr.clone(), proto::ALPN)
            .await?;
        match self.start(&conn).await {
            Ok(connected) => Ok(connected),
            Err(lost) => {
                conn.close(0u32.into(), b"bye");
                Err(lost)
            }
        }
    }

    async fn start(&mut self, conn: &Connection) -> Result<Connected, Lost> {
        self.outbox.set_state(LinkState::Authenticating).await?;
        let welcome = hello(
            conn,
            Hello {
                client: self.client.clone(),
                token: self.target.token,
            },
        )
        .await?;
        self.adopt(&welcome).await?;
        let live_at = welcome.head_seq;
        self.outbox
            .publish(Status {
                host: Some(welcome.host.clone()),
                state: LinkState::CatchingUp,
                ..self.outbox.current()
            })
            .await?;
        self.catch_up(self.cursor_seq(), live_at).await?;
        let iroh = Transport::Iroh(conn.clone());
        let feed = frames(subscribe(&iroh, welcome.epoch, self.cursor_seq()).await?);
        Ok(Connected {
            conn: conn.clone(),
            epoch: welcome.epoch,
            live_at,
            carrier: Carrier::Iroh,
            feed,
            paths: Box::pin(n0_future::stream::once(None).chain(conn.path_events().map(Some))),
            iroh_path: PathKind::Unknown,
            upgrade: Upgrade::new(self.stun_servers.clone()),
        })
    }

    /// Handles one input while connected. An error ends the connection.
    async fn carry(&mut self, connected: Connected, input: Input) -> Result<Connected, Lost> {
        match input {
            Input::Request(None) => Err(Lost::Stopped(Stopped {})),
            Input::Request(Some(Request::Reconnect)) => Err(Lost::Reconnect),
            Input::Request(Some(Request::Send { body, reply })) => {
                let command = Command {
                    id: self.next_id,
                    body,
                };
                self.next_id = CommandId(self.next_id.0.wrapping_add(1));
                task::spawn(answer(connected.transport(), command, reply));
                Ok(connected)
            }
            Input::Frame(Ok(Some(event))) => {
                self.apply(event, connected.epoch, connected.live_at)
                    .await?;
                Ok(connected)
            }
            Input::Frame(Ok(None)) => match &connected.carrier {
                Carrier::Iroh => Err(Lost::HostClosed(String::from("subscription"))),
                Carrier::WebRtc(_) => {
                    let reason = String::from("the host closed the WebRTC subscription");
                    self.fall_back(connected, reason).await
                }
            },
            Input::Frame(Err(err)) => match &connected.carrier {
                Carrier::Iroh => Err(err.into()),
                Carrier::WebRtc(_) => self.fall_back(connected, err.to_string()).await,
            },
            Input::PathsChanged => {
                let iroh_path = path_kind(&connected.conn.paths());
                self.outbox
                    .publish(Status {
                        path: connected.carrier.path(iroh_path),
                        ..self.outbox.current()
                    })
                    .await?;
                Ok(Connected {
                    iroh_path,
                    ..connected
                })
            }
            Input::Dropped => {
                let reason = String::from("the WebRTC connection closed");
                self.fall_back(connected, reason).await
            }
            Input::Upgraded(Ok(session)) => {
                self.upgraded(connected, Carrier::WebRtc(session)).await
            }
            Input::Upgraded(Err(reason)) => {
                self.outbox
                    .publish(Status {
                        upgrade_error: Some(reason),
                        ..self.outbox.current()
                    })
                    .await?;
                Ok(Connected {
                    upgrade: connected.upgrade.retry_later(&connected.conn),
                    ..connected
                })
            }
            Input::Dial => Ok(connected),
        }
    }

    /// Moves the subscription onto the new WebRTC connection.
    async fn upgraded(&mut self, connected: Connected, rtc: Carrier) -> Result<Connected, Lost> {
        let upgrade = connected.upgrade.succeeded();
        let transport = rtc.transport(&connected.conn);
        match subscribe(&transport, connected.epoch, self.cursor_seq()).await {
            Ok(halves) => {
                // Replaces, and so retires, the relayed subscription.
                let relayed = connected.feed;
                drop(relayed);
                let next = Connected {
                    carrier: rtc,
                    feed: frames(halves),
                    upgrade,
                    ..connected
                };
                let path = next.carrier.path(next.iroh_path);
                tracing::info!(?path, "now on WebRTC");
                self.outbox
                    .publish(Status {
                        path,
                        upgrade_error: None,
                        ..self.outbox.current()
                    })
                    .await?;
                Ok(next)
            }
            Err(err) => {
                let reason = format!("could not subscribe over WebRTC: {err}");
                self.outbox
                    .publish(Status {
                        upgrade_error: Some(reason),
                        ..self.outbox.current()
                    })
                    .await?;
                Ok(Connected {
                    upgrade: upgrade.retry_later(&connected.conn),
                    ..connected
                })
            }
        }
    }

    /// WebRTC dropped: subscribe on iroh again from the cursor and retry the
    /// upgrade later.
    async fn fall_back(&mut self, connected: Connected, reason: String) -> Result<Connected, Lost> {
        tracing::warn!(reason, "WebRTC dropped; back on iroh");
        let connected = connected.on_iroh();
        let iroh = Transport::Iroh(connected.conn.clone());
        let feed = frames(subscribe(&iroh, connected.epoch, self.cursor_seq()).await?);
        let webrtc = connected.feed;
        drop(webrtc);
        let next = Connected {
            feed,
            upgrade: connected.upgrade.retry_later(&connected.conn),
            ..connected
        };
        self.outbox
            .publish(Status {
                path: next.carrier.path(next.iroh_path),
                upgrade_error: Some(reason),
                ..self.outbox.current()
            })
            .await?;
        Ok(next)
    }

    fn cursor_seq(&self) -> HostSeq {
        self.cursor.map_or(HostSeq(0), |cursor| cursor.seq)
    }

    /// Keeps the cursor if the host's log is the one it points into;
    /// otherwise starts over from the beginning of the new epoch.
    async fn adopt(&mut self, welcome: &Welcome) -> Result<(), Stopped> {
        match self.cursor.filter(|cursor| cursor.epoch == welcome.epoch) {
            Some(_) => Ok(()),
            None => {
                self.cursor = Some(Cursor {
                    epoch: welcome.epoch,
                    seq: HostSeq(0),
                });
                self.outbox.emit(Update::Reset(welcome.epoch)).await
            }
        }
    }

    async fn apply(&mut self, event: Event, epoch: Epoch, live_at: HostSeq) -> Result<(), Lost> {
        let after = self.cursor_seq();
        match after.0.checked_add(1).map(|next| event.seq.0.cmp(&next)) {
            Some(Ordering::Less) => Ok(()),
            Some(Ordering::Equal) => {
                let seq = event.seq;
                self.cursor = Some(Cursor { epoch, seq });
                self.outbox.emit(Update::Event(event)).await?;
                Ok(self.catch_up(seq, live_at).await?)
            }
            Some(Ordering::Greater) | None => Err(Lost::Gap {
                after: after.0,
                got: event.seq.0,
            }),
        }
    }

    /// Goes live once the link has applied the host's head as of `Welcome`.
    async fn catch_up(&mut self, seq: HostSeq, live_at: HostSeq) -> Result<(), Stopped> {
        let state = self.outbox.state();
        if let (Ordering::Greater | Ordering::Equal, LinkState::CatchingUp) =
            (seq.cmp(&live_at), state)
        {
            self.go_live().await?;
        }
        Ok(())
    }

    async fn go_live(&mut self) -> Result<(), Stopped> {
        self.failures = 0;
        self.outbox
            .publish(Status {
                state: LinkState::Live,
                error: None,
                ..self.outbox.current()
            })
            .await
    }
}

impl Outbox {
    async fn disconnected(&self, error: Option<String>) -> Result<(), Stopped> {
        self.publish(Status {
            state: LinkState::Disconnected,
            path: PathKind::Unknown,
            error,
            ..self.current()
        })
        .await
    }

    async fn set_state(&self, state: LinkState) -> Result<(), Stopped> {
        self.publish(Status {
            state,
            ..self.current()
        })
        .await
    }

    fn state(&self) -> LinkState {
        self.status.borrow().state
    }

    fn current(&self) -> Status {
        self.status.borrow().clone()
    }

    /// Shows `next` to every handle, and sends it as an update if it changed.
    async fn publish(&self, next: Status) -> Result<(), Stopped> {
        let changed = self.status.borrow().ne(&next).then_some(());
        if let Some(()) = changed {
            self.status.send_replace(next.clone());
            self.emit(Update::Status(next)).await?;
        }
        Ok(())
    }

    async fn emit(&self, update: Update) -> Result<(), Stopped> {
        self.updates.send(update).await.map_err(|_| Stopped {})
    }
}

/// Opens a subscription on `carrier` after `after_seq`.
async fn subscribe(
    carrier: &Transport,
    epoch: Epoch,
    after_seq: HostSeq,
) -> Result<(SendHalf, RecvHalf), Lost> {
    let (send, recv) = carrier.open_bi().await?;
    let send = send
        .write(&Open::Subscribe(Subscribe { epoch, after_seq }))
        .await?;
    Ok((send, recv))
}

async fn hello(conn: &Connection, hello: Hello) -> Result<Welcome, Lost> {
    let (send, recv) = Transport::Iroh(conn.clone()).open_bi().await?;
    let _send = send.write(&Open::Hello(hello)).await?;
    let (_recv, reply) = recv.read::<HelloReply>().await;
    match reply? {
        Some(HelloReply::Welcome(welcome)) => Ok(welcome),
        Some(HelloReply::Refused { reason }) => Err(Lost::Refused(reason)),
        None => Err(Lost::HostClosed(String::from("hello"))),
    }
}

fn frames(halves: (SendHalf, RecvHalf)) -> BoxStream<Result<Option<Event>, WireError>> {
    Box::pin(n0_future::stream::unfold(
        halves,
        |(send, recv)| async move {
            let (recv, frame) = recv.read::<Event>().await;
            Some((frame, (send, recv)))
        },
    ))
}

async fn answer(
    carrier: Transport,
    command: Command,
    reply: oneshot::Sender<Result<CommandResult, SendError>>,
) {
    let _ = reply.send(rpc(carrier, command).await);
}

async fn rpc(carrier: Transport, command: Command) -> Result<CommandResult, SendError> {
    let (send, recv) = carrier.open_bi().await?;
    let send = send.write(&Open::Command(command)).await?;
    let _send = send.finish()?;
    let (_recv, result) = recv.read().await;
    result?.ok_or(SendError::NoResult)
}

fn path_kind(paths: &PathList) -> PathKind {
    paths
        .iter()
        .find(|path| path.is_selected())
        .map_or(PathKind::Unknown, |path| match path.remote_addr() {
            TransportAddr::Ip(_) => PathKind::Direct,
            TransportAddr::Relay(_) => PathKind::Relayed,
            _ => PathKind::Unknown,
        })
}

/// Exponential from `floor` to `ceiling`, with the upper half jittered so
/// many clients don't retry in step.
pub(crate) fn backoff(failures: u32, random: u64, floor: Duration, ceiling: Duration) -> Duration {
    let ceiling = floor
        .saturating_mul(1u32.checked_shl(failures).unwrap_or(u32::MAX))
        .min(ceiling);
    let half = ceiling.checked_div(2).unwrap_or(ceiling);
    let span = u64::try_from(half.as_millis())
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let jitter = Duration::from_millis(random.checked_rem(span).unwrap_or(0));
    half.saturating_add(jitter)
}

pub(crate) fn random_u64() -> u64 {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("the platform has a random source");
    u64::from_le_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::{BACKOFF_CEILING, BACKOFF_FLOOR, backoff};

    fn ms(failures: u32, random: u64) -> u128 {
        backoff(failures, random, BACKOFF_FLOOR, BACKOFF_CEILING).as_millis()
    }

    #[test]
    fn backoff_doubles_to_ten_seconds_with_jitter_in_the_upper_half() {
        assert_eq!(
            Vec::from([
                ms(0, 0),
                ms(0, 125),
                ms(1, 0),
                ms(1, 250),
                ms(3, 0),
                ms(6, 0),
                ms(6, 5000),
                ms(40, 0),
            ]),
            Vec::from([125, 250, 250, 500, 1000, 5000, 10_000, 5000])
        );
    }
}
