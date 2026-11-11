//! Host and client in one process over real iroh connections on loopback, with
//! relays off so the test needs no network.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use client::host_view::{HostView, Item, ReplyState, SessionStatus, SessionView};
use client::{Cursor, HostLink, LinkOptions, LinkState, Network, PathKind, Status, Target, Update};
use daemon::Stopped;
use daemon::log::Log;
use daemon::registry::{Availability, Providers, Registry};
use daemon::serve::Conductor;
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, RelayMode};
use n0_future::StreamExt;
use proto::{
    AuthToken, ClientName, CommandBody, CommandResult, Epoch, Event, EventKind, HostPath, HostSeq,
    Provider, Rejection, SessionId,
};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

const TOKEN: [u8; 16] = [7; 16];
const STEP: Duration = Duration::from_secs(10);
const UNAVAILABLE: &str = "no agents in this test";

async fn endpoint() -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .unwrap()
}

struct Fixture {
    router: Router,
    target: Target,
    log: Log,
}

async fn host() -> Fixture {
    let log = Log::spawn(String::from("test-host"), Epoch(11));
    let unavailable = Availability::Unavailable(String::from(UNAVAILABLE));
    let registry = Registry::spawn(Providers {
        claude: unavailable.clone(),
        codex: unavailable,
    });
    let endpoint = endpoint().await;
    let target = Target {
        addr: endpoint.addr(),
        token: AuthToken::from_bytes(TOKEN),
    };
    let conductor = Conductor {
        log: log.clone(),
        registry,
        token: AuthToken::from_bytes(TOKEN),
        rtc: None,
    };
    let router = Router::builder(endpoint)
        .accept(proto::ALPN, conductor)
        .spawn();
    Fixture {
        router,
        target,
        log,
    }
}

async fn link(target: Target, resume: Option<Cursor>) -> (HostLink, mpsc::Receiver<Update>) {
    let options = LinkOptions {
        resume,
        network: Network::Local,
        ..LinkOptions::new(ClientName(String::from("test-client")))
    };
    HostLink::spawn(target, options).await.unwrap()
}

async fn next(updates: &mut mpsc::Receiver<Update>) -> Update {
    tokio::time::timeout(STEP, updates.recv())
        .await
        .expect("an update within the step timeout")
        .expect("the link is running")
}

/// The updates before the link went live.
async fn until_live(updates: &mut mpsc::Receiver<Update>) -> Vec<Update> {
    let mut seen = Vec::new();
    loop {
        match next(updates).await {
            Update::Status(Status {
                state: LinkState::Live,
                ..
            }) => break seen,
            update => seen.push(update),
        }
    }
}

fn states(updates: Vec<Update>) -> Vec<LinkState> {
    updates
        .into_iter()
        .filter_map(|update| match update {
            Update::Status(status) => Some(status.state),
            Update::Reset(_) | Update::Event(_) => None,
        })
        .collect()
}

/// The events up to and including `last`.
async fn events_through(updates: &mut mpsc::Receiver<Update>, last: HostSeq) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        match next(updates).await {
            Update::Event(event) => match event.seq.cmp(&last) {
                Ordering::Less => events.push(event),
                Ordering::Equal | Ordering::Greater => {
                    events.push(event);
                    break events;
                }
            },
            Update::Status(_) | Update::Reset(_) => {}
        }
    }
}

/// Appends one Claude session's creation, or its deltas, to the log.
async fn append(log: &Log, kinds: Vec<EventKind>) {
    let appended = n0_future::stream::iter(kinds)
        .then(|kind| log.append(SessionId(1), kind))
        .collect::<Vec<Result<(), Stopped>>>()
        .await;
    assert_eq!(appended.into_iter().filter(Result::is_err).count(), 0);
}

fn created() -> EventKind {
    EventKind::SessionCreated {
        provider: Provider::Claude,
        directory: HostPath(String::from("/work")),
        by: ClientName(String::from("desk")),
    }
}

fn delta(text: &str) -> EventKind {
    EventKind::AssistantDelta {
        text: String::from(text),
    }
}

fn seqs(events: &[Event]) -> Vec<HostSeq> {
    events.iter().map(|event| event.seq).collect()
}

fn fold(events: Vec<Event>) -> HostView {
    events.into_iter().fold(HostView::empty(), HostView::apply)
}

/// The view of session 1 after `created()` and deltas that join into `text`.
fn streaming(text: &str) -> HostView {
    HostView {
        sessions: BTreeMap::from([(
            SessionId(1),
            SessionView {
                provider: Provider::Claude,
                directory: HostPath(String::from("/work")),
                status: SessionStatus::Starting,
                items: Vec::from([Item::Reply {
                    text: String::from(text),
                    state: ReplyState::Streaming,
                }]),
            },
        )]),
    }
}

/// One command and the host's literal answer.
struct Case {
    body: CommandBody,
    result: CommandResult,
}

fn start(directory: &std::path::Path) -> CommandBody {
    CommandBody::StartSession {
        provider: Provider::Claude,
        directory: HostPath(directory.display().to_string()),
    }
}

#[tokio::test]
async fn a_client_goes_live_and_start_session_is_checked_at_the_boundary() {
    let fx = host().await;
    let (link, mut updates) = link(fx.target.clone(), None).await;
    assert_eq!(
        states(until_live(&mut updates).await),
        [
            LinkState::Connecting,
            LinkState::Authenticating,
            LinkState::CatchingUp
        ]
    );
    assert_eq!(link.status().host, Some(String::from("test-host")));
    assert_eq!(link.status().path, PathKind::Direct);

    let cases = Vec::from([
        Case {
            body: start(&std::env::temp_dir().join("conductor-e2e-missing")),
            result: CommandResult::Rejected(Rejection::NoSuchDirectory),
        },
        Case {
            body: start(&std::env::current_exe().unwrap()),
            result: CommandResult::Rejected(Rejection::NoSuchDirectory),
        },
        Case {
            body: start(&std::env::temp_dir()),
            result: CommandResult::Rejected(Rejection::ProviderUnavailable {
                reason: String::from(UNAVAILABLE),
            }),
        },
        Case {
            body: CommandBody::SendMessage {
                session: SessionId(1),
                text: String::from("hi"),
            },
            result: CommandResult::Rejected(Rejection::UnknownSession),
        },
    ]);
    let results = n0_future::join_all(cases.iter().map(|case| link.send(case.body.clone()))).await;
    assert_eq!(
        results
            .into_iter()
            .map(Result::unwrap)
            .collect::<Vec<CommandResult>>(),
        cases
            .into_iter()
            .map(|case| case.result)
            .collect::<Vec<CommandResult>>()
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn reconnect_mid_feed_resumes_after_the_cursor_without_gaps_or_duplicates() {
    let fx = host().await;
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    append(&fx.log, Vec::from([created(), delta("Hel"), delta("lo")])).await;
    let before = events_through(&mut updates, HostSeq(3)).await;

    append(&fx.log, Vec::from([delta(" wor"), delta("ld")])).await;
    link.reconnect().await;
    append(&fx.log, Vec::from([delta("!")])).await;
    let after = events_through(&mut updates, HostSeq(6)).await;
    let seen = before.into_iter().chain(after).collect::<Vec<Event>>();
    assert_eq!(seqs(&seen), [1, 2, 3, 4, 5, 6].map(HostSeq));
    assert_eq!(fold(seen), streaming("Hello world!"));
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_new_client_resumes_from_a_dead_clients_cursor() {
    let fx = host().await;
    let (first, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    append(&fx.log, Vec::from([created(), delta("Hel")])).await;
    let seen = events_through(&mut updates, HostSeq(2)).await;
    first.stop().await.unwrap();
    drop(updates);

    append(&fx.log, Vec::from([delta("lo")])).await;
    let cursor = Cursor {
        epoch: Epoch(11),
        seq: HostSeq(2),
    };
    let (second, mut updates) = link(fx.target.clone(), Some(cursor)).await;
    let rest = events_through(&mut updates, HostSeq(3)).await;
    assert_eq!(seqs(&rest), [HostSeq(3)]);
    assert_eq!(
        fold(seen.into_iter().chain(rest).collect()),
        streaming("Hello")
    );
    second.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_cursor_from_another_epoch_starts_over_from_the_beginning() {
    let fx = host().await;
    append(&fx.log, Vec::from([created(), delta("Hi")])).await;
    let stale = Cursor {
        epoch: Epoch(10),
        seq: HostSeq(40),
    };
    let (link, mut updates) = link(fx.target.clone(), Some(stale)).await;
    let before_live = until_live(&mut updates).await;
    assert_eq!(
        before_live
            .iter()
            .filter_map(|update| match update {
                Update::Reset(epoch) => Some(epoch),
                Update::Status(_) | Update::Event(_) => None,
            })
            .collect::<Vec<&Epoch>>(),
        [&Epoch(11)]
    );
    let replayed = before_live
        .into_iter()
        .filter_map(|update| match update {
            Update::Event(event) => Some(event),
            Update::Status(_) | Update::Reset(_) => None,
        })
        .collect::<Vec<Event>>();
    assert_eq!(fold(replayed), streaming("Hi"));
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// The states before the link reported an error, and the error.
async fn until_error(updates: &mut mpsc::Receiver<Update>) -> (Vec<LinkState>, String) {
    let mut seen = Vec::new();
    loop {
        match next(updates).await {
            Update::Status(Status {
                error: Some(error), ..
            }) => break (seen, error),
            Update::Status(status) => seen.push(status.state),
            Update::Reset(_) | Update::Event(_) => {}
        }
    }
}

#[tokio::test]
async fn a_wrong_token_is_refused_and_never_goes_live() {
    let fx = host().await;
    let target = Target {
        token: AuthToken::from_bytes([0; 16]),
        ..fx.target.clone()
    };
    let (link, mut updates) = link(target, None).await;
    assert_eq!(
        until_error(&mut updates).await,
        (
            Vec::from([LinkState::Connecting, LinkState::Authenticating]),
            String::from("the host refused us: wrong token")
        )
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// What a reload is allowed: the 1.5 s hedge delay plus a loopback connect.
const HEDGED_LIVE: Duration = Duration::from_secs(3);

/// What iroh logs when an endpoint is dropped before anyone closed it.
const UNGRACEFUL: &str = "Endpoint dropped without calling `Endpoint::close`";

/// The largest datagram the hop carries.
const DATAGRAM: usize = 65536;

/// A UDP hop in front of the host that drops everything from the first
/// client address it hears and forwards for the next, like a path that went
/// dead for one endpoint only.
struct Hop {
    /// Where clients send.
    outer: UdpSocket,
    /// Connected to the host.
    inner: UdpSocket,
    dead: Option<SocketAddr>,
    live: Option<SocketAddr>,
}

enum Datagram {
    FromClient { bytes: Vec<u8>, from: SocketAddr },
    FromHost(Vec<u8>),
}

impl Hop {
    async fn spawn(host: SocketAddr) -> SocketAddr {
        let outer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let inner = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        inner.connect(host).await.unwrap();
        let addr = outer.local_addr().unwrap();
        let hop = Hop {
            outer,
            inner,
            dead: None,
            live: None,
        };
        tokio::spawn(hop.run());
        addr
    }

    async fn run(mut self) -> io::Result<Infallible> {
        loop {
            let datagram = self.next_input().await?;
            self.on(datagram).await?;
        }
    }

    async fn next_input(&self) -> io::Result<Datagram> {
        tokio::select! {
            datagram = from_client(&self.outer) => datagram,
            datagram = from_host(&self.inner) => datagram,
        }
    }

    async fn on(&mut self, datagram: Datagram) -> io::Result<()> {
        match (datagram, self.dead, self.live) {
            (Datagram::FromClient { from, .. }, None, _) => {
                self.dead = Some(from);
                Ok(())
            }
            (Datagram::FromClient { bytes, from }, Some(dead), _) => {
                match Some(from).filter(|from| from != &dead) {
                    Some(live) => {
                        self.live = Some(live);
                        let _ = self.inner.send(&bytes).await?;
                        Ok(())
                    }
                    None => Ok(()),
                }
            }
            (Datagram::FromHost(bytes), _, Some(live)) => {
                let _ = self.outer.send_to(&bytes, live).await?;
                Ok(())
            }
            (Datagram::FromHost(_), _, None) => Ok(()),
        }
    }
}

async fn from_client(socket: &UdpSocket) -> io::Result<Datagram> {
    let mut bytes = Vec::with_capacity(DATAGRAM);
    let (_, from) = socket.recv_buf_from(&mut bytes).await?;
    Ok(Datagram::FromClient { bytes, from })
}

async fn from_host(socket: &UdpSocket) -> io::Result<Datagram> {
    let mut bytes = Vec::with_capacity(DATAGRAM);
    let _ = socket.recv_buf(&mut bytes).await?;
    Ok(Datagram::FromHost(bytes))
}

/// Where `endpoint` listens on IPv4 loopback.
fn loopback(endpoint: &Endpoint) -> SocketAddr {
    let bound = endpoint
        .bound_sockets()
        .into_iter()
        .find(|addr| matches!(addr, SocketAddr::V4(_)))
        .unwrap();
    SocketAddr::from((Ipv4Addr::LOCALHOST, bound.port()))
}

#[tokio::test]
async fn a_stalled_dial_goes_live_through_a_fresh_endpoint_and_stops_cleanly() {
    let log_path = std::env::temp_dir().join(format!("conductor-e2e-{}.log", std::process::id()));
    let log = std::fs::File::create(&log_path).unwrap();
    let _logging = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_writer(Arc::new(log))
            .with_ansi(false)
            .finish(),
    );
    let fx = host().await;
    let hop = Hop::spawn(loopback(fx.router.endpoint())).await;
    let target = Target {
        addr: EndpointAddr::new(fx.target.addr.id).with_ip_addr(hop),
        ..fx.target.clone()
    };
    let (link, mut updates) = link(target, None).await;

    let before_live = tokio::time::timeout(HEDGED_LIVE, until_live(&mut updates))
        .await
        .expect("live within the hedge delay plus a connect");
    assert_eq!(
        states(before_live),
        [
            LinkState::Connecting,
            LinkState::Authenticating,
            LinkState::CatchingUp
        ]
    );
    link.stop().await.unwrap();
    drop(updates);
    fx.router.shutdown().await.unwrap();

    let logged = std::fs::read_to_string(&log_path).unwrap();
    std::fs::remove_file(&log_path).unwrap();
    assert_eq!(
        (
            logged.matches("hedged dial connected").count(),
            logged.matches(UNGRACEFUL).count()
        ),
        (1, 0),
        "{logged}"
    );
}
