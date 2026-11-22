//! [`HostLink`]: one client's connection to one host (architecture §9).
//!
//! The link's decisions are the state machine in [`client::link`]. A single
//! task owns the endpoints, the connection and its streams: it turns what
//! they do into an [`Input`], asks the core, and performs each [`Effect`].

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::future::{pending, ready};
use std::time::Duration;

use crate::wire::{self, WireError};
use iroh::endpoint::{BindError, Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr};
use n0_future::boxed::BoxFuture;
use n0_future::time::{Instant, sleep, timeout};
use n0_future::{FuturesUnordered, StreamExt, join_all, task};

use proto::fleet::Certificate;

use client::link::{
    Attempt, Cursor, Effect, Input, Link as Core, Lost, Operation, Unreached,
};
pub use client::link::{LinkState, Status, Update};
use proto::{
    AuthToken, ClientName, Command, CommandBody, CommandId, CommandResult,
    Feed, Hello, HelloReply, Open, Subscribe, Ticket, TicketError,
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

/// Why a link to a ticket's host did not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("{0}")]
    Ticket(#[from] TicketError),
    #[error("could not start iroh: {0}")]
    Bind(#[from] BindError),
    #[error("{0}")]
    Fleet(crate::fleet::IdentityError),
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
    shutdown: oneshot::Sender<oneshot::Sender<()>>,
}

#[derive(Debug)]
enum Request {
    /// The connection to run a command on, while the link is connected.
    Connection {
        reply: oneshot::Sender<Option<Connection>>,
    },
    Reconnect,
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
        Self::start_on(ticket, options).await.inspect_err(|err| {
            tracing::error!(%err, "the link did not start");
        })
    }

    async fn start_on(
        ticket: &Ticket,
        options: LinkOptions,
    ) -> Result<(Self, mpsc::Receiver<Update>), StartError> {
        let target = Target::of_ticket(ticket)?;
        Ok(Self::spawn(target, options).await?)
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
        Self::spawn_with(target.addr, target.token, None, options).await
    }

    /// Connects using this browser's fleet identity.
    ///
    /// # Errors
    ///
    /// If the host address or local endpoint cannot be opened.
    pub async fn spawn_fleet(
        host: &proto::fleet::HostEntry,
        certificate: Certificate,
        options: LinkOptions,
    ) -> Result<(Self, mpsc::Receiver<Update>), StartError> {
        let address = crate::fleet::address(host).map_err(StartError::Fleet)?;
        Ok(Self::spawn_with(
            address,
            AuthToken::from_bytes([0; 16]),
            Some(certificate),
            options,
        )
        .await?)
    }

    async fn spawn_with(
        address: EndpointAddr,
        token: AuthToken,
        fleet: Option<Certificate>,
        options: LinkOptions,
    ) -> Result<(Self, mpsc::Receiver<Update>), BindError> {
        let endpoint = platform::bind_endpoint(options.network).await?;
        let (requests_tx, requests_rx) = mpsc::channel(16);
        let (updates_tx, updates_rx) = mpsc::channel(UPDATE_BUFFER);
        let (status_tx, status_rx) = watch::channel(Status::default());
        let hello = Hello {
            client: options.client,
            token,
        };
        let (core, first) = Core::start(hello, options.resume);
        let link = Link {
            endpoint,
            retiring: Vec::new(),
            network: options.network,
            addr: address,
            fleet,
            conn: None,
            attempts: BTreeMap::new(),
            reached: BTreeMap::new(),
            dials: FuturesUnordered::new(),
            pending: idle(),
            requests: requests_rx,
            outbox: Outbox {
                status: status_tx,
                updates: updates_tx,
            },
        };
        let (shutdown, cancelled) = oneshot::channel();
        task::spawn(link.run(core, first, cancelled));
        let handle = Self {
            requests: requests_tx,
            status: status_rx,
            shutdown,
        };
        Ok((handle, updates_rx))
    }

    #[must_use]
    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    /// Runs one command on its own stream. Fails fast while not connected;
    /// the caller decides whether to retry.
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
            let result = rpc(&conn, body).await;
            let _ = reply.send(result);
        });
        answer.await.map_err(|_| SendError::Stopped)?
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
        let (reply, finished) = oneshot::channel();
        self.shutdown.send(reply).map_err(|_| SendError::Stopped)?;
        finished.await.map_err(|_| SendError::Stopped)
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

struct Stopped;

type Halves = (SendStream, RecvStream);

/// How the operation in flight ended.
enum Done {
    Input(Input),
    /// The feed's next frame is due on `Halves`.
    Read(Input, Halves),
    Subscribed(Feed, Halves),
}

enum Happening {
    Done(Box<Done>),
    /// A dial ended; the operation in flight runs on.
    Dialed(Box<Dialed>),
    Request(Option<Request>),
}

struct Link {
    endpoint: Endpoint,
    /// Endpoints a hedge left behind, still closing.
    retiring: Vec<Endpoint>,
    network: Network,
    addr: EndpointAddr,
    fleet: Option<Certificate>,
    conn: Option<Connection>,
    /// Each dial's endpoint, until the core adopts or retires the dial.
    attempts: BTreeMap<Attempt, Endpoint>,
    /// Each dial's connection, until the core adopts or retires the dial.
    reached: BTreeMap<Attempt, Connection>,
    dials: FuturesUnordered<BoxFuture<Dialed>>,
    /// The one hello, subscription read, draw or sleep in flight. It lives
    /// across `select!`s, so a read in progress loses no bytes.
    pending: BoxFuture<Done>,
    requests: mpsc::Receiver<Request>,
    outbox: Outbox,
}

/// What the handles see: the status, and the updates that report it.
struct Outbox {
    status: watch::Sender<Status>,
    updates: mpsc::Sender<Update>,
}

async fn until_shutdown(
    drive: impl Future<Output = Result<Infallible, Stopped>>,
    cancelled: oneshot::Receiver<oneshot::Sender<()>>,
) -> Option<oneshot::Sender<()>> {
    tokio::select! {
        biased;
        reply = cancelled => reply.ok(),
        _ = drive => None,
    }
}

impl Link {
    async fn run(
        mut self,
        core: Core,
        first: Vec<Effect>,
        cancelled: oneshot::Receiver<oneshot::Sender<()>>,
    ) {
        let reply = until_shutdown(self.drive(core, first), cancelled).await;
        self.close().await;
        if let Some(finished) = reply {
            let _ = finished.send(());
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
        tokio::select! {
            done = &mut self.pending => Happening::Done(Box::new(done)),
            Some(dialed) = self.dials.next() => Happening::Dialed(Box::new(dialed)),
            request = self.requests.recv(), if takes_requests => Happening::Request(request),
        }
    }

    async fn handle(
        &mut self,
        core: Core,
        happening: Happening,
    ) -> Result<Core, Stopped> {
        match happening {
            Happening::Done(done) => {
                let input = self.absorb(*done);
                self.step(core, input).await
            }
            Happening::Dialed(dialed) => {
                let input = self.dialed(*dialed);
                self.step(core, input).await
            }
            Happening::Request(Some(request)) => {
                self.request(core, request).await
            }
            Happening::Request(None) => Err(Stopped),
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
            Request::Reconnect => self.step(core, Input::Reconnect).await,
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
            Effect::Dial(attempt) => {
                let endpoint = self.endpoint.clone();
                self.attempts.insert(attempt, endpoint.clone());
                self.dials.push(Box::pin(dial(
                    attempt,
                    endpoint,
                    self.addr.clone(),
                    "dial",
                )));
                Ok(())
            }
            Effect::Hedge(attempt) => {
                tracing::info!(
                    "dial stalled; dialing again on a fresh endpoint"
                );
                self.dials.push(Box::pin(hedge(
                    attempt,
                    self.network,
                    self.addr.clone(),
                )));
                Ok(())
            }
            Effect::Adopt(attempt) => {
                self.adopt(attempt);
                Ok(())
            }
            Effect::Retire(attempt) => {
                self.retire(attempt);
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
            Operation::Hello { hello, within } => {
                let fleet = self.fleet.clone();
                let local = self.endpoint.id();
                self.on_connection(|conn| {
                    Box::pin(greet(conn, hello, within, fleet, local))
                })
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
            Done::Read(input, halves) => {
                self.pending = Box::pin(read(halves));
                input
            }
            Done::Subscribed(frame, halves) => {
                self.pending =
                    Box::pin(ready(Done::Read(Input::Frame(frame), halves)));
                Input::Subscribed
            }
        }
    }

    /// Keeps what the dial left until the core adopts or retires it.
    fn dialed(&mut self, dialed: Dialed) -> Input {
        let Dialed {
            attempt,
            endpoint,
            conn,
        } = dialed;
        self.attempts.extend(endpoint.map(|bound| (attempt, bound)));
        let outcome = conn.map(|connected| {
            self.reached.insert(attempt, connected);
        });
        Input::Dialed { attempt, outcome }
    }

    fn adopt(&mut self, attempt: Attempt) {
        if let Some(endpoint) = self.attempts.remove(&attempt) {
            self.endpoint = endpoint;
        }
        self.conn = self.reached.remove(&attempt);
    }

    /// Drops the dial's connection and starts closing its endpoint.
    fn retire(&mut self, attempt: Attempt) {
        drop(self.reached.remove(&attempt));
        self.retiring.retain(|endpoint| !endpoint.is_closed());
        self.retiring
            .extend(self.attempts.remove(&attempt).map(retire));
    }

    fn hang_up(&mut self) {
        if let Some(conn) = self.conn.take() {
            conn.close(0u32.into(), b"bye");
        }
    }

    /// Closes the connection and the endpoint the link runs on, and waits
    /// out the retired ones and those of dials still running.
    async fn close(&mut self) {
        self.hang_up();
        self.pending = idle();
        self.dials = FuturesUnordered::new();
        self.endpoint.close().await;
        let dialing = std::mem::take(&mut self.attempts);
        self.retiring.extend(dialing.into_values().map(retire));
        let _ = join_all(self.retiring.iter().map(Endpoint::closed)).await;
    }
}

impl Outbox {
    /// Shows `status` to every handle, and sends it as an update.
    async fn show(&self, status: Status) -> Result<(), Stopped> {
        tracing::info!(state = ?status.state, error = ?status.error, "link status");
        self.status.send_replace(status.clone());
        self.emit(Update::Status(status)).await
    }

    async fn emit(&self, update: Update) -> Result<(), Stopped> {
        self.updates.send(update).await.map_err(|_| Stopped)
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

async fn greet(
    conn: Connection,
    hello: Hello,
    within: Duration,
    fleet: Option<Certificate>,
    local: iroh::EndpointId,
) -> Done {
    let opening = match fleet {
        Some(certificate) => {
            match crate::fleet::hello(certificate, conn.remote_id(), local)
                .await
            {
                Ok(hello) => Open::FleetHello(hello),
                Err(error) => {
                    return Done::Input(Input::Lost(Lost::Wire(
                        error.to_string(),
                    )));
                }
            }
        }
        None => Open::Hello(hello),
    };
    let answered = timeout(within, hello_reply(&conn, &opening)).await;
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
    opening: &Open,
) -> Result<Option<HelloReply>, WireError> {
    let started = Instant::now();
    let peer = conn.remote_id();
    let connection = conn.stable_id();
    tracing::debug!(%peer, connection, "hello opening");
    let (mut send, mut recv) = conn.open_bi().await?;
    wire::write(&mut send, opening).await?;
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

/// The host always sends an initial frame after accepting a subscription.
async fn subscribed(conn: Connection, subscribe: Subscribe) -> Done {
    match open_subscription(&conn, subscribe).await {
        Ok((Some(frame), halves)) => Done::Subscribed(frame, halves),
        Ok((None, _)) => Done::Input(Input::from(None::<Feed>)),
        Err(err) => Done::Input(broken(&err)),
    }
}

async fn open_subscription(
    conn: &Connection,
    subscribe: Subscribe,
) -> Result<(Option<Feed>, Halves), WireError> {
    let (mut send, mut recv) = conn.open_bi().await?;
    wire::write(&mut send, &Open::Subscribe(subscribe)).await?;
    let first = wire::read(&mut recv).await?;
    Ok((first, (send, recv)))
}

/// Reads one frame and hands the halves back for the next. Holding both
/// keeps the subscription open.
async fn read((send, mut recv): Halves) -> Done {
    let frame = wire::read::<Feed>(&mut recv).await;
    Done::Read(
        frame.map_or_else(|err| broken(&err), Input::from),
        (send, recv),
    )
}

async fn rpc(
    conn: &Connection,
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

/// How a dial ended, and the endpoint it ran on, if one was bound.
struct Dialed {
    attempt: Attempt,
    endpoint: Option<Endpoint>,
    conn: Result<Connection, Unreached>,
}

/// Dials from `endpoint`; `kind` names the dial in the log.
async fn dial(
    attempt: Attempt,
    endpoint: Endpoint,
    addr: EndpointAddr,
    kind: &str,
) -> Dialed {
    let conn = endpoint
        .connect(addr, proto::ALPN)
        .await
        .map_err(|err| Unreached::Connect(err.to_string()))
        .inspect(|_| {
            let id = endpoint.id().fmt_short();
            tracing::info!(?attempt, endpoint = %id, "{kind} connected");
        })
        .inspect_err(|err| tracing::debug!(?attempt, ?err, "{kind} failed"));
    Dialed {
        attempt,
        endpoint: Some(endpoint),
        conn,
    }
}

/// Dials from a fresh endpoint of `network`.
async fn hedge(
    attempt: Attempt,
    network: Network,
    addr: EndpointAddr,
) -> Dialed {
    match platform::bind_endpoint(network).await {
        Ok(fresh) => dial(attempt, fresh, addr, "hedged dial").await,
        Err(err) => {
            tracing::warn!(%err, "no fresh endpoint to hedge with");
            Dialed {
                attempt,
                endpoint: None,
                conn: Err(Unreached::Unbound(err.to_string())),
            }
        }
    }
}

/// Starts closing an endpoint the link no longer uses, without waiting for
/// it, and hands it back so a stop can wait for the close.
fn retire(endpoint: Endpoint) -> Endpoint {
    let closing = endpoint.clone();
    task::spawn(async move { closing.close().await });
    endpoint
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;
    use std::pin::pin;
    use std::task::Poll;

    use super::{Outbox, Status, Update, until_shutdown};
    use tokio::sync::{mpsc, oneshot, watch};

    #[tokio::test]
    async fn shutdown_preempts_delivery_to_a_full_updates_buffer()
    -> anyhow::Result<()> {
        let (updates, _unread) = mpsc::channel(1);
        let (status, _observed) = watch::channel(Status::default());
        let outbox = Outbox { status, updates };
        outbox
            .emit(Update::Status(Status::default()))
            .await
            .map_err(|_| anyhow::anyhow!("updates channel stopped"))?;
        let (shutdown, cancelled) = oneshot::channel();
        let mut stopped = pin!(until_shutdown(
            async {
                outbox.emit(Update::Status(Status::default())).await?;
                std::future::pending().await
            },
            cancelled
        ));
        poll_fn(|cx| {
            assert!(stopped.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        let (ack, _acknowledged) = oneshot::channel();
        shutdown
            .send(ack)
            .map_err(|_| anyhow::anyhow!("shutdown channel stopped"))?;
        assert!(stopped.await.is_some());
        Ok(())
    }
}
