//! Host and client in one process over real iroh connections on loopback, with
//! relays off so the test needs no network.

use std::convert::Infallible;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use client::{Cursor, HostLink, LinkOptions, LinkState, Network, PathKind, Target, Update};
use daemon::host::{Host, HostOptions};
use daemon::server::Conductor;
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, RelayMode};
use proto::{
    AuthToken, CommandBody, CommandResult, Decision, Epoch, Event, EventKind, HostSeq, RequestId,
};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

const TOKEN: [u8; 16] = [7; 16];
const STEP: Duration = Duration::from_secs(10);

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
}

async fn host() -> Fixture {
    let host = Host::spawn(HostOptions {
        name: "test-host".into(),
        epoch: Epoch(11),
        delta_interval: Duration::from_millis(20),
    });
    let endpoint = endpoint().await;
    let target = Target {
        addr: endpoint.addr(),
        token: AuthToken::from_bytes(TOKEN),
    };
    let router = Router::builder(endpoint)
        .accept(
            proto::ALPN,
            Conductor {
                host,
                token: AuthToken::from_bytes(TOKEN),
                rtc: None,
            },
        )
        .spawn();
    Fixture { router, target }
}

async fn link(target: Target, resume: Option<Cursor>) -> (HostLink, mpsc::Receiver<Update>) {
    let options = LinkOptions {
        resume,
        network: Network::Local,
        ..LinkOptions::new(String::from("test-client"))
    };
    HostLink::spawn(target, options).await.unwrap()
}

async fn next(updates: &mut mpsc::Receiver<Update>) -> Update {
    tokio::time::timeout(STEP, updates.recv())
        .await
        .expect("an update within the step timeout")
        .expect("the link is running")
}

async fn until_live(updates: &mut mpsc::Receiver<Update>) -> Vec<Update> {
    let mut seen = Vec::new();
    loop {
        let update = next(updates).await;
        let live = matches!(&update, Update::Status(s) if s.state == LinkState::Live);
        seen.push(update);
        if live {
            return seen;
        }
    }
}

/// Reads events until `stop` says so, answering approvals with `decision`.
async fn events_until(
    link: &HostLink,
    updates: &mut mpsc::Receiver<Update>,
    mut stop: impl FnMut(&Event) -> bool,
) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        if let Update::Event(event) = next(updates).await {
            if let EventKind::ApprovalRequested { id, .. } = event.kind {
                approve(link, id).await;
            }
            let done = stop(&event);
            events.push(event);
            if done {
                return events;
            }
        }
    }
}

async fn approve(link: &HostLink, id: RequestId) {
    let result = link
        .send(CommandBody::Resolve {
            approval: id,
            decision: Decision::Approve,
        })
        .await
        .unwrap();
    assert_eq!(result, CommandResult::Accepted);
}

async fn prompt(link: &HostLink) {
    let result = link
        .send(CommandBody::Prompt { text: "hi".into() })
        .await
        .unwrap();
    assert_eq!(result, CommandResult::Accepted);
}

fn seqs(events: &[Event]) -> Vec<u64> {
    events.iter().map(|e| e.seq.0).collect()
}

fn transcript(events: &[Event]) -> String {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::TranscriptDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn finished(event: &Event) -> bool {
    event.kind == EventKind::TurnFinished
}

const REPLY: &str = "Looking into \"hi\". I need to run one command to answer.";

#[tokio::test]
async fn a_turn_streams_asks_for_approval_and_finishes() {
    let fx = host().await;
    let (link, mut updates) = link(fx.target.clone(), None).await;

    let states: Vec<_> = until_live(&mut updates)
        .await
        .into_iter()
        .filter_map(|u| match u {
            Update::Status(s) => Some(s.state),
            Update::Reset(epoch) => {
                assert_eq!(epoch, Epoch(11));
                None
            }
            Update::Event(e) => panic!("no history expected, got {e:?}"),
        })
        .collect();
    assert_eq!(
        states,
        [
            LinkState::Connecting,
            LinkState::Authenticating,
            LinkState::CatchingUp,
            LinkState::Live
        ]
    );
    assert_eq!(link.status().host.as_deref(), Some("test-host"));

    prompt(&link).await;
    let events = events_until(&link, &mut updates, finished).await;
    assert_eq!(transcript(&events), REPLY);
    let tail: Vec<_> = events[events.len() - 4..]
        .iter()
        .map(|e| e.kind.clone())
        .collect();
    assert_eq!(
        tail,
        [
            EventKind::ApprovalRequested {
                id: RequestId(1),
                tool: "Bash".into(),
                detail: "echo \"hi\"".into()
            },
            EventKind::ApprovalResolved {
                id: RequestId(1),
                decision: Decision::Approve,
                by: "test-client".into()
            },
            EventKind::AssistantMessage {
                text: "Ran it. The answer to \"hi\" is 42.".into()
            },
            EventKind::TurnFinished,
        ]
    );
    assert_eq!(seqs(&events), (1..=events.len() as u64).collect::<Vec<_>>());
    assert_eq!(link.status().path, PathKind::Direct);
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn reconnect_mid_turn_resumes_after_the_cursor_without_gaps_or_duplicates() {
    let fx = host().await;
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    prompt(&link).await;
    let mut before = events_until(&link, &mut updates, |e| e.seq == HostSeq(3)).await;

    link.reconnect().await;
    let mut states = Vec::new();
    let mut after = Vec::new();
    loop {
        match next(&mut updates).await {
            Update::Status(s) => states.push(s.state),
            Update::Reset(epoch) => panic!("same epoch, no reset expected: {epoch:?}"),
            Update::Event(event) => {
                if let EventKind::ApprovalRequested { id, .. } = event.kind {
                    approve(&link, id).await;
                }
                let done = finished(&event);
                after.push(event);
                if done {
                    break;
                }
            }
        }
    }
    assert_eq!(
        states[..3],
        [
            LinkState::Disconnected,
            LinkState::Connecting,
            LinkState::Authenticating
        ]
    );
    assert!(states.contains(&LinkState::Live), "{states:?}");
    assert_eq!(after[0].seq, HostSeq(4));
    before.append(&mut after);
    assert_eq!(seqs(&before), (1..=before.len() as u64).collect::<Vec<_>>());
    assert_eq!(transcript(&before), REPLY);
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_new_client_resumes_from_a_dead_clients_cursor() {
    let fx = host().await;
    let (first, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    prompt(&first).await;
    let mut seen = events_until(&first, &mut updates, |e| e.seq == HostSeq(5)).await;
    first.stop().await.unwrap();
    drop(updates);

    let cursor = Cursor {
        epoch: Epoch(11),
        seq: HostSeq(5),
    };
    let (second, mut updates) = link(fx.target.clone(), Some(cursor)).await;
    let mut rest = events_until(&second, &mut updates, finished).await;
    assert_eq!(rest[0].seq, HostSeq(6));
    seen.append(&mut rest);
    assert_eq!(seqs(&seen), (1..=seen.len() as u64).collect::<Vec<_>>());
    assert_eq!(transcript(&seen), REPLY);
    second.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_cursor_from_another_epoch_starts_over_from_the_beginning() {
    let fx = host().await;
    let (first, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    prompt(&first).await;
    events_until(&first, &mut updates, |e| e.seq == HostSeq(2)).await;

    let stale = Cursor {
        epoch: Epoch(10),
        seq: HostSeq(40),
    };
    let (second, mut updates) = link(fx.target.clone(), Some(stale)).await;
    let reset = until_live(&mut updates)
        .await
        .into_iter()
        .find_map(|u| match u {
            Update::Reset(epoch) => Some(epoch),
            _ => None,
        });
    assert_eq!(reset, Some(Epoch(11)));
    first.stop().await.unwrap();
    second.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_wrong_token_is_refused_and_never_goes_live() {
    let fx = host().await;
    let target = Target {
        token: AuthToken::from_bytes([0; 16]),
        ..fx.target.clone()
    };
    let (link, mut updates) = link(target, None).await;
    let error = loop {
        if let Update::Status(s) = next(&mut updates).await {
            assert_ne!(s.state, LinkState::Live);
            if let Some(error) = s.error {
                break error;
            }
        }
    };
    assert_eq!(error, "the host refused us: wrong token");
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

    let states = tokio::time::timeout(HEDGED_LIVE, until_live(&mut updates))
        .await
        .expect("live within the hedge delay plus a connect")
        .into_iter()
        .filter_map(|u| match u {
            Update::Status(s) => Some(s.state),
            _ => None,
        })
        .collect::<Vec<LinkState>>();
    assert_eq!(
        states,
        [
            LinkState::Connecting,
            LinkState::Authenticating,
            LinkState::CatchingUp,
            LinkState::Live
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
