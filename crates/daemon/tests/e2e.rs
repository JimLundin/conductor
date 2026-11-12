//! Host and client in one process over real iroh connections on loopback, with
//! relays off so the test needs no network.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agents::codex::ServerOptions;
use client::host_view::{
    Approval, HostView, Item, ReplyState, RequestState, SessionStatus, SessionView, ToolState,
};
use client::{Cursor, HostLink, LinkOptions, LinkState, Network, PathKind, Status, Target, Update};
use daemon::Stopped;
use daemon::codex_hub::CodexHub;
use daemon::log::Log;
use daemon::registry::{Launcher, Launchers, Registry};
use daemon::rtc::{PortRange, RtcOptions, RtcPorts};
use daemon::serve::Conductor;
use daemon::session::{ClaudeLaunch, CodexLaunch, Launch};
use iroh::endpoint::{Connection, ReadExactError, RecvStream, SendStream, presets};
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, RelayMode};
use proto::{
    AuthToken, Availability, Change, ClientName, CloseReason, Command, CommandBody, CommandId,
    CommandResult, Decision, Epoch, Event, EventKind, FileChange, Hello, HelloReply, HostPath,
    HostSeq, Open, Provider, Providers, Rejection, RequestId, Resolution, RtcOffer, SessionId,
    Signal, ToolCallId, ToolKind, ToolOutcome, ToolPhase, TurnOutcome,
};
use serde::de::DeserializeOwned;
use str0m::change::{SdpAnswer, SdpPendingOffer};
use str0m::net::{DatagramRecv, Protocol, Receive, Transmit};
use str0m::{Candidate, Input, Output, Rtc};
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

/// The gated stand-in that plays `stdout.jsonl` from the session's directory.
fn fake_claude() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../agents/tests/fake-claude.sh")
}

/// The gated stand-in that plays `stdout.jsonl` from the app-server's
/// working directory.
fn fake_codex() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../agents/tests/fake-codex.sh")
}

/// A host whose Claude is `claude_path` and whose Codex is unavailable.
async fn host_with(claude_path: PathBuf) -> Fixture {
    host_of(
        Launchers {
            claude: Launcher::Ready(Launch::Claude(ClaudeLaunch {
                claude_path,
                model: None,
            })),
            codex: Launcher::Unavailable(String::from(UNAVAILABLE)),
        },
        None,
    )
    .await
}

/// A host whose Codex app-server is the stand-in, playing `recording` from
/// the directory it returns, and whose Claude is unavailable.
async fn codex_host(recording: &str) -> (Fixture, PathBuf) {
    let server = scratch(recording, recording);
    let hub = CodexHub::spawn(ServerOptions {
        codex_path: fake_codex(),
        cwd: server.clone(),
        stdout_log: None,
    });
    let fixture = host_of(
        Launchers {
            claude: Launcher::Unavailable(String::from(UNAVAILABLE)),
            codex: Launcher::Ready(Launch::Codex(CodexLaunch { hub, model: None })),
        },
        None,
    )
    .await;
    (fixture, server)
}

async fn host_of(launchers: Launchers, rtc: Option<Arc<RtcOptions>>) -> Fixture {
    let log = Log::spawn(String::from("test-host"), Epoch(11), launchers.providers());
    let registry = Registry::spawn(log.clone(), launchers);
    let endpoint = endpoint().await;
    let target = Target {
        addr: endpoint.addr(),
        token: AuthToken::from_bytes(TOKEN),
    };
    let conductor = Conductor {
        log: log.clone(),
        registry,
        token: AuthToken::from_bytes(TOKEN),
        rtc,
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

/// Appends events of one Claude session to the log.
async fn append(log: &Log, kinds: Vec<EventKind>) -> Result<(), Stopped> {
    for kind in kinds {
        log.append(SessionId(1), kind).await?;
    }
    Ok(())
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

/// The view of session 1 after `created()` and the reply `text` at
/// `activity`.
fn replied(text: &str, activity: u64) -> HostView {
    HostView {
        sessions: BTreeMap::from([(
            SessionId(1),
            SessionView {
                provider: Provider::Claude,
                directory: HostPath(String::from("/work")),
                status: SessionStatus::Starting,
                items: Vec::from([done(text)]),
                activity: HostSeq(activity),
            },
        )]),
    }
}

/// One command and the host's literal answer.
struct Case {
    body: CommandBody,
    result: CommandResult,
}

async fn host() -> Fixture {
    host_with(fake_claude()).await
}

fn start(provider: Provider, directory: &Path) -> CommandBody {
    CommandBody::StartSession {
        provider,
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
    assert_eq!(
        link.status().providers,
        Some(Providers {
            claude: Availability::Available,
            codex: Availability::Unavailable {
                reason: String::from(UNAVAILABLE),
            },
        })
    );
    assert_eq!(link.status().path, PathKind::Direct);

    let cases = Vec::from([
        Case {
            body: start(
                Provider::Claude,
                &std::env::temp_dir().join("conductor-e2e-missing"),
            ),
            result: CommandResult::Rejected(Rejection::NoSuchDirectory),
        },
        Case {
            body: start(Provider::Claude, &std::env::current_exe().unwrap()),
            result: CommandResult::Rejected(Rejection::NoSuchDirectory),
        },
        Case {
            body: start(Provider::Codex, &std::env::temp_dir()),
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
    append(&fx.log, Vec::from([created(), reply("Hel"), reply("lo")]))
        .await
        .unwrap();
    let before = events_through(&mut updates, HostSeq(3)).await;

    append(&fx.log, Vec::from([reply(" wor"), reply("ld")]))
        .await
        .unwrap();
    link.reconnect().await;
    append(&fx.log, Vec::from([reply("!")])).await.unwrap();
    let after = events_through(&mut updates, HostSeq(6)).await;
    let seen = before.into_iter().chain(after).collect::<Vec<Event>>();
    assert_eq!(seqs(&seen), [1, 2, 3, 4, 5, 6].map(HostSeq));
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_new_client_resumes_from_a_dead_clients_cursor() {
    let fx = host().await;
    let (first, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    append(&fx.log, Vec::from([created(), reply("Hel")]))
        .await
        .unwrap();
    events_through(&mut updates, HostSeq(2)).await;
    first.stop().await.unwrap();
    drop(updates);

    append(&fx.log, Vec::from([reply("Hello")])).await.unwrap();
    let cursor = Cursor {
        epoch: Epoch(11),
        seq: HostSeq(2),
    };
    let (second, mut updates) = link(fx.target.clone(), Some(cursor)).await;
    let rest = events_through(&mut updates, HostSeq(3)).await;
    assert_eq!(seqs(&rest), [HostSeq(3)]);
    second.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_live_client_sees_the_deltas_and_a_resumed_one_only_the_message() {
    let fx = host().await;
    let (first, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    append(&fx.log, Vec::from([created()])).await.unwrap();
    let seen = events_through(&mut updates, HostSeq(1)).await;
    append(
        &fx.log,
        Vec::from([delta("Hel"), delta("lo"), reply("Hello")]),
    )
    .await
    .unwrap();
    let live = events_through(&mut updates, HostSeq(2)).await;
    assert_eq!(seqs(&live), [1, 1, 2].map(HostSeq));
    assert_eq!(of(1, &live), [delta("Hel"), delta("lo"), reply("Hello")]);
    first.stop().await.unwrap();
    drop(updates);

    let cursor = Cursor {
        epoch: Epoch(11),
        seq: HostSeq(1),
    };
    let (second, mut updates) = link(fx.target.clone(), Some(cursor)).await;
    let rest = events_through(&mut updates, HostSeq(2)).await;
    assert_eq!(of(1, &rest), [reply("Hello")]);
    assert_eq!(
        fold(seen.iter().chain(&live).cloned().collect()),
        fold(seen.into_iter().chain(rest).collect())
    );
    second.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_cursor_from_another_epoch_starts_over_from_the_beginning() {
    let fx = host().await;
    append(&fx.log, Vec::from([created(), delta("H"), reply("Hi")]))
        .await
        .unwrap();
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
    assert_eq!(fold(replayed), replied("Hi", 2));
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// A session directory holding `fixtures/e2e/<recording>` as the fake's
/// `stdout.jsonl`.
fn scratch(name: &str, recording: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("conductor-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/e2e");
    std::fs::copy(fixture.join(recording), dir.join("stdout.jsonl")).unwrap();
    dir
}

/// The event a test reads until.
enum Until {
    Ready,
    Request,
    Resolved,
    Delta,
    TurnEnd,
    Closed,
}

enum Reached {
    Yes,
    No,
}

fn reached(until: &Until, kind: &EventKind) -> Reached {
    match (until, kind) {
        (Until::Ready, EventKind::SessionReady)
        | (Until::Request, EventKind::RequestOpened { .. })
        | (Until::Resolved, EventKind::RequestResolved { .. })
        | (Until::Delta, EventKind::AssistantDelta { .. })
        | (Until::TurnEnd, EventKind::TurnCompleted { .. })
        | (Until::Closed, EventKind::SessionClosed { .. }) => Reached::Yes,
        _ => Reached::No,
    }
}

/// The kinds of the events the link receives, through the first that
/// reaches `until`, for tests that start one session.
async fn kinds_until(updates: &mut mpsc::Receiver<Update>, until: Until) -> Vec<EventKind> {
    events_until(updates, until)
        .await
        .into_iter()
        .map(|event| event.kind)
        .collect()
}

/// The events the link receives, through the first that reaches `until`.
async fn events_until(updates: &mut mpsc::Receiver<Update>, until: Until) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        match next(updates).await {
            Update::Event(event) => {
                let done = reached(&until, &event.kind);
                events.push(event);
                match done {
                    Reached::Yes => break events,
                    Reached::No => {}
                }
            }
            Update::Status(_) | Update::Reset(_) => {}
        }
    }
}

/// The kinds of `session`'s events.
fn of(session: u64, events: &[Event]) -> Vec<EventKind> {
    events
        .iter()
        .filter(|event| event.session == SessionId(session))
        .map(|event| event.kind.clone())
        .collect()
}

async fn send(link: &HostLink, body: CommandBody) -> CommandResult {
    link.send(body).await.unwrap()
}

fn message(text: &str) -> CommandBody {
    message_to(1, text)
}

fn message_to(session: u64, text: &str) -> CommandBody {
    CommandBody::SendMessage {
        session: SessionId(session),
        text: String::from(text),
    }
}

fn interrupt() -> CommandBody {
    CommandBody::Interrupt {
        session: SessionId(1),
    }
}

fn resolve(request: u64, decision: Decision) -> CommandBody {
    resolve_in(1, request, decision)
}

fn resolve_in(session: u64, request: u64, decision: Decision) -> CommandBody {
    CommandBody::Resolve {
        session: SessionId(session),
        request: RequestId(request),
        decision,
    }
}

fn test_client() -> ClientName {
    ClientName(String::from("test-client"))
}

fn opening(provider: Provider, directory: &Path) -> Vec<EventKind> {
    Vec::from([
        EventKind::SessionCreated {
            provider,
            directory: HostPath(directory.display().to_string()),
            by: test_client(),
        },
        EventKind::SessionReady,
    ])
}

fn user(text: &str) -> EventKind {
    EventKind::UserMessage {
        text: String::from(text),
        by: test_client(),
    }
}

fn reply(text: &str) -> EventKind {
    EventKind::AssistantMessage {
        text: String::from(text),
    }
}

fn turn_end(outcome: TurnOutcome) -> EventKind {
    EventKind::TurnCompleted { outcome }
}

/// The `Write` call and its permission request, as the recording has them.
fn write_asked(call: &str, path: &str) -> Vec<EventKind> {
    Vec::from([
        EventKind::ToolCall {
            call: ToolCallId(String::from(call)),
            phase: ToolPhase::Started {
                tool: ToolKind::FileChange {
                    files: Vec::from([FileChange {
                        path: String::from(path),
                        change: Change::Write,
                    }]),
                },
            },
        },
        EventKind::RequestOpened {
            request: RequestId(1),
            call: ToolCallId(String::from(call)),
            tool: ToolKind::FileChange {
                files: Vec::from([FileChange {
                    path: String::from(path),
                    change: Change::Write,
                }]),
            },
            reason: None,
        },
    ])
}

fn decided(decision: Decision, call: &str, outcome: ToolOutcome) -> Vec<EventKind> {
    Vec::from([
        EventKind::RequestResolved {
            request: RequestId(1),
            resolution: Resolution::Decided {
                decision,
                by: test_client(),
            },
        },
        EventKind::ToolCall {
            call: ToolCallId(String::from(call)),
            phase: ToolPhase::Finished { outcome },
        },
    ])
}

fn items(view: &HostView, session: u64) -> Vec<Item> {
    view.sessions
        .get(&SessionId(session))
        .map(|session| session.items.clone())
        .unwrap_or_default()
}

fn done(text: &str) -> Item {
    Item::Reply {
        text: String::from(text),
        state: ReplyState::Done,
    }
}

/// A Codex command the user decided on, as the client shows it.
fn codex_tool(call: &str, command: &str, decision: Decision, outcome: ToolOutcome) -> Item {
    Item::Tool {
        call: ToolCallId(String::from(call)),
        tool: ToolKind::Command {
            command: String::from(command),
        },
        state: ToolState::Finished(outcome),
        approval: Approval::Asked {
            request: RequestId(1),
            reason: None,
            state: RequestState::Resolved(Resolution::Decided {
                decision,
                by: test_client(),
            }),
        },
    }
}

const APPROVED_CALL: &str = "toolu_bdrk_011tLLSAecKzNjYWMVcNwMfD";
const DENIED_CALL: &str = "toolu_bdrk_01WAz64VUtaiJZHcy71oavyx";

#[tokio::test]
async fn a_claude_turn_streams_asks_and_finishes() {
    let fx = host().await;
    let dir = scratch("approve", "claude-approve.jsonl");
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    assert_eq!(
        send(&link, start(Provider::Claude, &dir)).await,
        CommandResult::Started {
            session: SessionId(1)
        }
    );
    assert_eq!(
        send(&link, message("create a.txt")).await,
        CommandResult::Accepted
    );
    assert_eq!(
        send(&link, message("and b.txt")).await,
        CommandResult::Rejected(Rejection::TurnInProgress)
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Request).await,
        [
            opening(Provider::Claude, &dir),
            Vec::from([user("create a.txt")]),
            write_asked(APPROVED_CALL, "/tmp/s1-scratch/a.txt"),
        ]
        .concat()
    );
    let desk = LinkOptions {
        network: Network::Local,
        ..LinkOptions::new(ClientName(String::from("desk")))
    };
    let (desk, mut at_desk) = HostLink::spawn(fx.target.clone(), desk).await.unwrap();
    until_live(&mut at_desk).await;
    assert_eq!(
        [
            send(&link, resolve(1, Decision::Approve)).await,
            send(&desk, resolve(1, Decision::Deny)).await,
            send(&link, resolve(2, Decision::Approve)).await,
        ],
        [
            CommandResult::Accepted,
            CommandResult::Rejected(Rejection::AlreadyResolved { by: test_client() }),
            CommandResult::Rejected(Rejection::UnknownRequest),
        ]
    );
    let text = "I created `a.txt` in `/tmp/s1-scratch` with the content `hello from s1`.";
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await,
        [
            decided(Decision::Approve, APPROVED_CALL, ToolOutcome::Succeeded),
            [
                "I",
                " created `a.txt` in",
                " `/tmp/s1",
                "-scratch` with the content",
                " `hello from s1`.",
            ]
            .map(delta)
            .to_vec(),
            Vec::from([reply(text), turn_end(TurnOutcome::Completed)]),
        ]
        .concat()
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_denied_request_continues_and_an_interrupt_ends_the_turn() {
    let fx = host().await;
    let dir = scratch("deny", "claude-deny-interrupt.jsonl");
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let _ = send(&link, start(Provider::Claude, &dir)).await;
    assert_eq!(
        send(&link, interrupt()).await,
        CommandResult::Rejected(Rejection::NoTurnRunning)
    );
    let _ = send(&link, message("create b.txt")).await;
    assert_eq!(
        kinds_until(&mut updates, Until::Request).await,
        [
            opening(Provider::Claude, &dir),
            Vec::from([user("create b.txt")]),
            write_asked(DENIED_CALL, "/tmp/s1-scratch/b.txt"),
        ]
        .concat()
    );
    assert_eq!(
        send(&link, resolve(1, Decision::Deny)).await,
        CommandResult::Accepted
    );
    let text = "The write to `b.txt` was denied (\"The user does not want b.txt.\"), so I did not retry and no file was created.";
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await,
        [
            decided(Decision::Deny, DENIED_CALL, ToolOutcome::Declined),
            [
                "The write to",
                " `",
                "b.txt` was denied",
                " ",
                "(\"The user does not want b.",
                "txt.\"",
                "), so I did not retry and",
                " no file was created.",
            ]
            .map(delta)
            .to_vec(),
            Vec::from([reply(text), turn_end(TurnOutcome::Completed)]),
        ]
        .concat()
    );

    let _ = send(&link, message("count to 100")).await;
    assert_eq!(
        kinds_until(&mut updates, Until::Delta).await,
        [user("count to 100"), delta("1")]
    );
    assert_eq!(send(&link, interrupt()).await, CommandResult::Accepted);
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await,
        [reply("1"), turn_end(TurnOutcome::Interrupted)]
    );

    let _ = send(&link, message("ping")).await;
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await,
        [
            user("ping"),
            delta("pong"),
            reply("pong"),
            turn_end(TurnOutcome::Completed)
        ]
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// The recording starts the turn only once `release` exists, and the fake
/// answers only an interrupt sent after that (Codex's of that turn's id), so
/// this passes only if the session holds the interrupt until the turn starts.
async fn held_interrupt(fx: Fixture, dir: PathBuf, provider: Provider) {
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let _ = send(&link, start(provider, &dir)).await;
    let _ = send(&link, message("count to 300")).await;
    assert_eq!(send(&link, interrupt()).await, CommandResult::Accepted);
    std::fs::write(dir.join("release"), "").unwrap();
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await,
        [
            opening(provider, &dir),
            Vec::from([user("count to 300"), turn_end(TurnOutcome::Interrupted)]),
        ]
        .concat()
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_interrupt_before_the_turn_starts_is_held_until_it_does() {
    let dir = scratch("held", "claude-held-interrupt.jsonl");
    held_interrupt(host().await, dir, Provider::Claude).await;
}

/// The CLI withdraws the open request when the turn is interrupted, and the
/// tool call it asked about fails.
#[tokio::test]
async fn an_interrupt_withdraws_the_open_request() {
    let fx = host().await;
    let dir = scratch("withdraw", "claude-withdraw.jsonl");
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let _ = send(&link, start(Provider::Claude, &dir)).await;
    let _ = send(&link, message("create d.txt")).await;
    let call = "toolu_bdrk_01KEnaUr83k9BPcLoGpArHUq";
    assert_eq!(
        kinds_until(&mut updates, Until::Request).await,
        [
            opening(Provider::Claude, &dir),
            Vec::from([user("create d.txt")]),
            write_asked(call, "/tmp/s1-scratch/d.txt"),
        ]
        .concat()
    );
    assert_eq!(send(&link, interrupt()).await, CommandResult::Accepted);
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await,
        [
            EventKind::RequestResolved {
                request: RequestId(1),
                resolution: Resolution::Withdrawn,
            },
            EventKind::ToolCall {
                call: ToolCallId(String::from(call)),
                phase: ToolPhase::Finished {
                    outcome: ToolOutcome::Failed,
                },
            },
            turn_end(TurnOutcome::Interrupted),
        ]
    );
    assert_eq!(
        send(&link, resolve(1, Decision::Approve)).await,
        CommandResult::Rejected(Rejection::UnknownRequest)
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// The stand-in stops reading after `initialize`, so the message can't
/// reach it. The turn must end, or every client shows it running forever.
#[tokio::test]
async fn a_message_the_agent_never_gets_fails_its_turn() {
    let fx = host().await;
    let dir = scratch("hang-up", "claude-hang-up.jsonl");
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let _ = send(&link, start(Provider::Claude, &dir)).await;
    assert_eq!(send(&link, message("hi")).await, CommandResult::Accepted);
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await,
        [
            opening(Provider::Claude, &dir),
            Vec::from([
                user("hi"),
                turn_end(TurnOutcome::Failed {
                    message: String::from("claude i/o: Broken pipe (os error 32)")
                }),
            ]),
        ]
        .concat()
    );
    assert_eq!(
        send(&link, interrupt()).await,
        CommandResult::Rejected(Rejection::NoTurnRunning)
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Closed).await,
        [EventKind::SessionClosed {
            reason: CloseReason::Exited {
                status: String::from("exit status: 0"),
            },
        }]
    );
    assert_eq!(
        send(&link, message("hi")).await,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_missing_claude_binary_closes_the_session() {
    let fx = host_with(PathBuf::from("/nonexistent/claude")).await;
    let dir = std::env::temp_dir();
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let _ = send(&link, start(Provider::Claude, &dir)).await;
    assert_eq!(
        kinds_until(&mut updates, Until::Closed).await,
        [
            EventKind::SessionCreated {
                provider: Provider::Claude,
                directory: HostPath(dir.display().to_string()),
                by: test_client(),
            },
            EventKind::SessionClosed {
                reason: CloseReason::FailedToStart {
                    message: String::from("claude i/o: No such file or directory (os error 2)"),
                },
            },
        ]
    );
    assert_eq!(
        send(&link, message("hi")).await,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// Two sessions' turns run at once on one app-server: each session's events
/// are its own thread's, one approves and the other declines.
#[tokio::test]
async fn two_codex_sessions_share_one_server() {
    let (fx, dir) = codex_host("codex-two-threads.jsonl").await;
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let _ = send(&link, start(Provider::Codex, &dir)).await;
    // The second session starts once the first has its thread, so the first
    // gets the recording's first thread.
    let first = events_until(&mut updates, Until::Ready).await;
    let _ = send(&link, start(Provider::Codex, &dir)).await;
    let second = events_until(&mut updates, Until::Ready).await;
    assert_eq!(
        [
            send(&link, message_to(1, "create a.txt")).await,
            send(&link, message_to(2, "create b.txt")).await,
        ],
        [CommandResult::Accepted, CommandResult::Accepted]
    );
    let asked = Vec::from([
        events_until(&mut updates, Until::Request).await,
        events_until(&mut updates, Until::Request).await,
    ])
    .concat();
    assert_eq!(
        [
            send(&link, resolve_in(1, 1, Decision::Approve)).await,
            send(&link, resolve_in(2, 1, Decision::Deny)).await,
        ],
        [CommandResult::Accepted, CommandResult::Accepted]
    );
    let ended = Vec::from([
        events_until(&mut updates, Until::TurnEnd).await,
        events_until(&mut updates, Until::TurnEnd).await,
    ])
    .concat();
    let view = fold(Vec::from([first, second, asked, ended]).concat());
    let a_call = "exec-f4c7c6d4-8255-4bcc-972d-9f106cda7991";
    assert_eq!(
        items(&view, 1),
        [
            Item::User {
                text: String::from("create a.txt"),
                by: test_client(),
            },
            done("I’ll create `a.txt` with the exact requested contents."),
            codex_tool(
                a_call,
                "/usr/bin/zsh -lc \"printf %s 'hello from s2' > a.txt\"",
                Decision::Approve,
                ToolOutcome::Succeeded
            ),
            done("Created [a.txt](/tmp/s2-scratch/a.txt)."),
            Item::TurnCompleted(TurnOutcome::Completed),
        ]
    );
    assert_eq!(
        items(&view, 2),
        [
            Item::User {
                text: String::from("create b.txt"),
                by: test_client(),
            },
            codex_tool(
                "exec-5f0ba5d1-395f-4d84-b098-a443a0873a5a",
                "/usr/bin/zsh -lc 'printf %s nope > b.txt'",
                Decision::Deny,
                ToolOutcome::Declined
            ),
            done("The request was declined."),
            Item::TurnCompleted(TurnOutcome::Completed),
        ]
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_codex_interrupt_before_the_turn_starts_is_sent_by_turn_id() {
    let (fx, dir) = codex_host("codex-held-interrupt.jsonl").await;
    held_interrupt(fx, dir, Provider::Codex).await;
}

#[tokio::test]
async fn a_codex_interrupt_mid_stream_ends_the_turn_and_the_next_runs() {
    let (fx, dir) = codex_host("codex-interrupt.jsonl").await;
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let _ = send(&link, start(Provider::Codex, &dir)).await;
    let _ = send(&link, message("count to 300")).await;
    assert_eq!(
        kinds_until(&mut updates, Until::Delta).await,
        [
            opening(Provider::Codex, &dir),
            Vec::from([user("count to 300"), delta("1")]),
        ]
        .concat()
    );
    assert_eq!(send(&link, interrupt()).await, CommandResult::Accepted);
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await,
        [delta("\n"), turn_end(TurnOutcome::Interrupted)]
    );
    let _ = send(&link, message("ping")).await;
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await,
        [
            user("ping"),
            delta("pong"),
            reply("pong"),
            turn_end(TurnOutcome::Completed)
        ]
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// Codex withdraws the open request only after the interrupted turn ends,
/// and the recording holds that withdrawal until `release` exists. The
/// session withdraws the request at the turn's end, so an answer in between
/// is refused and never reaches Codex.
#[tokio::test]
async fn a_codex_interrupt_withdraws_the_open_request_at_the_turn_end() {
    let (fx, dir) = codex_host("codex-withdraw.jsonl").await;
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let _ = send(&link, start(Provider::Codex, &dir)).await;
    let _ = send(&link, message("create d.txt")).await;
    let call = ToolCallId(String::from("exec-c1e73e4d-1cac-431e-baf8-8bf5b779606b"));
    let tool = ToolKind::Command {
        command: String::from("/usr/bin/zsh -lc 'printf %s d > d.txt'"),
    };
    assert_eq!(
        kinds_until(&mut updates, Until::Request).await,
        [
            opening(Provider::Codex, &dir),
            Vec::from([
                user("create d.txt"),
                EventKind::ToolCall {
                    call: call.clone(),
                    phase: ToolPhase::Started { tool: tool.clone() },
                },
                EventKind::RequestOpened {
                    request: RequestId(1),
                    call,
                    tool,
                    reason: None,
                },
            ]),
        ]
        .concat()
    );
    assert_eq!(send(&link, interrupt()).await, CommandResult::Accepted);
    assert_eq!(
        kinds_until(&mut updates, Until::Resolved).await,
        [
            turn_end(TurnOutcome::Interrupted),
            EventKind::RequestResolved {
                request: RequestId(1),
                resolution: Resolution::Withdrawn,
            },
        ]
    );
    assert_eq!(
        send(&link, resolve(1, Decision::Approve)).await,
        CommandResult::Rejected(Rejection::UnknownRequest)
    );
    std::fs::write(dir.join("release"), "").unwrap();
    // Codex's withdrawal plays before the next turn and is ignored.
    let _ = send(&link, message("ping")).await;
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await,
        [
            user("ping"),
            delta("pong"),
            reply("pong"),
            turn_end(TurnOutcome::Completed)
        ]
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("stdin.jsonl"))
            .unwrap()
            .matches("\"decision\"")
            .count(),
        0
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// The stand-in exits once both threads have started. Both sessions close,
/// and the next session runs on a new app-server.
#[tokio::test]
async fn the_app_server_exiting_closes_every_codex_session() {
    let (fx, dir) = codex_host("codex-exit.jsonl").await;
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let _ = send(&link, start(Provider::Codex, &dir)).await;
    let first = events_until(&mut updates, Until::Ready).await;
    let _ = send(&link, start(Provider::Codex, &dir)).await;
    let seen = Vec::from([
        first,
        events_until(&mut updates, Until::Closed).await,
        events_until(&mut updates, Until::Closed).await,
    ])
    .concat();
    let closed = EventKind::SessionClosed {
        reason: CloseReason::Exited {
            status: String::from("exit status: 0"),
        },
    };
    assert_eq!(
        (of(1, &seen), of(2, &seen)),
        (
            [opening(Provider::Codex, &dir), Vec::from([closed.clone()])].concat(),
            [opening(Provider::Codex, &dir), Vec::from([closed])].concat(),
        )
    );
    assert_eq!(
        send(&link, message("hi")).await,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    assert_eq!(
        send(&link, start(Provider::Codex, &dir)).await,
        CommandResult::Started {
            session: SessionId(3)
        }
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Ready).await,
        opening(Provider::Codex, &dir)
    );
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
            (Datagram::FromClient { bytes, from }, Some(dead), _) if from != dead => {
                self.live = Some(from);
                let _ = self.inner.send(&bytes).await?;
                Ok(())
            }
            (Datagram::FromClient { .. }, Some(_), _) => Ok(()),
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
    let (log_path, _logging) = capture_log("stalled-dial");
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

    let logged = captured(&log_path);
    assert_eq!(
        (
            logged.matches("hedged dial connected").count(),
            logged.matches(UNGRACEFUL).count()
        ),
        (1, 0),
        "{logged}"
    );
}

/// Sends this thread's logs to a file named after `test`. A `tokio::test`
/// runs on one thread, so they include the host's.
fn capture_log(test: &str) -> (PathBuf, tracing::subscriber::DefaultGuard) {
    let path =
        std::env::temp_dir().join(format!("conductor-e2e-{test}-{}.log", std::process::id()));
    let log = std::fs::File::create(&path).unwrap();
    let guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_writer(Arc::new(log))
            .with_ansi(false)
            .finish(),
    );
    (path, guard)
}

/// The captured log, whose file is then removed.
fn captured(path: &Path) -> String {
    let logged = std::fs::read_to_string(path).unwrap();
    std::fs::remove_file(path).unwrap();
    logged
}

/// A host with no agents whose WebRTC sockets bind in `first..=last`. It
/// has no STUN servers, so the test needs no network.
async fn rtc_host(first: u16, last: u16) -> Fixture {
    let ports = RtcPorts::Range(PortRange::parse(&format!("{first}-{last}")).unwrap());
    let launchers = Launchers {
        claude: Launcher::Unavailable(String::from(UNAVAILABLE)),
        codex: Launcher::Unavailable(String::from(UNAVAILABLE)),
    };
    host_of(
        launchers,
        Some(Arc::new(RtcOptions::new(Vec::new(), None, None, ports))),
    )
    .await
}

/// A UDP port that no socket holds now, picked by the OS, so the test
/// never depends on a fixed port or on another copy of itself.
fn free_port() -> u16 {
    std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The first of two adjacent free UDP ports. The OS picks the first; a few
/// tries find one whose neighbour is free too.
fn free_pair() -> u16 {
    std::iter::repeat_n(Ipv4Addr::UNSPECIFIED, 20)
        .find_map(adjacent_free)
        .expect("two adjacent free UDP ports within 20 tries")
}

fn adjacent_free(ip: Ipv4Addr) -> Option<u16> {
    let first = std::net::UdpSocket::bind((ip, 0)).ok()?;
    let port = first.local_addr().ok()?.port();
    let _second = std::net::UdpSocket::bind((ip, port.checked_add(1)?)).ok()?;
    Some(port)
}

/// An authenticated iroh connection without a link, for the raw streams a
/// browser opens.
async fn said_hello(endpoint: &Endpoint, target: &Target) -> Connection {
    let conn = endpoint
        .connect(target.addr.clone(), proto::ALPN)
        .await
        .unwrap();
    let hello = Open::Hello(Hello {
        client: test_client(),
        token: target.token,
    });
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    send.write_all(&proto::encode(&hello).unwrap())
        .await
        .unwrap();
    send.finish().unwrap();
    let welcomed = match read_frame::<HelloReply>(&mut recv).await.unwrap() {
        Some(HelloReply::Welcome(_)) => Some(conn),
        Some(HelloReply::Refused { .. }) | None => None,
    };
    welcomed.expect("the host welcomes the test client")
}

/// The next frame, or `None` once the peer finished the stream.
async fn read_frame<T: DeserializeOwned>(
    recv: &mut RecvStream,
) -> Result<Option<T>, ReadExactError> {
    let mut header = [0u8; proto::HEADER_LEN];
    match recv.read_exact(&mut header).await {
        Ok(()) => {
            let mut body =
                std::iter::repeat_n(0u8, proto::body_len(header).unwrap()).collect::<Vec<u8>>();
            recv.read_exact(&mut body).await.unwrap();
            Ok(Some(proto::decode(&body).unwrap()))
        }
        Err(ReadExactError::FinishedEarly(0)) => Ok(None),
        Err(err) => Err(err),
    }
}

/// A browser that has made its offer and waits for the answer.
struct Offering {
    rtc: Rtc,
    pending: SdpPendingOffer,
    offer: RtcOffer,
}

/// The offer a browser makes: one data channel.
fn offering() -> Offering {
    let mut rtc = Rtc::builder()
        .set_crypto_provider(Arc::new(str0m::crypto::from_feature_flags()))
        .build(Instant::now());
    let mut changes = rtc.sdp_api();
    let _ = changes.add_channel(String::from("probe"));
    let (offer, pending) = changes.apply().unwrap();
    Offering {
        rtc,
        pending,
        offer: RtcOffer {
            sdp: offer.to_sdp_string(),
        },
    }
}

/// A WebRTC upgrade's signaling stream. The host's session lives while it
/// stays open.
struct Signaling {
    send: SendStream,
    recv: RecvStream,
    /// The host's first signal, or `None` if it refused the offer.
    answer: Option<Signal>,
}

async fn offer(conn: &Connection) -> Signaling {
    send_offer(conn, offering().offer).await
}

async fn send_offer(conn: &Connection, offer: RtcOffer) -> Signaling {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let offer = proto::encode(&Open::Rtc(offer)).unwrap();
    send.write_all(&offer).await.unwrap();
    let answer = tokio::time::timeout(STEP, read_frame::<Signal>(&mut recv))
        .await
        .expect("the host answers or refuses within the step timeout")
        .unwrap();
    Signaling { send, recv, answer }
}

/// The ports of the host candidates in the answer.
fn host_ports(signaling: &Signaling) -> Vec<u16> {
    match &signaling.answer {
        Some(Signal::Answer { sdp }) => sdp
            .lines()
            .filter_map(|line| line.strip_prefix("a="))
            .filter_map(|line| Candidate::from_sdp_string(line).ok())
            .map(|candidate| candidate.addr().port())
            .collect::<BTreeSet<u16>>()
            .into_iter()
            .collect(),
        Some(Signal::Candidate { .. }) | None => Vec::new(),
    }
}

/// Closes the signaling stream, which ends the session, and waits until the
/// host closes its side.
async fn hang_up(signaling: Signaling) {
    let Signaling {
        mut send, mut recv, ..
    } = signaling;
    send.finish().unwrap();
    let rest = tokio::time::timeout(STEP, read_frame::<Signal>(&mut recv))
        .await
        .expect("the host ends the session within the step timeout")
        .unwrap();
    assert_eq!(rest, None);
}

async fn command(conn: &Connection, body: CommandBody) -> Option<CommandResult> {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    let command = Open::Command(Command {
        id: CommandId(1),
        body,
    });
    send.write_all(&proto::encode(&command).unwrap())
        .await
        .unwrap();
    send.finish().unwrap();
    read_frame::<CommandResult>(&mut recv).await.unwrap()
}

#[tokio::test]
async fn concurrent_webrtc_sessions_take_distinct_ports_in_the_range() {
    let first_port = free_pair();
    let second_port = first_port.checked_add(1).unwrap();
    let fx = rtc_host(first_port, second_port).await;
    let endpoint = endpoint().await;
    let conn = said_hello(&endpoint, &fx.target).await;
    let first = offer(&conn).await;
    let second = offer(&conn).await;
    assert_eq!(
        (host_ports(&first), host_ports(&second)),
        (Vec::from([first_port]), Vec::from([second_port]))
    );
    hang_up(first).await;
    hang_up(second).await;
    endpoint.close().await;
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_full_range_refuses_the_upgrade_and_iroh_keeps_serving() {
    let (log_path, _logging) = capture_log("full-range");
    let port = free_port();
    let fx = rtc_host(port, port).await;
    let endpoint = endpoint().await;
    let conn = said_hello(&endpoint, &fx.target).await;
    let holder = offer(&conn).await;
    let refused = offer(&conn).await;
    let on_iroh = command(&conn, message_to(9, "still there?")).await;
    hang_up(holder).await;
    let after = offer(&conn).await;
    assert_eq!(
        (refused.answer, on_iroh, host_ports(&after)),
        (
            None,
            Some(CommandResult::Rejected(Rejection::UnknownSession)),
            Vec::from([port])
        )
    );
    hang_up(after).await;
    endpoint.close().await;
    fx.router.shutdown().await.unwrap();
    let logged = captured(&log_path);
    assert_eq!(
        logged
            .matches(&format!(
                "every UDP port in --rtc-ports {port}-{port} is taken"
            ))
            .count(),
        1,
        "{logged}"
    );
}

/// The host's first datagram to `socket`, or `None` if none arrives within
/// two seconds.
async fn first_from_host(socket: &UdpSocket) -> Option<(Vec<u8>, SocketAddr)> {
    let mut bytes = Vec::with_capacity(DATAGRAM);
    let received = tokio::time::timeout(Duration::from_secs(2), socket.recv_buf_from(&mut bytes))
        .await
        .ok()?
        .unwrap();
    let (_, from) = received;
    Some((bytes, from))
}

/// The host candidates' addresses in the answer.
fn host_addrs(signaling: &Signaling) -> Vec<SocketAddr> {
    match &signaling.answer {
        Some(Signal::Answer { sdp }) => sdp
            .lines()
            .filter_map(|line| line.strip_prefix("a="))
            .filter_map(|line| Candidate::from_sdp_string(line).ok())
            .map(|candidate| candidate.addr())
            .collect(),
        Some(Signal::Candidate { .. }) | None => Vec::new(),
    }
}

/// The candidates Safari trickles on mobile data: an mDNS host candidate the
/// host can't resolve, and a srflx candidate at `public` with the related
/// address zeroed.
fn safari_candidates(public: SocketAddr) -> Vec<Signal> {
    Vec::from([
        Signal::Candidate {
            candidate: String::from(
                "candidate:3217215570 1 udp 2113939711 \
                 d9fe3477-56cf-45d1-9625-8ebc4f10e456.local 61390 typ host \
                 generation 0 network-cost 999",
            ),
        },
        Signal::Candidate {
            candidate: format!(
                "candidate:842163049 1 udp 1685987071 {} {} typ srflx \
                 raddr 0.0.0.0 rport 0 generation 0 network-cost 999",
                public.ip(),
                public.port()
            ),
        },
    ])
}

async fn trickle(send: &mut SendStream, signals: Vec<Signal>) -> anyhow::Result<()> {
    let frames = signals
        .iter()
        .map(proto::encode)
        .collect::<Result<Vec<Vec<u8>>, proto::FrameError>>()?;
    for frame in frames {
        send.write_all(&frame).await?;
    }
    Ok(())
}

/// STUN's message type for a Binding request (RFC 5389 §6).
fn stun_type(bytes: &[u8]) -> Option<Vec<u8>> {
    bytes.split_at_checked(2).map(|(head, _)| head.to_vec())
}

#[tokio::test]
async fn the_host_checks_a_srflx_candidate_before_hearing_from_it() {
    let port = free_port();
    let fx = rtc_host(port, port).await;
    let endpoint = endpoint().await;
    let conn = said_hello(&endpoint, &fx.target).await;
    let mut signaling = offer(&conn).await;
    let host = host_addrs(&signaling).into_iter().next().unwrap();
    let browser = UdpSocket::bind((host.ip(), 0)).await.unwrap();
    trickle(
        &mut signaling.send,
        safari_candidates(browser.local_addr().unwrap()),
    )
    .await
    .unwrap();
    let first = first_from_host(&browser).await;
    assert_eq!(
        first.map(|(bytes, from)| (stun_type(&bytes), from)),
        Some((Some(Vec::from([0u8, 1])), host))
    );
    hang_up(signaling).await;
    endpoint.close().await;
    fx.router.shutdown().await.unwrap();
}

/// Whether the host's side of the path lets the browser's datagrams in. Like
/// a stateful firewall in front of the host, it opens once the host has sent
/// to the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pinhole {
    Closed,
    Open,
}

enum Progress {
    Going,
    Connected,
}

/// A browser on mobile data: the host knows it only by its srflx candidate,
/// and can reach it only by sending first.
struct SrflxBrowser {
    rtc: Rtc,
    socket: UdpSocket,
    pinhole: Pinhole,
}

impl SrflxBrowser {
    fn answered(offering: Offering, signaling: &Signaling, socket: UdpSocket) -> SrflxBrowser {
        let Offering {
            mut rtc, pending, ..
        } = offering;
        let sdp = match &signaling.answer {
            Some(Signal::Answer { sdp }) => Some(sdp),
            Some(Signal::Candidate { .. }) | None => None,
        };
        let answer = SdpAnswer::from_sdp_string(sdp.unwrap()).unwrap();
        rtc.sdp_api().accept_answer(pending, answer).unwrap();
        // The browser's own agent knows its interface; the host hears of it
        // only as an mDNS name it can't resolve.
        let _ = rtc
            .add_local_candidate(Candidate::host(socket.local_addr().unwrap(), "udp").unwrap())
            .unwrap();
        rtc.handle_input(Input::Timeout(Instant::now())).unwrap();
        SrflxBrowser {
            rtc,
            socket,
            pinhole: Pinhole::Closed,
        }
    }

    /// Drives the connection until DTLS is up, and says whether the host
    /// had opened the path by then.
    async fn connect(mut self) -> Pinhole {
        loop {
            match self.turn().await {
                Progress::Going => {}
                Progress::Connected => break self.pinhole,
            }
        }
    }

    async fn turn(&mut self) -> Progress {
        match self.rtc.poll_output().unwrap() {
            Output::Transmit(transmit) => {
                self.send(transmit).await;
                Progress::Going
            }
            Output::Event(str0m::Event::Connected) => Progress::Connected,
            Output::Event(_) => Progress::Going,
            Output::Timeout(deadline) => {
                self.wait(deadline).await;
                Progress::Going
            }
        }
    }

    async fn send(&self, transmit: Transmit) {
        match self.pinhole {
            Pinhole::Open => {
                let _ = self
                    .socket
                    .send_to(&transmit.contents, transmit.destination)
                    .await
                    .unwrap();
            }
            Pinhole::Closed => {}
        }
    }

    async fn wait(&mut self, deadline: Instant) {
        let mut bytes = Vec::with_capacity(DATAGRAM);
        let received =
            tokio::time::timeout_at(deadline.into(), self.socket.recv_buf_from(&mut bytes)).await;
        let input = match received {
            Ok(received) => {
                let (_, source) = received.unwrap();
                self.pinhole = Pinhole::Open;
                Input::Receive(
                    Instant::now(),
                    Receive {
                        proto: Protocol::Udp,
                        source,
                        destination: self.socket.local_addr().unwrap(),
                        contents: DatagramRecv::try_from(bytes.as_slice()).unwrap(),
                    },
                )
            }
            Err(_) => Input::Timeout(Instant::now()),
        };
        self.rtc.handle_input(input).unwrap();
    }
}

#[tokio::test]
async fn a_browser_with_only_a_srflx_candidate_connects_through_the_hosts_first_check() {
    let port = free_port();
    let fx = rtc_host(port, port).await;
    let endpoint = endpoint().await;
    let conn = said_hello(&endpoint, &fx.target).await;
    let offering = offering();
    let mut signaling = send_offer(&conn, offering.offer.clone()).await;
    let host = host_addrs(&signaling).into_iter().next().unwrap();
    let socket = UdpSocket::bind((host.ip(), 0)).await.unwrap();
    trickle(
        &mut signaling.send,
        safari_candidates(socket.local_addr().unwrap()),
    )
    .await
    .unwrap();
    let browser = SrflxBrowser::answered(offering, &signaling, socket);
    let connected = tokio::time::timeout(STEP, browser.connect()).await;
    assert_eq!(connected.ok(), Some(Pinhole::Open));
    hang_up(signaling).await;
    endpoint.close().await;
    fx.router.shutdown().await.unwrap();
}
