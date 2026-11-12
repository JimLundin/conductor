//! [`HostLink`]: one client's connection to one host (architecture §9).
//!
//! A single task owns the connection and walks
//! `Disconnected → Connecting → Authenticating → CatchingUp → Live`, falling
//! back to `Disconnected` and retrying with jittered backoff whenever the
//! connection drops. It keeps the cursor (epoch and last applied seq), so
//! every reconnect resubscribes with `after_seq` and the event stream it hands
//! out has no duplicates and no gaps.

use std::cmp::Ordering;
use std::convert::Infallible;
use std::time::Duration;

use iroh::endpoint::{BindError, ConnectError, Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr};
use n0_future::boxed::{BoxFuture, BoxStream};
use n0_future::time::sleep;
use n0_future::{StreamExt, join_all, task};
use proto::wire::{self, WireError};
use proto::{
    AuthToken, ClientName, Command, CommandBody, CommandId, CommandResult, Epoch, Event, EventKind,
    Hello, HelloReply, HostSeq, Open, Providers, Subscribe, Ticket, Welcome,
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::platform::{self, Network};

const UPDATE_BUFFER: usize = 1024;
const BACKOFF_FLOOR: Duration = Duration::from_millis(250);
const BACKOFF_CEILING: Duration = Duration::from_secs(10);
/// How long a dial may run before a second one starts on a fresh endpoint.
/// A relay connection can stall at the network level for seconds while a
/// fresh one is up in well under a second (docs/forensics/reconnect on the
/// `reconnect-forensics` branch).
const HEDGE_AFTER: Duration = Duration::from_millis(1500);

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
                .with_relay_url(proto::without_root_dot(&ticket.relay)),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub state: LinkState,
    /// The host's name from its last `Welcome`.
    pub host: Option<String>,
    /// What the host can run, from its last `Welcome`.
    pub providers: Option<Providers>,
    /// Why the last connection ended, until the next one is live.
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LinkOptions {
    /// Shown as `by` on what this client does.
    pub client: ClientName,
    /// Continues an earlier link's cursor, so the first subscription replays
    /// only what that link had not seen.
    pub resume: Option<Cursor>,
    pub network: Network,
}

impl LinkOptions {
    pub fn new(client: ClientName) -> LinkOptions {
        LinkOptions {
            client,
            resume: None,
            network: Network::Public,
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
    /// What the host runs now, from a command's answer.
    Adopt {
        providers: Providers,
        reply: oneshot::Sender<()>,
    },
    Reconnect,
    Stop {
        reply: oneshot::Sender<()>,
    },
}

impl HostLink {
    /// Binds an endpoint on `options.network` and starts the link on it. A
    /// dial that stalls is retried on a fresh endpoint of the same network,
    /// and the link then runs on whichever connected and closes the other.
    pub async fn spawn(
        target: Target,
        options: LinkOptions,
    ) -> Result<(HostLink, mpsc::Receiver<Update>), BindError> {
        let endpoint = platform::bind_endpoint(options.network).await?;
        let (requests_tx, requests_rx) = mpsc::channel(16);
        let (updates_tx, updates_rx) = mpsc::channel(UPDATE_BUFFER);
        let initial = Status {
            state: LinkState::Disconnected,
            host: None,
            providers: None,
            error: None,
        };
        let (status_tx, status_rx) = watch::channel(initial);
        let link = Link {
            endpoint,
            retiring: Vec::new(),
            network: options.network,
            target,
            client: options.client,
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
        Ok((handle, updates_rx))
    }

    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    /// Runs one command on its own stream. Fails fast while not connected;
    /// the caller decides whether to retry.
    /// A `Providers` answer is in the status by the time this returns.
    pub async fn send(&self, body: CommandBody) -> Result<CommandResult, SendError> {
        let result = self.ask(|reply| Request::Send { body, reply }).await??;
        if let CommandResult::Providers(providers) = &result {
            let providers = providers.clone();
            self.ask(|reply| Request::Adopt { providers, reply })
                .await?;
        }
        Ok(result)
    }

    /// Drops the current connection and connects again at once, as after a
    /// network change.
    pub async fn reconnect(&self) {
        let _ = self.requests.send(Request::Reconnect).await;
    }

    /// Closes the connection and every endpoint the link bound, and returns
    /// once they are closed.
    pub async fn stop(self) -> Result<(), SendError> {
        self.ask(|reply| Request::Stop { reply }).await
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
    /// Endpoints a hedge left behind, still closing.
    retiring: Vec<Endpoint>,
    network: Network,
    target: Target,
    client: ClientName,
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
    /// The subscription. Its read in progress lives inside the stream, so a
    /// `select!` that picks another branch doesn't lose bytes. It owns both
    /// halves, so dropping it ends the subscription.
    feed: BoxStream<Result<Option<Event>, WireError>>,
}

enum Input {
    Dial,
    Request(Option<Request>),
    Frame(Result<Option<Event>, WireError>),
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
                request = self.requests.recv() => Input::Request(request),
            },
        }
    }

    async fn on(&mut self, input: Input) -> Result<(), Stopped> {
        let phase = std::mem::replace(&mut self.phase, Phase::Dialing);
        match self.step(phase, input).await {
            Ok(phase) => {
                self.phase = phase;
                Ok(())
            }
            Err(stopped) => {
                // Also when every handle was dropped without a stop.
                self.close().await;
                Err(stopped)
            }
        }
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
            (idle, Input::Request(Some(Request::Adopt { providers, reply }))) => {
                self.outbox.providers(providers).await?;
                let _ = reply.send(());
                Ok(idle)
            }
            (_, Input::Request(Some(Request::Stop { reply }))) => Err(self.stop(reply).await),
            (_, Input::Request(None)) => Err(Stopped {}),
            (idle, Input::Frame(_)) => Ok(idle),
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
        let dialed = dial_hedged(
            self.network,
            self.endpoint.clone(),
            self.target.addr.clone(),
        )
        .await;
        self.endpoint = dialed.endpoint;
        self.retiring.retain(|endpoint| !endpoint.is_closed());
        self.retiring.extend(dialed.retired);
        let conn = dialed.conn?;
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
        let current = self.outbox.current();
        self.outbox
            .publish(Status {
                host: Some(welcome.host.clone()),
                providers: Some(welcome.providers.clone()),
                state: LinkState::CatchingUp,
                ..current
            })
            .await?;
        self.catch_up(self.cursor_seq(), live_at).await?;
        let feed = frames(subscribe(conn, welcome.epoch, self.cursor_seq()).await?);
        Ok(Connected {
            conn: conn.clone(),
            epoch: welcome.epoch,
            live_at,
            feed,
        })
    }

    /// Handles one input while connected. An error ends the connection.
    async fn carry(&mut self, connected: Connected, input: Input) -> Result<Connected, Lost> {
        match input {
            Input::Request(None) => Err(Lost::Stopped(Stopped {})),
            Input::Request(Some(Request::Reconnect)) => Err(Lost::Reconnect),
            Input::Request(Some(Request::Adopt { providers, reply })) => {
                self.outbox.providers(providers).await?;
                let _ = reply.send(());
                Ok(connected)
            }
            Input::Request(Some(Request::Stop { reply })) => Err(self.stop(reply).await.into()),
            Input::Request(Some(Request::Send { body, reply })) => {
                let command = Command {
                    id: self.next_id,
                    body,
                };
                self.next_id = CommandId(self.next_id.0.wrapping_add(1));
                task::spawn(answer(connected.conn.clone(), command, reply));
                Ok(connected)
            }
            Input::Frame(Ok(Some(event))) => {
                self.apply(event, connected.epoch, connected.live_at)
                    .await?;
                Ok(connected)
            }
            Input::Frame(Ok(None)) => Err(Lost::HostClosed(String::from("subscription"))),
            Input::Frame(Err(err)) => Err(err.into()),
            Input::Dial => Ok(connected),
        }
    }

    async fn stop(&mut self, reply: oneshot::Sender<()>) -> Stopped {
        self.close().await;
        let _ = reply.send(());
        Stopped {}
    }

    /// Closes the endpoint the link runs on and waits out the retired ones.
    async fn close(&mut self) {
        self.endpoint.close().await;
        let _ = join_all(self.retiring.iter().map(Endpoint::closed)).await;
    }

    fn cursor_seq(&self) -> HostSeq {
        self.cursor.map_or(HostSeq(0), |cursor| cursor.seq)
    }

    /// Keeps the cursor if the host's log is the one it points into;
    /// otherwise starts over from the beginning of the new epoch.
    async fn adopt(&mut self, welcome: &Welcome) -> Result<(), Stopped> {
        match self.cursor {
            Some(Cursor { epoch, .. }) if epoch == welcome.epoch => Ok(()),
            Some(_) | None => {
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
        let order = after.0.checked_add(1).map(|next| event.seq.0.cmp(&next));
        match (order, &event.kind) {
            // A delta repeats the seq of the stored event before it, which
            // the stream delivered first (`EventKind::AssistantDelta`).
            (
                Some(Ordering::Less),
                EventKind::AssistantDelta { .. } | EventKind::ReasoningDelta { .. },
            ) => Ok(self.outbox.emit(Update::Event(event)).await?),
            (Some(Ordering::Less), _) => Ok(()),
            (
                Some(Ordering::Equal),
                EventKind::AssistantDelta { .. } | EventKind::ReasoningDelta { .. },
            )
            | (Some(Ordering::Greater) | None, _) => Err(Lost::Gap {
                after: after.0,
                got: event.seq.0,
            }),
            (Some(Ordering::Equal), _) => {
                let seq = event.seq;
                self.cursor = Some(Cursor { epoch, seq });
                self.outbox.emit(Update::Event(event)).await?;
                Ok(self.catch_up(seq, live_at).await?)
            }
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
        let current = self.outbox.current();
        self.outbox
            .publish(Status {
                state: LinkState::Live,
                error: None,
                ..current
            })
            .await
    }
}

impl Outbox {
    async fn disconnected(&self, error: Option<String>) -> Result<(), Stopped> {
        let current = self.current();
        self.publish(Status {
            state: LinkState::Disconnected,
            error,
            ..current
        })
        .await
    }

    async fn providers(&self, providers: Providers) -> Result<(), Stopped> {
        let current = self.current();
        self.publish(Status {
            providers: Some(providers),
            ..current
        })
        .await
    }

    async fn set_state(&self, state: LinkState) -> Result<(), Stopped> {
        let current = self.current();
        self.publish(Status { state, ..current }).await
    }

    fn state(&self) -> LinkState {
        self.status.borrow().state
    }

    fn current(&self) -> Status {
        self.status.borrow().clone()
    }

    /// Shows `next` to every handle, and sends it as an update if it changed.
    async fn publish(&self, next: Status) -> Result<(), Stopped> {
        match self.current() {
            current if current != next => {
                self.status.send_replace(next.clone());
                self.emit(Update::Status(next)).await
            }
            _ => Ok(()),
        }
    }

    async fn emit(&self, update: Update) -> Result<(), Stopped> {
        self.updates.send(update).await.map_err(|_| Stopped {})
    }
}

/// A dial in flight and the endpoint it runs on.
struct Attempt {
    endpoint: Endpoint,
    connecting: BoxFuture<Result<Connection, ConnectError>>,
}

impl Attempt {
    fn start(endpoint: Endpoint, addr: EndpointAddr) -> Attempt {
        Attempt {
            connecting: Box::pin(dial(endpoint.clone(), addr)),
            endpoint,
        }
    }
}

/// How a dial ended, the endpoint the link runs on from now on, and the one
/// a hedge retired.
struct Dialed {
    endpoint: Endpoint,
    conn: Result<Connection, ConnectError>,
    retired: Option<Endpoint>,
}

async fn dial(endpoint: Endpoint, addr: EndpointAddr) -> Result<Connection, ConnectError> {
    endpoint.connect(addr, proto::ALPN).await
}

/// Dials from `endpoint`, and from a fresh endpoint too if the first dial
/// hasn't returned after [`HEDGE_AFTER`].
async fn dial_hedged(network: Network, endpoint: Endpoint, addr: EndpointAddr) -> Dialed {
    let Attempt {
        endpoint,
        mut connecting,
    } = Attempt::start(endpoint, addr.clone());
    tokio::select! {
        conn = &mut connecting => Dialed { endpoint, conn, retired: None },
        () = sleep(HEDGE_AFTER) => hedge(network, Attempt { endpoint, connecting }, addr).await,
    }
}

async fn hedge(network: Network, first: Attempt, addr: EndpointAddr) -> Dialed {
    tracing::info!("dial stalled; dialing again on a fresh endpoint");
    match platform::bind_endpoint(network).await {
        Ok(fresh) => race(first, Attempt::start(fresh, addr)).await,
        Err(err) => {
            tracing::warn!(%err, "no fresh endpoint to hedge with");
            finish(first, None).await
        }
    }
}

/// Keeps whichever attempt connects first.
async fn race(first: Attempt, second: Attempt) -> Dialed {
    let Attempt {
        endpoint: first_endpoint,
        connecting: mut first_connecting,
    } = first;
    let Attempt {
        endpoint: second_endpoint,
        connecting: mut second_connecting,
    } = second;
    tokio::select! {
        conn = &mut first_connecting => {
            let second = Attempt { endpoint: second_endpoint, connecting: second_connecting };
            settle(first_endpoint, conn, second).await
        }
        conn = &mut second_connecting => {
            let first = Attempt { endpoint: first_endpoint, connecting: first_connecting };
            settle(second_endpoint, conn, first).await
        }
    }
}

/// One attempt returned. If it connected, the other is dropped and its
/// endpoint closed; if it failed, its endpoint is closed and the other
/// decides.
async fn settle(
    endpoint: Endpoint,
    conn: Result<Connection, ConnectError>,
    other: Attempt,
) -> Dialed {
    match conn {
        Ok(conn) => {
            tracing::info!(endpoint = %endpoint.id().fmt_short(), "hedged dial connected");
            Dialed {
                endpoint,
                conn: Ok(conn),
                retired: Some(retire(other.endpoint)),
            }
        }
        Err(err) => {
            tracing::debug!(%err, "one hedged dial failed");
            finish(other, Some(retire(endpoint))).await
        }
    }
}

async fn finish(attempt: Attempt, retired: Option<Endpoint>) -> Dialed {
    Dialed {
        conn: attempt.connecting.await,
        endpoint: attempt.endpoint,
        retired,
    }
}

/// Starts closing an endpoint the link no longer uses, without waiting for
/// it, and hands it back so a stop can wait for the close.
fn retire(endpoint: Endpoint) -> Endpoint {
    let closing = endpoint.clone();
    task::spawn(async move { closing.close().await });
    endpoint
}

/// Opens a subscription after `after_seq`.
async fn subscribe(
    conn: &Connection,
    epoch: Epoch,
    after_seq: HostSeq,
) -> Result<(SendStream, RecvStream), Lost> {
    let (mut send, recv) = conn.open_bi().await.map_err(WireError::from)?;
    wire::write(&mut send, &Open::Subscribe(Subscribe { epoch, after_seq })).await?;
    Ok((send, recv))
}

async fn hello(conn: &Connection, hello: Hello) -> Result<Welcome, Lost> {
    let (mut send, mut recv) = conn.open_bi().await.map_err(WireError::from)?;
    wire::write(&mut send, &Open::Hello(hello)).await?;
    match wire::read::<HelloReply>(&mut recv).await? {
        Some(HelloReply::Welcome(welcome)) => Ok(welcome),
        Some(HelloReply::Refused { reason }) => Err(Lost::Refused(reason)),
        None => Err(Lost::HostClosed(String::from("hello"))),
    }
}

fn frames(halves: (SendStream, RecvStream)) -> BoxStream<Result<Option<Event>, WireError>> {
    Box::pin(n0_future::stream::unfold(
        halves,
        |(send, mut recv)| async move {
            let frame = wire::read::<Event>(&mut recv).await;
            Some((frame, (send, recv)))
        },
    ))
}

async fn answer(
    conn: Connection,
    command: Command,
    reply: oneshot::Sender<Result<CommandResult, SendError>>,
) {
    let _ = reply.send(rpc(conn, command).await);
}

async fn rpc(conn: Connection, command: Command) -> Result<CommandResult, SendError> {
    let (mut send, mut recv) = conn.open_bi().await.map_err(WireError::from)?;
    wire::write(&mut send, &Open::Command(command)).await?;
    send.finish().map_err(WireError::from)?;
    wire::read(&mut recv).await?.ok_or(SendError::NoResult)
}

/// Exponential from `floor` to `ceiling`, with the upper half jittered so
/// many clients don't retry in step.
fn backoff(failures: u32, random: u64, floor: Duration, ceiling: Duration) -> Duration {
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

fn random_u64() -> u64 {
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
