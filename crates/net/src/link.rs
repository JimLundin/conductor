//! [`HostLink`]: one client's connection to one host (architecture §9).
//!
//! The link's decisions are the state machine in [`client::link`]. A single
//! task owns the endpoint, the connection and its streams: it turns what
//! they do into an [`Input`], asks the core, and performs each [`Effect`].

use std::convert::Infallible;
use std::future::{pending, ready};
use std::time::Duration;

use crate::wire::{self, WireError};
use iroh::endpoint::{
    BindError, ConnectError, Connection, RecvStream, SendStream,
};
use iroh::{Endpoint, EndpointAddr};
use n0_future::boxed::BoxFuture;
use n0_future::time::{Instant, sleep, timeout};
use n0_future::{join_all, task};

use client::host_view::HostView;
pub use client::link::{Cursor, LinkState, Status, Update};
use client::link::{Effect, Input, Link as Core, Lost, Operation};
use proto::{
    AuthToken, ClientName, Command, CommandBody, CommandId, CommandResult,
    Event, Hello, HelloReply, Open, Providers, Subscribe, Ticket, TicketError,
};
use tokio::sync::{mpsc, oneshot, watch};

use crate::platform::{self, Network};

const UPDATE_BUFFER: usize = 1024;

/// Where a host is and the secret it expects in `Hello`.
#[derive(Debug, Clone)]
pub struct Target {
    pub addr: EndpointAddr,
    pub token: AuthToken,
}

impl Target {
    /// # Errors
    ///
    /// The [`TicketError`] of [`crate::address`].
    pub fn of_ticket(ticket: &Ticket) -> Result<Self, TicketError> {
        Ok(Self {
            addr: crate::address(ticket)?,
            token: ticket.token,
        })
    }
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
    #[must_use]
    pub const fn new(client: ClientName) -> Self {
        Self {
            client,
            resume: None,
            network: Network::Public,
        }
    }
}

/// What the app shows, its link status and host view, once `update`
/// arrives.
#[must_use]
pub fn shown_after(
    status: Option<Status>,
    view: HostView,
    update: Update,
) -> (Option<Status>, HostView) {
    if let Update::Status(next) = &update {
        tracing::info!(state = ?next.state, error = ?next.error, "link status");
    }
    client::link::shown_after(status, view, update)
}

/// Why a link to a ticket's host did not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("{0}")]
    Ticket(#[from] TicketError),
    #[error("could not start iroh: {0}")]
    Bind(#[from] BindError),
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
    #[error("no random source for a command id: {0}")]
    Random(#[from] getrandom::Error),
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
    /// The connection to run a command on, while the link is connected.
    Connection {
        reply: oneshot::Sender<Option<Connection>>,
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
    /// Starts a link to the host `ticket` names, and logs why when it
    /// can't.
    ///
    /// # Errors
    ///
    /// [`StartError::Ticket`] when the ticket names no host, and
    /// [`StartError::Bind`] when iroh can't start.
    pub async fn start(
        ticket: &Ticket,
        options: LinkOptions,
    ) -> Result<(Self, mpsc::Receiver<Update>), StartError> {
        let started = match Target::of_ticket(ticket) {
            Ok(target) => {
                Self::spawn(target, options).await.map_err(StartError::from)
            }
            Err(err) => Err(StartError::from(err)),
        };
        started.inspect_err(|err| {
            tracing::error!(%err, "the link did not start");
        })
    }

    /// Binds an endpoint on `options.network` and starts the link on it. A
    /// dial that stalls is retried on a fresh endpoint of the same network,
    /// and the link then runs on whichever connected and closes the other.
    ///
    /// # Errors
    /// Fails when no endpoint can be bound on `options.network`.
    pub async fn spawn(
        target: Target,
        options: LinkOptions,
    ) -> Result<(Self, mpsc::Receiver<Update>), BindError> {
        let endpoint = platform::bind_endpoint(options.network).await?;
        let (requests_tx, requests_rx) = mpsc::channel(16);
        let (updates_tx, updates_rx) = mpsc::channel(UPDATE_BUFFER);
        let (status_tx, status_rx) = watch::channel(Status::default());
        let hello = Hello {
            client: options.client,
            token: target.token,
        };
        let (core, first) = Core::start(hello, options.resume);
        let link = Link {
            endpoint,
            retiring: Vec::new(),
            network: options.network,
            addr: target.addr,
            conn: None,
            pending: idle(),
            requests: requests_rx,
            outbox: Outbox {
                status: status_tx,
                updates: updates_tx,
            },
        };
        task::spawn(link.run(core, first));
        let handle = Self {
            requests: requests_tx,
            status: status_rx,
        };
        Ok((handle, updates_rx))
    }

    #[must_use]
    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    /// Runs one command on its own stream. Fails fast while not connected;
    /// the caller decides whether to retry.
    /// A `Providers` answer is in the status by the time this returns.
    ///
    /// # Errors
    /// Fails when the link is not connected or has stopped, or the host
    /// closes the stream without a result or sends one that can't be read,
    /// or the platform has no random source for the command's id.
    pub async fn send(
        &self,
        body: CommandBody,
    ) -> Result<CommandResult, SendError> {
        let conn = self
            .ask(|reply| Request::Connection { reply })
            .await?
            .ok_or(SendError::NotConnected)?;
        let (reply, answer) = oneshot::channel();
        task::spawn(async move {
            let _ = reply.send(rpc(conn, body).await);
        });
        let result = answer.await.map_err(|_| SendError::Stopped)??;
        if let CommandResult::Providers(listed) = &result {
            let providers = listed.clone();
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
    ///
    /// # Errors
    /// Fails when the link has already stopped.
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

/// The link's task ends: asked to stop, or every [`HostLink`] was dropped.
#[derive(Default)]
struct Stopped {
    reply: Option<oneshot::Sender<()>>,
}

type Halves = (SendStream, RecvStream);

/// How the operation in flight ended.
enum Done {
    Input(Input),
    Dialed(Dialed),
    /// The feed's next frame is due on `Halves`.
    Read(Input, Halves),
}

enum Happening {
    Done(Done),
    Request(Option<Request>),
}

struct Link {
    endpoint: Endpoint,
    /// Endpoints a hedge left behind, still closing.
    retiring: Vec<Endpoint>,
    network: Network,
    addr: EndpointAddr,
    conn: Option<Connection>,
    /// The one dial, hello, subscription read, draw or sleep in flight. It
    /// lives across `select!`s, so a read in progress loses no bytes.
    pending: BoxFuture<Done>,
    requests: mpsc::Receiver<Request>,
    outbox: Outbox,
}

/// What the handles see: the status, and the updates that report it.
struct Outbox {
    status: watch::Sender<Status>,
    updates: mpsc::Sender<Update>,
}

impl Link {
    async fn run(mut self, core: Core, first: Vec<Effect>) {
        let Err(stopped) = self.drive(core, first).await;
        self.close().await;
        if let Some(reply) = stopped.reply {
            let _ = reply.send(());
        }
    }

    async fn drive(
        &mut self,
        core: Core,
        first: Vec<Effect>,
    ) -> Result<Infallible, Stopped> {
        self.perform_all(first).await?;
        let mut now = core;
        loop {
            let happening = self.next(now.takes_requests()).await;
            now = self.handle(now, happening).await?;
        }
    }

    async fn next(&mut self, takes_requests: bool) -> Happening {
        if takes_requests {
            tokio::select! {
                done = &mut self.pending => Happening::Done(done),
                request = self.requests.recv() => Happening::Request(request),
            }
        } else {
            Happening::Done((&mut self.pending).await)
        }
    }

    async fn handle(
        &mut self,
        core: Core,
        happening: Happening,
    ) -> Result<Core, Stopped> {
        match happening {
            Happening::Done(done) => {
                let input = self.absorb(done);
                self.step(core, input).await
            }
            Happening::Request(Some(request)) => {
                self.request(core, request).await
            }
            Happening::Request(None) => Err(Stopped::default()),
        }
    }

    async fn request(
        &mut self,
        core: Core,
        request: Request,
    ) -> Result<Core, Stopped> {
        match request {
            Request::Connection { reply } => {
                let _ = reply.send(self.conn.clone());
                Ok(core)
            }
            Request::Adopt { providers, reply } => self
                .step(core, Input::Providers(providers))
                .await
                .inspect(|_| {
                    let _ = reply.send(());
                }),
            Request::Reconnect => self.step(core, Input::Reconnect).await,
            Request::Stop { reply } => Err(Stopped { reply: Some(reply) }),
        }
    }

    async fn step(
        &mut self,
        core: Core,
        input: Input,
    ) -> Result<Core, Stopped> {
        let (next, effects) = core.step(input);
        self.perform_all(effects).await?;
        Ok(next)
    }

    async fn perform_all(
        &mut self,
        effects: Vec<Effect>,
    ) -> Result<(), Stopped> {
        for effect in effects {
            self.perform(effect).await?;
        }
        Ok(())
    }

    async fn perform(&mut self, effect: Effect) -> Result<(), Stopped> {
        match effect {
            Effect::Start(operation) => {
                self.pending = self.start(operation);
                Ok(())
            }
            Effect::Close => {
                self.hang_up();
                Ok(())
            }
            Effect::Show(status) => self.outbox.show(status).await,
            Effect::Forward(update) => self.outbox.emit(update).await,
        }
    }

    fn start(&self, operation: Operation) -> BoxFuture<Done> {
        match operation {
            Operation::Dial { hedge_after } => Box::pin(dial_hedged(
                self.network,
                self.endpoint.clone(),
                self.addr.clone(),
                hedge_after,
            )),
            Operation::Hello { hello, within } => {
                self.on_connection(|conn| Box::pin(greet(conn, hello, within)))
            }
            Operation::Subscribe(subscribe) => {
                self.on_connection(|conn| Box::pin(subscribed(conn, subscribe)))
            }
            Operation::Draw => now(Input::Drawn(jitter())),
            Operation::Sleep(delay) => Box::pin(elapse(delay)),
        }
    }

    fn on_connection(
        &self,
        start: impl FnOnce(Connection) -> BoxFuture<Done>,
    ) -> BoxFuture<Done> {
        self.conn.clone().map_or_else(
            || now(Input::Lost(Lost::Wire(String::from("not connected")))),
            start,
        )
    }

    /// Takes what the operation in flight left behind, and what it means.
    fn absorb(&mut self, done: Done) -> Input {
        self.pending = idle();
        match done {
            Done::Input(input) => input,
            Done::Dialed(dialed) => self.dialed(dialed),
            Done::Read(input, halves) => {
                self.pending = Box::pin(read(halves));
                input
            }
        }
    }

    fn dialed(&mut self, dialed: Dialed) -> Input {
        self.endpoint = dialed.endpoint;
        self.retiring.retain(|endpoint| !endpoint.is_closed());
        self.retiring.extend(dialed.retired);
        self.conn = dialed.conn.as_ref().ok().cloned();
        dialed.conn.map_or_else(
            |err| Input::Lost(Lost::Connect(err.to_string())),
            |_| Input::Connected,
        )
    }

    fn hang_up(&mut self) {
        if let Some(conn) = self.conn.take() {
            conn.close(0u32.into(), b"bye");
        }
    }

    /// Closes the connection and the endpoint the link runs on, and waits
    /// out the retired ones.
    async fn close(&mut self) {
        self.hang_up();
        self.endpoint.close().await;
        let _ = join_all(self.retiring.iter().map(Endpoint::closed)).await;
    }
}

impl Outbox {
    /// Shows `status` to every handle, and sends it as an update.
    async fn show(&self, status: Status) -> Result<(), Stopped> {
        self.status.send_replace(status.clone());
        self.emit(Update::Status(status)).await
    }

    async fn emit(&self, update: Update) -> Result<(), Stopped> {
        self.updates
            .send(update)
            .await
            .map_err(|_| Stopped::default())
    }
}

fn now(input: Input) -> BoxFuture<Done> {
    Box::pin(ready(Done::Input(input)))
}

fn idle() -> BoxFuture<Done> {
    Box::pin(pending())
}

fn jitter() -> u64 {
    getrandom::u64().unwrap_or_else(|err| {
        tracing::warn!(%err, "no random source; backing off without jitter");
        0
    })
}

async fn elapse(delay: Duration) -> Done {
    sleep(delay).await;
    Done::Input(Input::Elapsed)
}

fn broken(err: &WireError) -> Input {
    Input::Lost(Lost::Wire(err.to_string()))
}

async fn greet(conn: Connection, hello: Hello, within: Duration) -> Done {
    let answered = timeout(within, hello_reply(&conn, hello)).await;
    Done::Input(answered.map_or_else(
        |_| {
            tracing::warn!(
                peer = %conn.remote_id(),
                connection = conn.stable_id(),
                timeout_ms = within.as_millis(),
                "hello timed out"
            );
            Input::Lost(Lost::HelloTimeout)
        },
        |reply| reply.map_or_else(|err| broken(&err), Input::from),
    ))
}

async fn hello_reply(
    conn: &Connection,
    hello: Hello,
) -> Result<Option<HelloReply>, WireError> {
    let started = Instant::now();
    let peer = conn.remote_id();
    let connection = conn.stable_id();
    tracing::debug!(%peer, connection, "hello opening");
    let (mut send, mut recv) = conn.open_bi().await?;
    wire::write(&mut send, &Open::Hello(hello)).await?;
    let elapsed_ms = || started.elapsed().as_millis();
    tracing::debug!(%peer, connection, elapsed_ms = elapsed_ms(), "hello sent");
    let reply = wire::read::<HelloReply>(&mut recv).await?;
    tracing::debug!(
        %peer,
        connection,
        elapsed_ms = elapsed_ms(),
        "hello reply received"
    );
    Ok(reply)
}

/// Opens the subscription; its frames follow as [`Done::Read`].
async fn subscribed(conn: Connection, subscribe: Subscribe) -> Done {
    open_subscription(&conn, subscribe).await.map_or_else(
        |err| Done::Input(broken(&err)),
        |halves| Done::Read(Input::Subscribed, halves),
    )
}

async fn open_subscription(
    conn: &Connection,
    subscribe: Subscribe,
) -> Result<Halves, WireError> {
    let (mut send, recv) = conn.open_bi().await?;
    wire::write(&mut send, &Open::Subscribe(subscribe)).await?;
    Ok((send, recv))
}

/// Reads one frame and hands the halves back for the next. Holding both
/// keeps the subscription open.
async fn read((send, mut recv): Halves) -> Done {
    let frame = wire::read::<Event>(&mut recv).await;
    Done::Read(
        frame.map_or_else(|err| broken(&err), Input::from),
        (send, recv),
    )
}

async fn rpc(
    conn: Connection,
    body: CommandBody,
) -> Result<CommandResult, SendError> {
    let command = Command {
        id: random_command_id()?,
        body,
    };
    let (mut send, mut recv) = conn.open_bi().await.map_err(WireError::from)?;
    wire::write(&mut send, &Open::Command(command)).await?;
    send.finish().map_err(WireError::from)?;
    wire::read(&mut recv).await?.ok_or(SendError::NoResult)
}

fn random_command_id() -> Result<CommandId, getrandom::Error> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)?;
    Ok(CommandId(u128::from_le_bytes(bytes)))
}

/// A dial in flight and the endpoint it runs on.
struct Attempt {
    endpoint: Endpoint,
    connecting: BoxFuture<Result<Connection, ConnectError>>,
}

impl Attempt {
    fn start(endpoint: Endpoint, addr: EndpointAddr) -> Self {
        Self {
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

async fn dial(
    endpoint: Endpoint,
    addr: EndpointAddr,
) -> Result<Connection, ConnectError> {
    endpoint.connect(addr, proto::ALPN).await
}

/// Dials from `endpoint`, and from a fresh endpoint too if the first dial
/// hasn't returned after `hedge_after`.
async fn dial_hedged(
    network: Network,
    bound: Endpoint,
    addr: EndpointAddr,
    hedge_after: Duration,
) -> Done {
    let Attempt {
        endpoint,
        mut connecting,
    } = Attempt::start(bound, addr.clone());
    let dialed = tokio::select! {
        conn = &mut connecting => Dialed { endpoint, conn, retired: None },
        () = sleep(hedge_after) => {
            let first = Attempt { endpoint, connecting };
            hedge(network, first, addr).await
        }
    };
    Done::Dialed(dialed)
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
            let second = Attempt {
                endpoint: second_endpoint,
                connecting: second_connecting,
            };
            settle(first_endpoint, conn, second).await
        }
        conn = &mut second_connecting => {
            let first = Attempt {
                endpoint: first_endpoint,
                connecting: first_connecting,
            };
            settle(second_endpoint, conn, first).await
        }
    }
}

/// One attempt returned. If it connected, the other is dropped and its
/// endpoint closed; if it failed, its endpoint is closed and the other
/// decides.
async fn settle(
    endpoint: Endpoint,
    outcome: Result<Connection, ConnectError>,
    other: Attempt,
) -> Dialed {
    match outcome {
        Ok(conn) => {
            tracing::info!(
                endpoint = %endpoint.id().fmt_short(),
                "hedged dial connected"
            );
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
