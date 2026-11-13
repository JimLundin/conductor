//! Host and client in one process over real iroh connections on loopback, with
//! relays off so the test needs no network.

use std::cmp::Ordering;
use std::convert::Infallible;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agents::Home;
use agents::claude::Sandbox;
use agents::codex::ServerOptions;
use client::host_view::{
    Approval, HostView, Item, ReplyState, RequestState, SessionStatus, ToolState,
};
use client::{Cursor, HostLink, LinkOptions, LinkState, Network, Status, Target, Update};
use daemon::Stopped;
use daemon::codex_hub::CodexHub;
use daemon::credentials::Store;
use daemon::log::Log;
use daemon::registry::{Launcher, Launchers, OwnSignIn, Registry};
use daemon::serve::Conductor;
use daemon::session::{ClaudeLaunch, CodexLaunch, Launch};
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, RelayMode};
use proto::{
    Answer, Auth, AuthToken, Availability, Change, ClaudeCredential, ClientName, CloseReason,
    CodexCredential, CommandBody, CommandResult, Credential, Decision, Effort, Epoch, Event,
    EventKind, FileChange, HostPath, HostSeq, ModelChoice, ModelId, Models, Provider, Providers,
    Question, QuestionOption, Rejection, RequestId, Resolution, Selection, SessionId, ToolCallId,
    ToolKind, ToolOutcome, ToolPhase, TurnOutcome,
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
    host_of(Launchers {
        claude: Launcher::Ready {
            launch: Launch::Claude(ClaudeLaunch {
                claude_path,
                home: Home::User,
                credential: None,
            }),
            own: OwnSignIn::Unmanaged,
            models: models(),
            chosen: None,
        },
        codex: Launcher::Unavailable(String::from(UNAVAILABLE)),
    })
    .await
}

/// A host whose Codex app-server is the stand-in, playing `recording` from
/// the directory it returns, and whose Claude is unavailable.
async fn codex_host(recording: &str) -> (Fixture, PathBuf) {
    let server = scratch(recording, recording);
    let hub = CodexHub::spawn(ServerOptions {
        codex_path: fake_codex(),
        home: Home::User,
        credential: None,
        cwd: server.clone(),
        stdout_log: None,
    });
    let fixture = host_of(Launchers {
        claude: Launcher::Unavailable(String::from(UNAVAILABLE)),
        codex: Launcher::Ready {
            launch: Launch::Codex(CodexLaunch { hub }),
            own: OwnSignIn::Unmanaged,
            models: models(),
            chosen: None,
        },
    })
    .await;
    (fixture, server)
}

/// A host with a data dir of its own.
async fn host_of(launchers: Launchers) -> Fixture {
    let endpoint = endpoint().await;
    let data = data_dir(&endpoint.id().to_string());
    serve(endpoint, launchers, &data).await
}

/// `tmp` in the build directory, found from this test binary at
/// `target/debug/deps/`, so scratch stays out of `/tmp` and `cargo clean` or
/// removing the worktree clears it.
fn target_tmp() -> PathBuf {
    let dir = std::env::current_exe()
        .unwrap()
        .ancestors()
        .nth(3)
        .unwrap()
        .join("tmp");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An empty data dir for a host. Its name has `conductor-test` in it, so
/// the fake CLIs show the paths under it (fake-cli.py).
fn data_dir(name: &str) -> PathBuf {
    let dir = target_tmp().join(format!("conductor-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A host on `endpoint`, keeping its credentials in `data`.
async fn serve(endpoint: Endpoint, launchers: Launchers, data: &Path) -> Fixture {
    let (store, stored) = Store::open(data).unwrap();
    let log = Log::spawn(String::from("test-host"), Epoch(11));
    let registry = Registry::spawn(log.clone(), launchers, store, stored);
    let _ = tokio::time::timeout(STEP, registry.providers())
        .await
        .expect("registry restoration within the step timeout")
        .unwrap();
    let target = Target {
        addr: endpoint.addr(),
        token: AuthToken::from_bytes(TOKEN),
    };
    let conductor = Conductor {
        log: log.clone(),
        registry,
        token: AuthToken::from_bytes(TOKEN),
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

/// A real host that reopens its database, endpoint identity and ticket secret.
async fn durable_host(data: &Path, address: SocketAddr) -> Fixture {
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .secret_key(daemon::load_or_create_key(data).unwrap())
        .bind_addr(address)
        .unwrap()
        .bind()
        .await
        .unwrap();
    let token = daemon::load_or_create_token(data).unwrap();
    let log = Log::open(String::from("test-host"), &data.join("conductor.redb")).unwrap();
    let (store, stored) = Store::open(data).unwrap();
    let registry = Registry::spawn(
        log.clone(),
        Launchers {
            claude: Launcher::Ready {
                launch: Launch::Claude(ClaudeLaunch {
                    claude_path: fake_claude(),
                    home: Home::User,
                    credential: None,
                }),
                own: OwnSignIn::Unmanaged,
                models: models(),
                chosen: None,
            },
            codex: Launcher::Unavailable(String::from(UNAVAILABLE)),
        },
        store,
        stored,
    );
    let _ = tokio::time::timeout(STEP, registry.providers())
        .await
        .expect("registry restoration within the step timeout")
        .unwrap();
    let target = Target {
        addr: endpoint.addr(),
        token,
    };
    let router = Router::builder(endpoint)
        .accept(
            proto::ALPN,
            Conductor {
                log: log.clone(),
                registry,
                token,
            },
        )
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
        model: None,
        effort: None,
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
        model: None,
        effort: None,
    }
}

/// What every test host's CLIs offer: a default with two efforts and a
/// model with one.
fn models() -> Models {
    Models {
        choices: Vec::from([
            ModelChoice {
                id: ModelId(String::from("big")),
                name: String::from("Big"),
                description: String::from("thinks"),
                efforts: Vec::from([Effort::Low, Effort::High]),
            },
            ModelChoice {
                id: ModelId(String::from("small")),
                name: String::from("Small"),
                description: String::from("quick"),
                efforts: Vec::from([Effort::Low]),
            },
        ]),
        default: Some(ModelId(String::from("big"))),
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
            claude: Availability::Available {
                auth: Auth::Unmanaged,
                models: models(),
            },
            codex: Availability::Unavailable {
                reason: String::from(UNAVAILABLE),
            },
        })
    );

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
    assert_eq!(
        of(SessionId(1), &live),
        [delta("Hel"), delta("lo"), reply("Hello")]
    );
    first.stop().await.unwrap();
    drop(updates);

    let cursor = Cursor {
        epoch: Epoch(11),
        seq: HostSeq(1),
    };
    let (second, mut updates) = link(fx.target.clone(), Some(cursor)).await;
    let rest = events_through(&mut updates, HostSeq(2)).await;
    assert_eq!(of(SessionId(1), &rest), [reply("Hello")]);
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
    let view = fold(replayed);
    assert_eq!(view.sessions.len(), 1);
    assert_eq!(
        view.sessions.get(&SessionId(1)).map(|session| (
            session.provider,
            session.directory.clone(),
            session.status.clone(),
            session.items.clone(),
            session.activity,
        )),
        Some((
            Provider::Claude,
            HostPath(String::from("/work")),
            SessionStatus::Starting,
            Vec::from([done("Hi")]),
            HostSeq(2)
        ))
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// Git commands use this test repository's identity, never the user's config.
fn git(directory: &Path, arguments: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(directory)
        .args([
            "-c",
            "user.name=Conductor test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(arguments)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn committed_repository(directory: &Path) {
    git(directory, &["init", "--quiet"]);
    std::fs::write(directory.join("baseline.txt"), "committed baseline\n").unwrap();
    git(directory, &["add", "baseline.txt"]);
    git(
        directory,
        &[
            "commit",
            "--quiet",
            "-m",
            "test baseline\n\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>",
        ],
    );
}

/// A session directory holding `fixtures/e2e/<recording>` as the fake's
/// `stdout.jsonl`.
fn scratch(name: &str, recording: &str) -> PathBuf {
    let dir = target_tmp().join(format!("conductor-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/e2e");
    std::fs::copy(fixture.join(recording), dir.join("stdout.jsonl")).unwrap();
    committed_repository(&dir);
    dir
}

/// The event a test reads until.
enum Until {
    Ready,
    Request,
    Question,
    Resolved,
    Delta,
    TurnEnd,
    Closed,
    Undone,
    UndoFailure,
}

enum Reached {
    Yes,
    No,
}

fn reached(until: &Until, kind: &EventKind) -> Reached {
    match (until, kind) {
        (Until::Ready, EventKind::SessionReady)
        | (Until::Request, EventKind::RequestOpened { .. })
        | (Until::Question, EventKind::QuestionAsked { .. })
        | (Until::Resolved, EventKind::RequestResolved { .. })
        | (Until::Delta, EventKind::AssistantDelta { .. })
        | (Until::TurnEnd, EventKind::TurnCompleted { .. })
        | (Until::Closed, EventKind::SessionClosed { .. })
        | (Until::Undone, EventKind::TurnsUndone { .. })
        | (Until::UndoFailure, EventKind::UndoFailed { .. }) => Reached::Yes,
        _ => Reached::No,
    }
}

/// The kinds of the events the link receives, through the first that
/// reaches `until`, for tests that start one session.
async fn kinds_until(updates: &mut mpsc::Receiver<Update>, until: Until) -> Vec<EventKind> {
    legacy_events_until(updates, until)
        .await
        .into_iter()
        .map(|event| event.kind)
        .collect()
}

/// Preserve existing protocol assertions while workspace metadata has dedicated
/// raw-event coverage below.
async fn legacy_events_until(updates: &mut mpsc::Receiver<Update>, until: Until) -> Vec<Event> {
    events_until(updates, until)
        .await
        .into_iter()
        .filter(|event| {
            !matches!(
                event.kind,
                EventKind::WorkspaceCreated { .. }
                    | EventKind::CheckpointCreated { .. }
                    | EventKind::CheckpointReserved { .. }
                    | EventKind::HistoryMaterialized { .. }
                    | EventKind::NativeTurnStarted { .. }
            )
        })
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
fn of(session: SessionId, events: &[Event]) -> Vec<EventKind> {
    events
        .iter()
        .filter(|event| event.session == session)
        .map(|event| event.kind.clone())
        .filter(|kind| {
            !matches!(
                kind,
                EventKind::WorkspaceCreated { .. }
                    | EventKind::CheckpointCreated { .. }
                    | EventKind::CheckpointReserved { .. }
                    | EventKind::HistoryMaterialized { .. }
                    | EventKind::NativeTurnStarted { .. }
            )
        })
        .collect()
}

async fn send(link: &HostLink, body: CommandBody) -> CommandResult {
    tokio::time::timeout(STEP, link.send(body))
        .await
        .expect("a command result within the step timeout")
        .unwrap()
}

fn started_session(result: CommandResult) -> SessionId {
    match result {
        CommandResult::Started { session } => Some(session),
        _ => None,
    }
    .expect("a started session")
}

fn message(session: SessionId, text: &str) -> CommandBody {
    CommandBody::SendMessage {
        session,
        text: String::from(text),
    }
}

fn interrupt(session: SessionId) -> CommandBody {
    CommandBody::Interrupt { session }
}

fn resolve(session: SessionId, request: u64, decision: Decision) -> CommandBody {
    CommandBody::Resolve {
        session,
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
            model: Some(ModelId(String::from("big"))),
            effort: None,
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
            for_session: None,
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

fn items(view: &HostView, session: SessionId) -> Vec<Item> {
    view.sessions
        .get(&session)
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
            for_session: Some(String::from(command)),
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
    let session = started_session(send(&link, start(Provider::Claude, &dir)).await);
    assert_eq!(
        send(&link, message(session, "create a.txt")).await,
        CommandResult::Accepted
    );
    assert_eq!(
        send(&link, message(session, "and b.txt")).await,
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
            send(&link, resolve(session, 1, Decision::Approve)).await,
            send(&desk, resolve(session, 1, Decision::Deny)).await,
            send(&link, resolve(session, 2, Decision::Approve)).await,
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
    let session = started_session(send(&link, start(Provider::Claude, &dir)).await);
    assert_eq!(
        send(&link, interrupt(session)).await,
        CommandResult::Rejected(Rejection::NoTurnRunning)
    );
    let _ = send(&link, message(session, "create b.txt")).await;
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
        send(&link, resolve(session, 1, Decision::Deny)).await,
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

    let _ = send(&link, message(session, "count to 100")).await;
    assert_eq!(
        kinds_until(&mut updates, Until::Delta).await,
        [user("count to 100"), delta("1")]
    );
    assert_eq!(
        send(&link, interrupt(session)).await,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await,
        [reply("1"), turn_end(TurnOutcome::Interrupted)]
    );

    let _ = send(&link, message(session, "ping")).await;
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
    let session = started_session(send(&link, start(provider, &dir)).await);
    let _ = send(&link, message(session, "count to 300")).await;
    assert_eq!(
        send(&link, interrupt(session)).await,
        CommandResult::Accepted
    );
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
    let session = started_session(send(&link, start(Provider::Claude, &dir)).await);
    let _ = send(&link, message(session, "create d.txt")).await;
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
    assert_eq!(
        send(&link, interrupt(session)).await,
        CommandResult::Accepted
    );
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
        send(&link, resolve(session, 1, Decision::Approve)).await,
        CommandResult::Rejected(Rejection::UnknownRequest)
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

fn answer(session: SessionId, request: u64, words: &[&str]) -> CommandBody {
    CommandBody::Answer {
        session,
        request: RequestId(request),
        answers: Vec::from([Answer(words.iter().copied().map(String::from).collect())]),
    }
}

/// A question with the options `s1` and `s2` asked for, as the model
/// described them.
fn asked(header: &str, text: &str, options: &[(&str, &str)]) -> EventKind {
    EventKind::QuestionAsked {
        request: RequestId(1),
        questions: Vec::from([Question {
            header: String::from(header),
            text: String::from(text),
            options: options
                .iter()
                .map(|(label, description)| QuestionOption {
                    label: label.to_string(),
                    description: description.to_string(),
                })
                .collect(),
            selection: Selection::One,
        }]),
    }
}

fn answered_blue() -> EventKind {
    EventKind::RequestResolved {
        request: RequestId(1),
        resolution: Resolution::Answered {
            answers: Vec::from([Answer(Vec::from([String::from("Blue")]))]),
            by: test_client(),
        },
    }
}

fn started(call: &str, tool: &ToolKind) -> EventKind {
    EventKind::ToolCall {
        call: ToolCallId(String::from(call)),
        phase: ToolPhase::Started { tool: tool.clone() },
    }
}

fn finished(call: &str, outcome: ToolOutcome) -> EventKind {
    EventKind::ToolCall {
        call: ToolCallId(String::from(call)),
        phase: ToolPhase::Finished { outcome },
    }
}

/// What the session asked and did, without the text it streamed.
fn calls_and_requests(kinds: Vec<EventKind>) -> Vec<EventKind> {
    kinds
        .into_iter()
        .filter(|kind| {
            !matches!(
                kind,
                EventKind::AssistantDelta { .. } | EventKind::AssistantMessage { .. }
            )
        })
        .collect()
}

/// What the session wrote to the stand-in's stdin.
fn sent_to_agent(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("stdin.jsonl")).unwrap()
}

/// Claude asks with `AskUserQuestion`. The first client's answer goes back
/// in the tool's input; a second client, a decision and the wrong number of
/// answers are refused.
#[tokio::test]
async fn a_claude_question_is_answered_once_by_the_first_client() {
    let fx = host().await;
    let dir = scratch("question", "claude-question.jsonl");
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(send(&link, start(Provider::Claude, &dir)).await);
    let _ = send(&link, message(session, "ask me")).await;
    assert_eq!(
        kinds_until(&mut updates, Until::Question).await,
        [
            opening(Provider::Claude, &dir),
            Vec::from([
                user("ask me"),
                asked(
                    "Color",
                    "Which color do you prefer?",
                    &[
                        ("Red", "Choose red as your preferred color."),
                        ("Blue", "Choose blue as your preferred color."),
                    ],
                ),
            ]),
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
            send(&link, resolve(session, 1, Decision::Approve)).await,
            send(
                &link,
                CommandBody::Answer {
                    session,
                    request: RequestId(1),
                    answers: Vec::new(),
                }
            )
            .await,
            send(&link, answer(session, 1, &["Blue"])).await,
            send(&desk, answer(session, 1, &["Red"])).await,
        ],
        [
            CommandResult::Rejected(Rejection::WrongReply {
                reason: String::from("a question takes answers, not a decision"),
            }),
            CommandResult::Rejected(Rejection::WrongReply {
                reason: String::from("0 answers for 1 questions"),
            }),
            CommandResult::Accepted,
            CommandResult::Rejected(Rejection::AlreadyResolved { by: test_client() }),
        ]
    );
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await,
        [
            answered_blue(),
            finished(
                "toolu_bdrk_01CcDpZHC4vQ3AZi8neyebkS",
                ToolOutcome::Succeeded
            ),
            delta("Blue"),
            reply("Blue"),
            turn_end(TurnOutcome::Completed),
        ]
    );
    assert_eq!(
        sent_to_agent(&dir)
            .matches(r#""answers":{"Which color do you prefer?":"Blue"}"#)
            .count(),
        1
    );
    link.stop().await.unwrap();
    desk.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// Codex asks with `requestUserInput`, answered as an RPC result keyed by
/// the question's id, which the stand-in waits for.
#[tokio::test]
async fn a_codex_question_is_answered_by_its_question_id() {
    let (fx, dir) = codex_host("codex-question.jsonl").await;
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(send(&link, start(Provider::Codex, &dir)).await);
    let _ = send(&link, message(session, "ask me")).await;
    assert_eq!(
        kinds_until(&mut updates, Until::Question).await,
        [
            opening(Provider::Codex, &dir),
            Vec::from([
                user("ask me"),
                asked(
                    "Color",
                    "Which color do you prefer?",
                    &[("Red", "Choose Red."), ("Blue", "Choose Blue.")],
                ),
            ]),
        ]
        .concat()
    );
    assert_eq!(
        send(&link, answer(session, 1, &["Blue"])).await,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await,
        [
            answered_blue(),
            delta("Blue"),
            reply("Blue"),
            turn_end(TurnOutcome::Completed),
        ]
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// Approving for the session adds the CLI's suggested rule for the session
/// only, so the same command runs again without a request.
#[tokio::test]
async fn a_claude_call_approved_for_the_session_runs_again_without_asking() {
    let fx = host().await;
    let dir = scratch("for-session", "claude-for-session.jsonl");
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(send(&link, start(Provider::Claude, &dir)).await);
    let _ = send(&link, message(session, "touch t1.txt")).await;
    let (first, again) = (
        "toolu_bdrk_0194hqw8EXmFKbEqNMeNMkWf",
        "toolu_bdrk_015276Epur1cWgLdAb2EXFPF",
    );
    let touch = ToolKind::Command {
        command: String::from("touch t1.txt"),
    };
    assert_eq!(
        kinds_until(&mut updates, Until::Request).await,
        [
            opening(Provider::Claude, &dir),
            Vec::from([
                user("touch t1.txt"),
                started(first, &touch),
                EventKind::RequestOpened {
                    request: RequestId(1),
                    call: ToolCallId(String::from(first)),
                    tool: touch.clone(),
                    reason: None,
                    for_session: Some(String::from("Bash(touch t1.txt)")),
                },
            ]),
        ]
        .concat()
    );
    assert_eq!(
        send(&link, resolve(session, 1, Decision::ApproveForSession)).await,
        CommandResult::Accepted
    );
    let first_turn = kinds_until(&mut updates, Until::TurnEnd).await;
    let _ = send(&link, message(session, "again")).await;
    let second_turn = kinds_until(&mut updates, Until::TurnEnd).await;
    assert_eq!(
        (
            calls_and_requests(first_turn),
            calls_and_requests(second_turn)
        ),
        (
            Vec::from([
                EventKind::RequestResolved {
                    request: RequestId(1),
                    resolution: Resolution::Decided {
                        decision: Decision::ApproveForSession,
                        by: test_client(),
                    },
                },
                finished(first, ToolOutcome::Succeeded),
                turn_end(TurnOutcome::Completed),
            ]),
            Vec::from([
                user("again"),
                started(again, &touch),
                finished(again, ToolOutcome::Succeeded),
                turn_end(TurnOutcome::Completed),
            ]),
        )
    );
    assert_eq!(
        sent_to_agent(&dir)
            .matches(r#""updatedPermissions":[{"behavior":"allow","destination":"session","rules":[{"ruleContent":"touch t1.txt","toolName":"Bash"}],"type":"addRules"}]"#)
            .count(),
        1
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// Codex takes `acceptForSession` for a command, which the stand-in waits
/// for, and runs the same command again without a request.
#[tokio::test]
async fn a_codex_command_accepted_for_the_session_runs_again_without_asking() {
    let (fx, dir) = codex_host("codex-for-session.jsonl").await;
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(send(&link, start(Provider::Codex, &dir)).await);
    let _ = send(&link, message(session, "touch t1.txt")).await;
    let command = "/usr/bin/zsh -c 'touch t1.txt'";
    let (first, again) = (
        "exec-6a70037b-22b5-482b-955f-e442841e7e21",
        "exec-0e77cebc-6940-4892-b90c-558be7ccb2b4",
    );
    let touch = ToolKind::Command {
        command: String::from(command),
    };
    let asked = calls_and_requests(kinds_until(&mut updates, Until::Request).await);
    assert_eq!(
        send(&link, resolve(session, 1, Decision::ApproveForSession)).await,
        CommandResult::Accepted
    );
    let first_turn = kinds_until(&mut updates, Until::TurnEnd).await;
    let _ = send(&link, message(session, "again")).await;
    let second_turn = kinds_until(&mut updates, Until::TurnEnd).await;
    assert_eq!(
        (
            asked,
            calls_and_requests(first_turn),
            calls_and_requests(second_turn)
        ),
        (
            [
                opening(Provider::Codex, &dir),
                Vec::from([
                    user("touch t1.txt"),
                    started(first, &touch),
                    EventKind::RequestOpened {
                        request: RequestId(1),
                        call: ToolCallId(String::from(first)),
                        tool: touch.clone(),
                        reason: None,
                        for_session: Some(String::from(command)),
                    },
                ]),
            ]
            .concat(),
            Vec::from([
                EventKind::RequestResolved {
                    request: RequestId(1),
                    resolution: Resolution::Decided {
                        decision: Decision::ApproveForSession,
                        by: test_client(),
                    },
                },
                finished(first, ToolOutcome::Succeeded),
                turn_end(TurnOutcome::Completed),
            ]),
            Vec::from([
                user("again"),
                started(again, &touch),
                finished(again, ToolOutcome::Succeeded),
                turn_end(TurnOutcome::Completed),
            ]),
        )
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// An interrupt withdraws an open question, as it does an approval: Claude
/// cancels it, Codex withdraws it after the turn ends, and either way the
/// view shows it withdrawn and a late answer is refused.
#[tokio::test]
async fn an_interrupt_withdraws_an_open_question() {
    let fx = host().await;
    let dir = scratch("question-withdraw", "claude-question-withdraw.jsonl");
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(send(&link, start(Provider::Claude, &dir)).await);
    let _ = send(&link, message(session, "ask me")).await;
    let mut events = legacy_events_until(&mut updates, Until::Question).await;
    assert_eq!(
        send(&link, interrupt(session)).await,
        CommandResult::Accepted
    );
    events.extend(legacy_events_until(&mut updates, Until::TurnEnd).await);
    assert_eq!(
        events
            .iter()
            .skip(3)
            .map(|event| event.kind.clone())
            .collect::<Vec<EventKind>>(),
        [
            asked(
                "Fruit",
                "Which fruit do you prefer?",
                &[
                    ("Apple", "Choose apple as your preferred fruit."),
                    ("Pear", "Choose pear as your preferred fruit."),
                ],
            ),
            EventKind::RequestResolved {
                request: RequestId(1),
                resolution: Resolution::Withdrawn,
            },
            finished("toolu_bdrk_01Ctjtd63ZgJ5uKQpWhcgJUa", ToolOutcome::Failed),
            turn_end(TurnOutcome::Interrupted),
        ]
    );
    assert_eq!(
        send(&link, answer(session, 1, &["Pear"])).await,
        CommandResult::Rejected(Rejection::UnknownRequest)
    );
    let questions = items(&fold(events), session)
        .into_iter()
        .filter_map(|item| match item {
            Item::Question { state, .. } => Some(state),
            Item::User { .. }
            | Item::Reply { .. }
            | Item::Reasoning { .. }
            | Item::Tool { .. }
            | Item::TurnCompleted(_)
            | Item::Continued(_)
            | Item::ModelChanged(_)
            | Item::Error(_) => None,
        })
        .collect::<Vec<RequestState>>();
    assert_eq!(questions, [RequestState::Resolved(Resolution::Withdrawn)]);
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_codex_interrupt_withdraws_the_open_question_at_the_turn_end() {
    let (fx, dir) = codex_host("codex-question-withdraw.jsonl").await;
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(send(&link, start(Provider::Codex, &dir)).await);
    let _ = send(&link, message(session, "ask me")).await;
    let question = kinds_until(&mut updates, Until::Question).await;
    assert_eq!(
        send(&link, interrupt(session)).await,
        CommandResult::Accepted
    );
    assert_eq!(
        (
            question.last().cloned(),
            kinds_until(&mut updates, Until::Resolved).await
        ),
        (
            Some(asked(
                "Fruit",
                "Which fruit do you prefer?",
                &[("Apple", "Choose Apple."), ("Pear", "Choose Pear.")],
            )),
            Vec::from([
                turn_end(TurnOutcome::Interrupted),
                EventKind::RequestResolved {
                    request: RequestId(1),
                    resolution: Resolution::Withdrawn,
                },
            ]),
        )
    );
    assert_eq!(
        send(&link, answer(session, 1, &["Pear"])).await,
        CommandResult::Rejected(Rejection::UnknownRequest)
    );
    std::fs::write(dir.join("release"), "").unwrap();
    assert_eq!(sent_to_agent(&dir).matches("\"answers\"").count(), 0);
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
    let session = started_session(send(&link, start(Provider::Claude, &dir)).await);
    assert_eq!(
        send(&link, message(session, "hi")).await,
        CommandResult::Accepted
    );
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
        send(&link, interrupt(session)).await,
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
        send(&link, message(session, "hi")).await,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_missing_claude_binary_closes_the_session() {
    let fx = host_with(PathBuf::from("/nonexistent/claude")).await;
    let dir = scratch("missing-binary", "claude-approve.jsonl");
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(send(&link, start(Provider::Claude, &dir)).await);
    assert_eq!(
        kinds_until(&mut updates, Until::Closed).await,
        [
            EventKind::SessionCreated {
                provider: Provider::Claude,
                directory: HostPath(dir.display().to_string()),
                by: test_client(),
                model: Some(ModelId(String::from("big"))),
                effort: None,
            },
            EventKind::SessionClosed {
                reason: CloseReason::FailedToStart {
                    message: String::from("claude i/o: No such file or directory (os error 2)"),
                },
            },
        ]
    );
    assert_eq!(
        send(&link, message(session, "hi")).await,
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
    let first_session = started_session(send(&link, start(Provider::Codex, &dir)).await);
    // The second session starts once the first has its thread, so the first
    // gets the recording's first thread.
    let first = legacy_events_until(&mut updates, Until::Ready).await;
    let second_session = started_session(send(&link, start(Provider::Codex, &dir)).await);
    assert_ne!(first_session, second_session);
    let second = legacy_events_until(&mut updates, Until::Ready).await;
    assert_eq!(
        [
            send(&link, message(first_session, "create a.txt")).await,
            send(&link, message(second_session, "create b.txt")).await,
        ],
        [CommandResult::Accepted, CommandResult::Accepted]
    );
    let asked = [
        legacy_events_until(&mut updates, Until::Request).await,
        legacy_events_until(&mut updates, Until::Request).await,
    ]
    .concat();
    assert_eq!(
        [
            send(&link, resolve(first_session, 1, Decision::Approve)).await,
            send(&link, resolve(second_session, 1, Decision::Deny)).await,
        ],
        [CommandResult::Accepted, CommandResult::Accepted]
    );
    let ended = [
        legacy_events_until(&mut updates, Until::TurnEnd).await,
        legacy_events_until(&mut updates, Until::TurnEnd).await,
    ]
    .concat();
    let view = fold([first, second, asked, ended].concat());
    let a_call = "exec-f4c7c6d4-8255-4bcc-972d-9f106cda7991";
    assert_eq!(
        items(&view, first_session),
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
        items(&view, second_session),
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
    let session = started_session(send(&link, start(Provider::Codex, &dir)).await);
    let _ = send(&link, message(session, "count to 300")).await;
    assert_eq!(
        kinds_until(&mut updates, Until::Delta).await,
        [
            opening(Provider::Codex, &dir),
            Vec::from([user("count to 300"), delta("1")]),
        ]
        .concat()
    );
    assert_eq!(
        send(&link, interrupt(session)).await,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await,
        [delta("\n"), turn_end(TurnOutcome::Interrupted)]
    );
    let _ = send(&link, message(session, "ping")).await;
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
    let session = started_session(send(&link, start(Provider::Codex, &dir)).await);
    let _ = send(&link, message(session, "create d.txt")).await;
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
                    for_session: Some(String::from("/usr/bin/zsh -lc 'printf %s d > d.txt'")),
                },
            ]),
        ]
        .concat()
    );
    assert_eq!(
        send(&link, interrupt(session)).await,
        CommandResult::Accepted
    );
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
        send(&link, resolve(session, 1, Decision::Approve)).await,
        CommandResult::Rejected(Rejection::UnknownRequest)
    );
    std::fs::write(dir.join("release"), "").unwrap();
    // Codex's withdrawal plays before the next turn and is ignored.
    let _ = send(&link, message(session, "ping")).await;
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
    let first_session = started_session(send(&link, start(Provider::Codex, &dir)).await);
    let first = legacy_events_until(&mut updates, Until::Ready).await;
    let second_session = started_session(send(&link, start(Provider::Codex, &dir)).await);
    assert_ne!(first_session, second_session);
    let seen = [
        first,
        legacy_events_until(&mut updates, Until::Closed).await,
        legacy_events_until(&mut updates, Until::Closed).await,
    ]
    .concat();
    let closed = EventKind::SessionClosed {
        reason: CloseReason::Exited {
            status: String::from("exit status: 0"),
        },
    };
    assert_eq!(
        (of(first_session, &seen), of(second_session, &seen)),
        (
            [opening(Provider::Codex, &dir), Vec::from([closed.clone()])].concat(),
            [opening(Provider::Codex, &dir), Vec::from([closed])].concat(),
        )
    );
    assert_eq!(
        send(&link, message(first_session, "hi")).await,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    let next_session = started_session(send(&link, start(Provider::Codex, &dir)).await);
    assert_ne!(next_session, first_session);
    assert_ne!(next_session, second_session);
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

    // No deadline: only the hedge can take the link live, and STEP bounds each update.
    let before_live = until_live(&mut updates).await;
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

/// Sends this thread's logs, at debug and above, to a file named after
/// `test`. A `tokio::test` runs on one thread, so they include the host's.
fn capture_log(test: &str) -> (PathBuf, tracing::subscriber::DefaultGuard) {
    let path = target_tmp().join(format!("conductor-e2e-{test}-{}.log", std::process::id()));
    let log = std::fs::File::create(&path).unwrap();
    let guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_writer(Arc::new(log))
            .with_max_level(tracing::Level::DEBUG)
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

const GATEWAY: &str = "https://conductor-test.invalid/gateway";
const CLAUDE_TOKEN: &str = "conductor-test-claude-token";
const NO_CREDENTIAL: &str = "no credential configured";
const UNPROTECTED: &str =
    "Claude's commands can read the gateway token; install bubblewrap and socat to sandbox them";

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// How many times `text` is in the file at `path`.
fn count(path: &Path, text: &str) -> usize {
    std::fs::read(path)
        .unwrap()
        .windows(text.len())
        .filter(|window| window == &text.as_bytes())
        .count()
}

/// A host on `data` whose Claude is the fake in a private home, with no
/// sign-in of its own, and whose Codex is unavailable.
async fn claude_host_in(data: &Path) -> Fixture {
    let homes = daemon::homes(data).unwrap();
    std::fs::create_dir_all(data.join("probe")).unwrap();
    std::fs::write(data.join("probe/stdout.jsonl"), format!("{PROBED}\n")).unwrap();
    let launchers = Launchers {
        claude: Launcher::Ready {
            launch: Launch::Claude(ClaudeLaunch {
                claude_path: fake_claude(),
                home: homes.claude,
                credential: None,
            }),
            own: OwnSignIn::Missing,
            models: models(),
            chosen: None,
        },
        codex: Launcher::Unavailable(String::from(UNAVAILABLE)),
    };
    serve(endpoint().await, launchers, data).await
}

fn with_claude(claude: Availability) -> Providers {
    Providers {
        claude,
        codex: Availability::Unavailable {
            reason: String::from(UNAVAILABLE),
        },
    }
}

fn gateway(models: Models) -> Availability {
    Availability::Available {
        auth: Auth::Gateway {
            base_url: String::from(GATEWAY),
        },
        models,
    }
}

/// What the Claude stand-in lists in the probe directory, which differs
/// from what the host started with, so a test sees the list was read again.
const PROBED: &str = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_1","response":{"claude_code_version":"2.1.316","models":[{"value":"served","displayName":"Served","description":"by the gateway","supportedEffortLevels":["high"]}]}}}"#;

fn probed() -> Models {
    Models {
        choices: Vec::from([ModelChoice {
            id: ModelId(String::from("served")),
            name: String::from("Served"),
            description: String::from("by the gateway"),
            efforts: Vec::from([Effort::High]),
        }]),
        default: None,
    }
}

fn no_credential() -> Availability {
    Availability::Unavailable {
        reason: String::from(NO_CREDENTIAL),
    }
}

fn refused_for_no_credential() -> CommandResult {
    CommandResult::Rejected(Rejection::ProviderUnavailable {
        reason: String::from(NO_CREDENTIAL),
    })
}

fn claude_gateway() -> CommandBody {
    CommandBody::SetCredential {
        credential: Credential::Claude(ClaudeCredential::Gateway {
            base_url: String::from(GATEWAY),
            token: String::from(CLAUDE_TOKEN),
        }),
    }
}

/// A fresh link to `fx`, live, and the providers its `Welcome` reported.
async fn live(fx: &Fixture) -> (HostLink, mpsc::Receiver<Update>, Option<Providers>) {
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let providers = link.status().providers;
    (link, updates, providers)
}

#[tokio::test]
async fn a_claude_gateway_reaches_the_cli_survives_a_restart_and_clears() {
    let data = data_dir("claude-gateway");
    let (log, logging) = capture_log("claude-gateway");
    let dir = scratch("claude-gateway", "claude-approve.jsonl");
    let fx = claude_host_in(&data).await;
    let (link, mut updates, welcomed) = live(&fx).await;
    assert_eq!(welcomed, Some(with_claude(no_credential())));
    assert_eq!(
        send(&link, start(Provider::Claude, &dir)).await,
        refused_for_no_credential()
    );
    assert_eq!(
        send(&link, claude_gateway()).await,
        CommandResult::Providers(with_claude(gateway(probed())))
    );
    assert_eq!(
        (
            link.status().providers,
            read(&data.join("probe/env.txt")).contains(GATEWAY)
        ),
        (Some(with_claude(gateway(probed()))), true),
        "the models are listed again, with the new credential"
    );
    let _ = started_session(send(&link, start(Provider::Claude, &dir)).await);
    let events = legacy_events_until(&mut updates, Until::Ready).await;
    assert_eq!(
        read(&dir.join("env.txt")),
        format!(
            "ANTHROPIC_AUTH_TOKEN={CLAUDE_TOKEN}\nANTHROPIC_BASE_URL={GATEWAY}\n{}CLAUDE_CONFIG_DIR={}\n",
            match agents::claude::sandbox() {
                Sandbox::Available => "CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1\n",
                Sandbox::Missing => "",
            },
            data.join("home/claude").display()
        )
    );
    assert_eq!(count(&data.join("credentials"), CLAUDE_TOKEN), 0);
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();

    let fx = claude_host_in(&data).await;
    let (link, _updates, welcomed) = live(&fx).await;
    assert_eq!(welcomed, Some(with_claude(gateway(models()))));
    assert_eq!(
        send(
            &link,
            CommandBody::ClearCredential {
                provider: Provider::Claude
            }
        )
        .await,
        CommandResult::Providers(with_claude(no_credential()))
    );
    assert_eq!(
        send(&link, start(Provider::Claude, &dir)).await,
        refused_for_no_credential()
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();

    let fx = claude_host_in(&data).await;
    let (link, _updates, welcomed) = live(&fx).await;
    assert_eq!(welcomed, Some(with_claude(no_credential())));
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
    drop(logging);
    let logged = captured(&log);
    assert_eq!(
        (
            logged.matches("stored a credential change").count(),
            logged.matches(CLAUDE_TOKEN).count(),
            format!("{events:?}").matches(CLAUDE_TOKEN).count(),
            logged.matches(UNPROTECTED).count(),
        ),
        (
            2,
            0,
            0,
            // Once when the gateway is set, once when the restart finds it.
            match agents::claude::sandbox() {
                Sandbox::Available => 0,
                Sandbox::Missing => 2,
            }
        )
    );
}

fn with_codex(codex: Availability) -> Providers {
    Providers {
        claude: Availability::Unavailable {
            reason: String::from(UNAVAILABLE),
        },
        codex,
    }
}

/// How Codex signs in and the models it offers, by id, with the default
/// first, from a host's answer to a credential change.
fn codex_offer(result: CommandResult) -> Option<(Auth, Option<String>, Vec<String>)> {
    match result {
        CommandResult::Providers(Providers {
            codex: Availability::Available { auth, models },
            ..
        }) => Some((
            auth,
            models.default.map(|model| model.0),
            models
                .choices
                .into_iter()
                .map(|choice| choice.id.0)
                .collect(),
        )),
        _ => None,
    }
}

const LISTED: &[&str] = &[
    "gpt-6.1-sol",
    "gpt-6-astra",
    "gpt-6-sol",
    "gpt-6-luna",
    "gpt-5.6-sol",
    "gpt-6.1-terra",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
];

/// What `codex-*-listed.jsonl` lists, with a Codex provider credential's
/// model as the default (`Registry::models`).
fn codex_gateway() -> Option<(Auth, Option<String>, Vec<String>)> {
    Some((
        Auth::Gateway {
            base_url: String::from(GATEWAY),
        },
        Some(String::from("conductor-test-model")),
        LISTED.iter().copied().map(String::from).collect(),
    ))
}

fn codex_provider(key: &str) -> CommandBody {
    CommandBody::SetCredential {
        credential: Credential::Codex(CodexCredential::Provider {
            name: String::from("Conductor test"),
            base_url: String::from(GATEWAY),
            key: String::from(key),
            model: String::from("conductor-test-model"),
        }),
    }
}

#[tokio::test]
async fn a_codex_provider_reaches_the_app_server_and_a_new_one_restarts_it() {
    const FIRST_KEY: &str = "conductor-test-first-key";
    const SECOND_KEY: &str = "conductor-test-second-key";
    let data = data_dir("codex-provider");
    let (log, logging) = capture_log("codex-provider");
    let server = scratch("codex-provider", "codex-two-threads-listed.jsonl");
    let homes = daemon::homes(&data).unwrap();
    let hub = CodexHub::spawn(ServerOptions {
        codex_path: fake_codex(),
        home: homes.codex,
        credential: None,
        cwd: server.clone(),
        stdout_log: None,
    });
    let launchers = Launchers {
        claude: Launcher::Unavailable(String::from(UNAVAILABLE)),
        codex: Launcher::Ready {
            launch: Launch::Codex(CodexLaunch { hub }),
            own: OwnSignIn::Missing,
            models: models(),
            chosen: None,
        },
    };
    let fx = serve(endpoint().await, launchers, &data).await;
    let (link, mut updates, welcomed) = live(&fx).await;
    assert_eq!(welcomed, Some(with_codex(no_credential())));
    assert_eq!(
        codex_offer(send(&link, codex_provider(FIRST_KEY)).await),
        codex_gateway()
    );
    let _ = started_session(send(&link, start(Provider::Codex, &server)).await);
    let first = legacy_events_until(&mut updates, Until::Ready).await;
    let home = data.join("home/codex");
    assert_eq!(
        read(&server.join("env.txt")),
        format!(
            "CODEX_HOME={}\nCONDUCTOR_CODEX_KEY={FIRST_KEY}\n",
            home.display()
        )
    );
    assert_eq!(
        read(&server.join("codex-config.toml")),
        format!(
            "check_for_update_on_startup = false\nmodel_provider = \"conductor\"\nmodel = \"conductor-test-model\"\n\n[model_providers.conductor]\nname = \"Conductor test\"\nbase_url = \"{GATEWAY}\"\nenv_key = \"CONDUCTOR_CODEX_KEY\"\nwire_api = \"responses\"\n\n[shell_environment_policy.filters]\nCONDUCTOR_CODEX_KEY = \"exclude\"\n"
        )
    );
    assert_eq!(
        codex_offer(send(&link, codex_provider(SECOND_KEY)).await),
        codex_gateway()
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Closed).await,
        [EventKind::SessionClosed {
            reason: CloseReason::Stopped {
                reason: String::from("new Codex credential"),
            },
        }]
    );
    let _ = started_session(send(&link, start(Provider::Codex, &server)).await);
    let second = legacy_events_until(&mut updates, Until::Ready).await;
    assert_eq!(
        read(&server.join("env.txt")),
        format!(
            "CODEX_HOME={}\nCONDUCTOR_CODEX_KEY={SECOND_KEY}\n",
            home.display()
        )
    );
    assert_eq!(count(&home.join("config.toml"), FIRST_KEY), 0);
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
    drop(logging);
    let logged = captured(&log);
    assert_eq!(
        (
            logged
                .matches("stopped the codex app-server for a new credential")
                .count(),
            logged.matches(FIRST_KEY).count(),
            logged.matches(SECOND_KEY).count(),
            format!("{first:?}{second:?}").matches("-key").count(),
        ),
        (1, 0, 0, 0)
    );
}

#[tokio::test]
async fn a_credential_sent_before_hello_is_refused_and_not_stored() {
    let data = data_dir("unauthenticated");
    let fx = claude_host_in(&data).await;
    let endpoint = endpoint().await;
    let conn = endpoint
        .connect(fx.target.addr.clone(), proto::ALPN)
        .await
        .unwrap();
    let (mut send_half, mut recv_half) = conn.open_bi().await.unwrap();
    proto::wire::write(
        &mut send_half,
        &proto::Open::Command(proto::Command {
            id: proto::CommandId(1),
            body: claude_gateway(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        proto::wire::read::<proto::HelloReply>(&mut recv_half)
            .await
            .unwrap(),
        Some(proto::HelloReply::Refused {
            reason: String::from("the first stream must be a hello"),
        })
    );
    endpoint.close().await;
    let (link, _updates, welcomed) = live(&fx).await;
    assert_eq!(
        (welcomed, data.join("credentials").exists()),
        (Some(with_claude(no_credential())), false)
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_unreadable_store_keeps_the_host_up_refuses_a_set_and_clears_aside() {
    let data = data_dir("unreadable");
    // A store key of its own, then a store that doesn't open with it.
    let _ = Store::open(&data).unwrap();
    std::fs::write(data.join("credentials"), b"not a sealed store").unwrap();
    let fx = claude_host_in(&data).await;
    let (link, _updates, welcomed) = live(&fx).await;
    let unreadable = Availability::Unavailable {
        reason: format!(
            "{} does not open with the store key; clearing a credential moves it aside",
            data.join("credentials").display()
        ),
    };
    assert_eq!(
        welcomed,
        Some(with_claude(unreadable.clone())),
        "a stored credential may be in it, so the home's sign-in is not used"
    );
    let refused = CommandResult::Rejected(Rejection::CredentialNotSaved {
        reason: format!(
            "{} does not open with the store key; clearing a credential moves it aside",
            data.join("credentials").display()
        ),
    });
    assert_eq!(send(&link, claude_gateway()).await, refused);
    assert_eq!(
        read(&data.join("credentials")),
        "not a sealed store",
        "a refused set leaves the file as it was"
    );
    assert_eq!(
        send(
            &link,
            CommandBody::ClearCredential {
                provider: Provider::Codex
            }
        )
        .await,
        CommandResult::Providers(with_claude(no_credential()))
    );
    assert_eq!(
        read(&data.join("credentials.unreadable")),
        "not a sealed store"
    );
    assert_eq!(
        send(&link, claude_gateway()).await,
        CommandResult::Providers(with_claude(gateway(probed())))
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_base_url_that_could_hide_a_secret_is_refused() {
    let data = data_dir("base-url");
    let fx = claude_host_in(&data).await;
    let (link, _updates, _) = live(&fx).await;
    let body = CommandBody::SetCredential {
        credential: Credential::Claude(ClaudeCredential::Gateway {
            base_url: String::from("https://gateway.example/?key=conductor-test"),
            token: String::from(CLAUDE_TOKEN),
        }),
    };
    assert_eq!(
        (send(&link, body).await, data.join("credentials").exists()),
        (
            CommandResult::Rejected(Rejection::InvalidCredential {
                reason: String::from(
                    "the base URL may not carry a user, a password, a query or a fragment"
                ),
            }),
            false
        )
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

fn start_with(provider: Provider, directory: &Path, model: &str, effort: Effort) -> CommandBody {
    CommandBody::StartSession {
        provider,
        directory: HostPath(directory.display().to_string()),
        model: Some(ModelId(String::from(model))),
        effort: Some(effort),
    }
}

fn continued(session: SessionId) -> CommandBody {
    CommandBody::Continue { session }
}

/// The arguments of each run of the Claude stand-in in `dir`.
fn argv(dir: &Path) -> Vec<Vec<String>> {
    read(&dir.join("argv.txt"))
        .split("run\n")
        .filter(|run| !run.is_empty())
        .map(|run| run.lines().map(String::from).collect())
        .collect()
}

/// The value after `flag` in one run's arguments.
fn flag(run: &[String], flag: &str) -> Option<String> {
    run.iter()
        .position(|arg| arg == flag)
        .and_then(|at| run.get(at.saturating_add(1)))
        .cloned()
}

/// The params of each request named `method` the stand-in in `dir` read in
/// `log`.
fn requests(dir: &Path, log: &str, method: &str) -> Vec<serde_json::Value> {
    read(&dir.join(log))
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|request| request.get("method").and_then(|m| m.as_str()) == Some(method))
        .filter_map(|request| request.get("params").cloned())
        .collect()
}

fn created_with(provider: Provider, directory: &Path, model: &str, effort: Effort) -> EventKind {
    EventKind::SessionCreated {
        provider,
        directory: HostPath(directory.display().to_string()),
        by: test_client(),
        model: Some(ModelId(String::from(model))),
        effort: Some(effort),
    }
}

/// A model the host doesn't list, or an effort its model doesn't take, is
/// refused; a chosen one is logged and reaches the CLI.
#[tokio::test]
async fn start_session_takes_only_a_model_and_effort_the_host_offers() {
    let fx = host().await;
    let dir = scratch("chosen", "claude-approve.jsonl");
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    assert_eq!(
        [
            send(
                &link,
                start_with(Provider::Claude, &dir, "huge", Effort::Low)
            )
            .await,
            send(
                &link,
                start_with(Provider::Claude, &dir, "small", Effort::High)
            )
            .await,
        ],
        [
            CommandResult::Rejected(Rejection::NotOffered {
                reason: String::from("this host doesn't offer the model huge"),
            }),
            CommandResult::Rejected(Rejection::NotOffered {
                reason: String::from("Small doesn't take the effort high"),
            }),
        ]
    );
    let _ = started_session(
        send(
            &link,
            start_with(Provider::Claude, &dir, "small", Effort::Low),
        )
        .await,
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Ready).await,
        [
            created_with(Provider::Claude, &dir, "small", Effort::Low),
            EventKind::SessionReady,
        ]
    );
    let runs = argv(&dir);
    assert_eq!(
        runs.iter()
            .map(|run| (flag(run, "--model"), flag(run, "--effort")))
            .collect::<Vec<(Option<String>, Option<String>)>>(),
        [(Some(String::from("small")), Some(String::from("low")))]
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

const THOUGHT: &str = "\"All but 9 run away\" means 9 stay, so he buys double that, 18, bringing the total to 27, then sells a third (9), leaving 18 sheep.\n\n";

/// Reasoning streams to live clients and is stored once finished, so a
/// client that catches up later sees the finished text without deltas.
/// The CLI sends this traffic only with betas on, which the owner's
/// gateway refuses.
#[tokio::test]
async fn claude_reasoning_streams_live_and_is_stored_once_finished() {
    let fx = host().await;
    let dir = scratch("reasoning", "claude-reasoning.jsonl");
    let (phone, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(send(&phone, start(Provider::Claude, &dir)).await);
    let _ = send(&phone, message(session, "sheep?")).await;
    let live = kinds_until(&mut updates, Until::TurnEnd).await;
    let streamed = live
        .iter()
        .filter_map(|kind| match kind {
            EventKind::ReasoningDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    let reasoning = EventKind::Reasoning {
        text: String::from(THOUGHT),
    };
    let stored = [
        opening(Provider::Claude, &dir),
        Vec::from([
            user("sheep?"),
            reasoning,
            reply("18"),
            turn_end(TurnOutcome::Completed),
        ]),
    ]
    .concat();
    assert_eq!(
        (
            streamed,
            live.into_iter()
                .filter(|kind| !matches!(
                    kind,
                    EventKind::ReasoningDelta { .. } | EventKind::AssistantDelta { .. }
                ))
                .collect::<Vec<EventKind>>()
        ),
        (String::from(THOUGHT), stored.clone())
    );
    let (late, mut late_updates) = link(fx.target.clone(), None).await;
    let caught_up = until_live(&mut late_updates)
        .await
        .into_iter()
        .filter_map(|update| match update {
            Update::Event(event) => Some(event),
            Update::Status(_) | Update::Reset(_) => None,
        })
        .collect::<Vec<Event>>();
    assert_eq!(
        caught_up
            .iter()
            .map(|event| event.kind.clone())
            .filter(|kind| !matches!(
                kind,
                EventKind::WorkspaceCreated { .. }
                    | EventKind::CheckpointCreated { .. }
                    | EventKind::CheckpointReserved { .. }
                    | EventKind::NativeTurnStarted { .. }
                    | EventKind::HistoryMaterialized { .. }
            ))
            .collect::<Vec<EventKind>>(),
        stored
    );
    assert_eq!(
        items(&fold(caught_up), session)
            .into_iter()
            .filter(|item| matches!(item, Item::Reasoning { .. }))
            .collect::<Vec<Item>>(),
        [Item::Reasoning {
            text: String::from(THOUGHT),
            state: ReplyState::Done,
        }]
    );
    late.stop().await.unwrap();
    phone.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// Through the owner's gateway, with betas off, the CLI streams an empty
/// thinking block (`claude-killed.jsonl`'s first turn), which shows nothing.
#[tokio::test]
async fn empty_gateway_thinking_logs_no_reasoning_and_folds_to_no_disclosure() {
    let fx = host().await;
    let dir = scratch("betas-off", "claude-killed.jsonl");
    let (phone, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(send(&phone, start(Provider::Claude, &dir)).await);
    let _ = send(&phone, message(session, "sheep?")).await;
    let turn = legacy_events_until(&mut updates, Until::TurnEnd).await;
    assert_eq!(
        (
            turn.iter()
                .filter(|event| matches!(
                    event.kind,
                    EventKind::Reasoning { .. } | EventKind::ReasoningDelta { .. }
                ))
                .count(),
            items(&fold(turn), session)
                .into_iter()
                .filter(|item| matches!(item, Item::Reasoning { .. } | Item::Reply { .. }))
                .collect::<Vec<Item>>(),
        ),
        (
            0,
            Vec::from([Item::Reply {
                text: String::from("18"),
                state: ReplyState::Done,
            }])
        )
    );
    phone.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// A Claude CLI killed mid-turn closes the session; `Continue` starts it
/// again with `--resume` on the same conversation, model and effort, and
/// the next turn runs.
#[tokio::test]
async fn a_killed_claude_session_continues_its_conversation() {
    let fx = host().await;
    let dir = scratch("killed", "claude-killed.jsonl");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/e2e/claude-resumed.jsonl"),
        dir.join("stdout.2.jsonl"),
    )
    .unwrap();
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    assert_eq!(
        send(&link, continued(SessionId(1))).await,
        CommandResult::Rejected(Rejection::UnknownSession)
    );
    let session = started_session(
        send(
            &link,
            start_with(Provider::Claude, &dir, "small", Effort::Low),
        )
        .await,
    );
    let _ = send(&link, message(session, "sheep?")).await;
    let first = legacy_events_until(&mut updates, Until::TurnEnd).await;
    assert_eq!(
        send(&link, continued(session)).await,
        CommandResult::Accepted
    );
    let _ = send(&link, message(session, "count")).await;
    let killed = legacy_events_until(&mut updates, Until::Closed).await;
    assert_eq!(
        killed
            .iter()
            .map(|event| event.kind.clone())
            .collect::<Vec<EventKind>>(),
        [
            user("count"),
            EventKind::SessionClosed {
                reason: CloseReason::Exited {
                    status: String::from("signal: 9 (SIGKILL)"),
                },
            },
        ]
    );
    assert_eq!(
        send(&link, message(session, "hello?")).await,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    assert_eq!(
        send(&link, continued(session)).await,
        CommandResult::Accepted
    );
    let ready = legacy_events_until(&mut updates, Until::Ready).await;
    let _ = send(&link, message(session, "how many sheep?")).await;
    let after = legacy_events_until(&mut updates, Until::TurnEnd).await;
    assert_eq!(
        after
            .iter()
            .map(|event| event.kind.clone())
            .filter(|kind| !matches!(kind, EventKind::AssistantDelta { .. }))
            .collect::<Vec<EventKind>>(),
        [
            user("how many sheep?"),
            reply("18"),
            turn_end(TurnOutcome::Completed),
        ]
    );
    let runs = argv(&dir);
    assert_eq!(
        runs.iter()
            .map(|run| (
                flag(run, "--model"),
                flag(run, "--effort"),
                run.iter().find(|arg| arg.starts_with("--resume=")).cloned()
            ))
            .collect::<Vec<(Option<String>, Option<String>, Option<String>)>>(),
        [
            (Some(String::from("small")), Some(String::from("low")), None),
            (
                Some(String::from("small")),
                Some(String::from("low")),
                runs.first()
                    .and_then(|run| flag(run, "--session-id"))
                    .map(|id| format!("--resume={id}"))
            ),
        ]
    );
    let view = fold([first, killed, ready, after].concat());
    assert_eq!(
        items(&view, session)
            .into_iter()
            .filter(|item| matches!(item, Item::Continued(_)))
            .collect::<Vec<Item>>(),
        [Item::Continued(CloseReason::Exited {
            status: String::from("signal: 9 (SIGKILL)"),
        })]
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// A `Continue` whose transcript is gone leaves the session closed, with
/// the CLI's own reason (the recorded `result` of the pinned CLI).
#[tokio::test]
async fn a_continue_with_no_transcript_stays_closed_with_the_clis_reason() {
    let fx = host().await;
    let dir = scratch("resume-missing", "claude-killed.jsonl");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/claude-resume-missing.jsonl"),
        dir.join("stdout.2.jsonl"),
    )
    .unwrap();
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(send(&link, start(Provider::Claude, &dir)).await);
    let _ = send(&link, message(session, "sheep?")).await;
    let _ = legacy_events_until(&mut updates, Until::TurnEnd).await;
    let _ = send(&link, message(session, "count")).await;
    let _ = legacy_events_until(&mut updates, Until::Closed).await;
    assert_eq!(
        send(&link, continued(session)).await,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Closed).await,
        [EventKind::SessionClosed {
            reason: CloseReason::FailedToStart {
                message: String::from(
                    "claude exited: No conversation found with session ID: 5b0b7a52-1c9e-4d1e-9a4e-3f2f8c1d6a10"
                ),
            },
        }]
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

const COUNTING: &str = "**Compiling numbers**\n\nI'm tasked with creating a list of 300 numbers, one per line. It sounds massive but straightforward—I'll ensure each number is on its own line. I need to focus on accuracy while keeping the format neat. There are around 600 tokens to work with overall. Luckily, it seems I won’t need any tools for this task; I just need to produce the exact output as required. Let's get started on this!";

/// The app-server killed mid-turn, as live: after `turn/started`, with
/// deltas streamed. The session closes with no failed turn, and `Continue`
/// resumes the thread.
#[tokio::test]
async fn a_codex_server_killed_mid_turn_closes_and_continues() {
    let (fx, server) = codex_host("codex-killed-mid-turn.jsonl").await;
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/e2e/codex-resumed.jsonl"),
        server.join("stdout.2.jsonl"),
    )
    .unwrap();
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(
        send(
            &link,
            start_with(Provider::Codex, &server, "small", Effort::Low),
        )
        .await,
    );
    let _ = send(&link, message(session, "prime?")).await;
    let _ = kinds_until(&mut updates, Until::TurnEnd).await;
    let _ = send(&link, message(session, COUNT)).await;
    let killed = kinds_until(&mut updates, Until::Closed).await;
    assert_eq!(
        (
            killed
                .iter()
                .filter(|kind| matches!(kind, EventKind::AssistantDelta { .. }))
                .count(),
            killed
                .into_iter()
                .filter(|kind| !matches!(
                    kind,
                    EventKind::ReasoningDelta { .. } | EventKind::AssistantDelta { .. }
                ))
                .collect::<Vec<EventKind>>(),
        ),
        (
            3,
            Vec::from([
                user(COUNT),
                EventKind::Reasoning {
                    text: String::from(COUNTING),
                },
                EventKind::SessionClosed {
                    reason: CloseReason::Exited {
                        status: String::from("signal: 9 (SIGKILL)"),
                    },
                },
            ])
        )
    );
    assert_eq!(
        send(&link, continued(session)).await,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Ready).await,
        [EventKind::SessionReady]
    );
    let _ = send(&link, message(session, "codeword?")).await;
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd)
            .await
            .into_iter()
            .filter(|kind| !matches!(
                kind,
                EventKind::ReasoningDelta { .. } | EventKind::AssistantDelta { .. }
            ))
            .collect::<Vec<EventKind>>(),
        [
            user("codeword?"),
            reply("quince"),
            turn_end(TurnOutcome::Completed),
        ]
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// A new credential that doesn't offer a Codex session's model: `Continue`
/// starts it on the new default and logs why. (It closed before its first
/// turn, so it starts a thread afresh; a resume takes the same choice.)
#[tokio::test]
async fn a_codex_session_continues_on_the_default_when_a_new_credential_drops_its_model() {
    let (fx, server) = codex_host("codex-resumed.jsonl").await;
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/e2e/codex-resumed-listed.jsonl"),
        server.join("stdout.2.jsonl"),
    )
    .unwrap();
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(
        send(
            &link,
            start_with(Provider::Codex, &server, "small", Effort::Low),
        )
        .await,
    );
    let _ = kinds_until(&mut updates, Until::Ready).await;
    assert_eq!(
        codex_offer(send(&link, codex_provider("conductor-test-key")).await),
        codex_gateway()
    );
    let _ = kinds_until(&mut updates, Until::Closed).await;
    assert_eq!(
        send(&link, continued(session)).await,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Ready).await,
        [
            EventKind::ModelChanged {
                model: Some(ModelId(String::from("conductor-test-model"))),
                effort: Some(Effort::Low),
                reason: String::from(
                    "Model changed to conductor-test-model: small is not offered by the current credential"
                ),
            },
            EventKind::SessionReady,
        ]
    );
    assert_eq!(
        requests(&server, "stdin.2.jsonl", "thread/start")
            .iter()
            .map(thread_settings)
            .collect::<Vec<_>>(),
        [(
            None,
            Some(serde_json::json!("conductor-test-model")),
            Some(serde_json::json!("low"))
        )]
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// The thread, model and effort a `thread/start` or `thread/resume` names.
fn thread_settings(
    params: &serde_json::Value,
) -> (
    Option<serde_json::Value>,
    Option<serde_json::Value>,
    Option<serde_json::Value>,
) {
    (
        params.get("threadId").cloned(),
        params.get("model").cloned(),
        params.pointer("/config/model_reasoning_effort").cloned(),
    )
}

const COUNT: &str = "Count from 1 to 300, one number per line.";
const PRIME: &str = "**Assessing primality**\n\nI need to determine if 1,000,003 is prime. To do this, I should check its divisors up to the square root of about 1,000. It seems I recall that 1,000,003 is a known prime number, often recognized as the first prime above one million. I think I can confidently answer \"yes\" to the user's question without needing any tools. It’s important to verify my information carefully, though!";

/// The app-server killed mid-turn closes the Codex session; `Continue`
/// starts a new server and resumes the thread on it with the session's
/// model and effort.
#[tokio::test]
async fn a_killed_codex_server_continues_the_thread() {
    let (fx, server) = codex_host("codex-killed.jsonl").await;
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/e2e/codex-resumed.jsonl"),
        server.join("stdout.2.jsonl"),
    )
    .unwrap();
    let (link, mut updates) = link(fx.target.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(
        send(
            &link,
            start_with(Provider::Codex, &server, "small", Effort::Low),
        )
        .await,
    );
    let _ = send(&link, message(session, "prime?")).await;
    let first = kinds_until(&mut updates, Until::TurnEnd).await;
    assert_eq!(
        first
            .into_iter()
            .filter(|kind| !matches!(
                kind,
                EventKind::ReasoningDelta { .. } | EventKind::AssistantDelta { .. }
            ))
            .collect::<Vec<EventKind>>(),
        [
            created_with(Provider::Codex, &server, "small", Effort::Low),
            EventKind::SessionReady,
            user("prime?"),
            EventKind::Reasoning {
                text: String::from(PRIME),
            },
            reply("Yes."),
            turn_end(TurnOutcome::Completed),
        ]
    );
    let _ = send(&link, message(session, COUNT)).await;
    assert_eq!(
        kinds_until(&mut updates, Until::Closed).await,
        [
            user(COUNT),
            // It dies before it answers `turn/start`.
            turn_end(TurnOutcome::Failed {
                message: String::from("the codex app-server has exited"),
            }),
            EventKind::SessionClosed {
                reason: CloseReason::Exited {
                    status: String::from("signal: 9 (SIGKILL)"),
                },
            },
        ]
    );
    assert_eq!(
        send(&link, continued(session)).await,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Ready).await,
        [EventKind::SessionReady]
    );
    let _ = send(&link, message(session, "codeword?")).await;
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd)
            .await
            .into_iter()
            .filter(|kind| !matches!(
                kind,
                EventKind::ReasoningDelta { .. } | EventKind::AssistantDelta { .. }
            ))
            .collect::<Vec<EventKind>>(),
        [
            user("codeword?"),
            reply("quince"),
            turn_end(TurnOutcome::Completed),
        ]
    );
    let started = requests(&server, "stdin.jsonl", "thread/start");
    let resumed = requests(&server, "stdin.2.jsonl", "thread/resume");
    assert_eq!(
        (
            started.iter().map(thread_settings).collect::<Vec<_>>(),
            resumed.iter().map(thread_settings).collect::<Vec<_>>(),
        ),
        (
            Vec::from([(
                None,
                Some(serde_json::json!("small")),
                Some(serde_json::json!("low"))
            )]),
            Vec::from([(
                Some(serde_json::json!("01a1d192-4ae3-7691-9a42-41f8302fa397")),
                Some(serde_json::json!("small")),
                Some(serde_json::json!("low"))
            )]),
        )
    );
    link.stop().await.unwrap();
    fx.router.shutdown().await.unwrap();
}

/// The original ticket and live link survive a real transport restart. The
/// saved cursor replays only restart cleanup, and Continue resumes the same
/// conversation after withdrawing an unanswered approval.
#[tokio::test]
async fn a_durable_host_restarts_with_the_same_ticket_cursor_and_conversation() {
    let dir = scratch("transport-restart", "claude-approve.jsonl");
    let data = data_dir("transport-restart");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/e2e/claude-resumed.jsonl"),
        dir.join("stdout.2.jsonl"),
    )
    .unwrap();
    let first = durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await;
    let address = first
        .target
        .addr
        .ip_addrs()
        .copied()
        .find(|address| address.is_ipv4())
        .unwrap();
    let ticket = first.target.clone();
    let (phone, mut updates) = link(ticket.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(send(&phone, start(Provider::Claude, &dir)).await);
    let _ = send(&phone, message(session, "create a.txt")).await;
    let before = legacy_events_until(&mut updates, Until::Request).await;
    let cursor = before.last().unwrap().seq;
    let conversation = argv(&dir)
        .first()
        .and_then(|run| flag(run, "--session-id"))
        .unwrap();
    let Fixture { router, log, .. } = first;
    router.shutdown().await.unwrap();
    drop(router);
    log.shutdown().await.unwrap();

    let second = durable_host(&data, address).await;
    assert_eq!(second.target.addr.id, ticket.addr.id);
    assert_eq!(second.target.token, ticket.token);
    phone.reconnect().await;
    let caught_up = until_live(&mut updates).await;
    assert_eq!(
        caught_up
            .iter()
            .filter(|update| matches!(update, Update::Reset(_)))
            .count(),
        0
    );
    let cleanup = caught_up
        .into_iter()
        .filter_map(|update| match update {
            Update::Event(event) => Some(event),
            Update::Status(_) | Update::Reset(_) => None,
        })
        .collect::<Vec<Event>>();
    assert_eq!(
        seqs(&cleanup),
        Vec::from([
            HostSeq(cursor.0.checked_add(1).unwrap()),
            HostSeq(cursor.0.checked_add(2).unwrap()),
            HostSeq(cursor.0.checked_add(3).unwrap()),
            HostSeq(cursor.0.checked_add(4).unwrap()),
            HostSeq(cursor.0.checked_add(5).unwrap()),
        ])
    );
    assert_eq!(
        checkpoint(&cleanup, proto::CheckpointId(1)).parent,
        Some(proto::CheckpointId(0))
    );
    assert_eq!(
        of(session, &cleanup),
        [
            EventKind::RequestResolved {
                request: RequestId(1),
                resolution: Resolution::Withdrawn
            },
            turn_end(TurnOutcome::Interrupted),
            EventKind::SessionClosed {
                reason: CloseReason::Stopped {
                    reason: String::from("the host restarted")
                }
            },
        ]
    );
    assert_eq!(
        send(&phone, resolve(session, 1, Decision::Approve)).await,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    assert_eq!(
        send(&phone, continued(session)).await,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Ready).await,
        [EventKind::SessionReady]
    );
    assert_eq!(
        send(&phone, resolve(session, 1, Decision::Approve)).await,
        CommandResult::Rejected(Rejection::UnknownRequest)
    );
    let _ = send(&phone, message(session, "how many sheep?")).await;
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd)
            .await
            .into_iter()
            .filter(|kind| !matches!(kind, EventKind::AssistantDelta { .. }))
            .collect::<Vec<EventKind>>(),
        [
            user("how many sheep?"),
            reply("18"),
            turn_end(TurnOutcome::Completed)
        ]
    );
    assert_eq!(
        argv(&dir).last().and_then(|run| run
            .iter()
            .find(|argument| argument.starts_with("--resume="))),
        Some(&format!("--resume={conversation}"))
    );
    phone.stop().await.unwrap();
    let Fixture { router, log, .. } = second;
    router.shutdown().await.unwrap();
    drop(router);
    log.shutdown().await.unwrap();
}

/// A raw connection permits an explicit retry ID, including from a fresh
/// endpoint identity after the host restarts.
async fn authenticated(endpoint: &Endpoint, target: &Target) -> iroh::endpoint::Connection {
    let conn = endpoint
        .connect(target.addr.clone(), proto::ALPN)
        .await
        .unwrap();
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    proto::wire::write(
        &mut send,
        &proto::Open::Hello(proto::Hello {
            client: test_client(),
            token: target.token,
        }),
    )
    .await
    .unwrap();
    send.finish().unwrap();
    let welcome = proto::wire::read::<proto::HelloReply>(&mut recv)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        Some(&welcome).filter(|reply| matches!(reply, proto::HelloReply::Welcome(_))),
        Some(&welcome)
    );
    conn
}

async fn retry_command(
    conn: &iroh::endpoint::Connection,
    command: &proto::Command,
) -> CommandResult {
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    proto::wire::write(&mut send, &proto::Open::Command(command.clone()))
        .await
        .unwrap();
    send.finish().unwrap();
    proto::wire::read::<CommandResult>(&mut recv)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn one_command_receipt_survives_new_transport_identities_and_a_host_restart() {
    let dir = scratch("receipt-transport", "claude-approve.jsonl");
    let data = data_dir("receipt-transport");
    let first = durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await;
    let address = first
        .target
        .addr
        .ip_addrs()
        .copied()
        .find(|address| address.is_ipv4())
        .unwrap();
    let ticket = first.target.clone();
    let command = proto::Command {
        id: proto::CommandId(71),
        body: start(Provider::Claude, &dir),
    };
    let desktop = endpoint().await;
    let phone = endpoint().await;
    assert_ne!(desktop.id(), phone.id());
    let desktop_conn = authenticated(&desktop, &ticket).await;
    let phone_conn = authenticated(&phone, &ticket).await;
    let receipt = retry_command(&desktop_conn, &command).await;
    let session = started_session(receipt.clone());
    assert_eq!(retry_command(&phone_conn, &command).await, receipt);
    desktop.close().await;
    phone.close().await;
    let Fixture { router, log, .. } = first;
    router.shutdown().await.unwrap();
    drop(router);
    log.shutdown().await.unwrap();

    let second = durable_host(&data, address).await;
    let restarted_phone = endpoint().await;
    assert_ne!(restarted_phone.id(), desktop.id());
    assert_ne!(restarted_phone.id(), phone.id());
    let conn = authenticated(&restarted_phone, &ticket).await;
    assert_eq!(retry_command(&conn, &command).await, receipt);
    let history = second.log.subscribe(HostSeq(0)).await.unwrap().replay;
    assert_eq!(
        history
            .iter()
            .filter(|event| matches!(event.kind, EventKind::SessionCreated { .. }))
            .count(),
        1
    );
    assert_eq!(history.iter().find(|event| event.session != session), None);
    restarted_phone.close().await;
    let Fixture { router, log, .. } = second;
    router.shutdown().await.unwrap();
    drop(router);
    log.shutdown().await.unwrap();
}

fn checkpoint(events: &[Event], id: proto::CheckpointId) -> proto::Checkpoint {
    events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::CheckpointCreated { checkpoint } if checkpoint.id == id => {
                Some(checkpoint.clone())
            }
            _ => None,
        })
        .expect("the requested checkpoint was published")
}

fn workspace(events: &[Event]) -> proto::Workspace {
    events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::WorkspaceCreated { workspace } => Some(workspace.clone()),
            _ => None,
        })
        .expect("the managed worktree was published")
}

fn diff_result(result: CommandResult) -> proto::CheckpointDiff {
    match result {
        CommandResult::Diff(diff) => Some(diff),
        _ => None,
    }
    .expect("a checkpoint diff")
}

/// File undo and the lazy native fork survive restart together. All Git
/// staging/checkpoint operations stay out of the source repository's index.
#[tokio::test]
async fn managed_worktrees_isolate_source_diff_and_undo_files_and_history_after_restart() {
    let source = scratch("workspace-undo", "claude-worktrees.jsonl");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/claude-worktrees-fork.jsonl"),
        source.join("stdout.2.jsonl"),
    )
    .unwrap();
    std::fs::copy(source.join("stdout.2.jsonl"), source.join("stdout.3.jsonl")).unwrap();
    std::fs::write(source.join("baseline.txt"), "source staged\n").unwrap();
    git(&source, &["add", "baseline.txt"]);
    std::fs::write(source.join("baseline.txt"), "source unstaged\n").unwrap();
    std::fs::write(source.join("source-only.txt"), "never enter a session\n").unwrap();
    let index = std::fs::read(source.join(".git/index")).unwrap();
    let staged = git(&source, &["diff", "--cached"]);
    let unstaged = git(&source, &["diff"]);
    let data = data_dir("workspace-undo");
    let first = durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await;
    let address = first
        .target
        .addr
        .ip_addrs()
        .copied()
        .find(|address| address.is_ipv4())
        .unwrap();
    let ticket = first.target.clone();
    let (phone, mut updates) = link(ticket.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(send(&phone, start(Provider::Claude, &source)).await);
    let opening = events_until(&mut updates, Until::Ready).await;
    let managed = workspace(&opening);
    assert_eq!(checkpoint(&opening, proto::CheckpointId(0)).parent, None);
    assert_eq!(managed.original.0, source.display().to_string());
    assert_eq!(
        managed.branch,
        format!("conductor/{}", ulid::Ulid::from(session.0))
    );
    let worktree = PathBuf::from(&managed.worktree.0);
    assert_ne!(worktree, source);
    assert_eq!(
        std::fs::read_to_string(worktree.join("baseline.txt")).unwrap(),
        "committed baseline\n"
    );
    assert_eq!(
        std::fs::metadata(worktree.join("source-only.txt"))
            .err()
            .map(|error| error.kind()),
        Some(std::io::ErrorKind::NotFound)
    );
    let other = started_session(send(&phone, start(Provider::Claude, &source)).await);
    let other_opening = events_until(&mut updates, Until::Ready).await;
    let other_workspace = workspace(&other_opening);
    let other_path = PathBuf::from(&other_workspace.worktree.0);
    assert_ne!(session, other);
    assert_ne!(managed.worktree, other_workspace.worktree);
    assert_ne!(managed.branch, other_workspace.branch);
    let _ = send(&phone, message(session, "turn one")).await;
    let first_turn = events_until(&mut updates, Until::TurnEnd).await;
    let retained = checkpoint(&first_turn, proto::CheckpointId(1));
    let _ = send(&phone, message(session, "turn two")).await;
    let second_turn = events_until(&mut updates, Until::TurnEnd).await;
    let discarded = checkpoint(&second_turn, proto::CheckpointId(2));
    assert_eq!(discarded.parent, Some(retained.id));
    let diff = diff_result(
        send(
            &phone,
            CommandBody::GetDiff {
                session,
                checkpoint: discarded.id,
            },
        )
        .await,
    );
    assert_eq!(diff.parent, retained.id);
    assert_eq!(diff.extent, proto::DiffExtent::Complete);
    assert_eq!(
        diff.files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<String>>(),
        ["changed.txt", "deleted.txt", "exec.sh", "introduced.txt"]
    );
    assert_ne!(
        diff.files
            .iter()
            .filter_map(|file| match &file.patch {
                proto::FilePatch::Text(patch) => Some(patch.as_str()),
                _ => None,
            })
            .collect::<String>()
            .find("+turn two"),
        None
    );
    assert_eq!(
        git(&worktree, &["ls-tree", &discarded.commit, "exec.sh"])
            .split_whitespace()
            .next(),
        Some("100755")
    );
    let _ = send(&phone, message(session, "turn three")).await;
    let third_turn = events_until(&mut updates, Until::TurnEnd).await;
    checkpoint(&third_turn, proto::CheckpointId(3));
    assert_eq!(std::fs::read(source.join(".git/index")).unwrap(), index);
    assert_eq!(git(&source, &["diff", "--cached"]), staged);
    assert_eq!(git(&source, &["diff"]), unstaged);
    assert_eq!(
        std::fs::metadata(other_path.join("changed.txt"))
            .err()
            .map(|error| error.kind()),
        Some(std::io::ErrorKind::NotFound)
    );
    let _ = send(&phone, message(other, "an independent second session")).await;
    let other_turn = events_until(&mut updates, Until::TurnEnd).await;
    checkpoint(&other_turn, proto::CheckpointId(1));
    assert_eq!(
        std::fs::read_to_string(other_path.join("after-undo.txt")).unwrap(),
        "continued from retained prefix\n"
    );
    assert_eq!(
        std::fs::metadata(worktree.join("after-undo.txt"))
            .err()
            .map(|error| error.kind()),
        Some(std::io::ErrorKind::NotFound)
    );
    let retry_endpoint = endpoint().await;
    let retry = authenticated(&retry_endpoint, &ticket).await;
    let undo = proto::Command {
        id: proto::CommandId(1301),
        body: CommandBody::UndoTurn {
            session,
            checkpoint: discarded.id,
        },
    };
    let receipt = retry_command(&retry, &undo).await;
    assert_eq!(receipt, CommandResult::Accepted);
    let undone = events_until(&mut updates, Until::Undone).await;
    assert_eq!(
        of(session, &undone).last(),
        Some(&EventKind::TurnsUndone {
            checkpoint: discarded.id,
            restored: retained.id
        })
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("changed.txt")).unwrap(),
        "turn one\n"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("deleted.txt")).unwrap(),
        "restore me\n"
    );
    assert_eq!(
        std::fs::metadata(worktree.join("introduced.txt"))
            .err()
            .map(|error| error.kind()),
        Some(std::io::ErrorKind::NotFound)
    );
    assert_eq!(
        std::fs::metadata(worktree.join("later.txt"))
            .err()
            .map(|error| error.kind()),
        Some(std::io::ErrorKind::NotFound)
    );
    assert_eq!(
        git(&worktree, &["ls-files", "--stage", "exec.sh"])
            .split_whitespace()
            .next(),
        Some("100644")
    );
    retry_endpoint.close().await;
    let Fixture { router, log, .. } = first;
    router.shutdown().await.unwrap();
    drop(router);
    log.shutdown().await.unwrap();
    let second = durable_host(&data, address).await;
    phone.reconnect().await;
    until_live(&mut updates).await;
    let restarted_retry_endpoint = endpoint().await;
    let restarted_retry = authenticated(&restarted_retry_endpoint, &ticket).await;
    assert_eq!(retry_command(&restarted_retry, &undo).await, receipt);
    assert_eq!(
        send(&phone, continued(session)).await,
        CommandResult::Accepted
    );
    events_until(&mut updates, Until::Ready).await;
    let _ = send(&phone, message(session, "remember retained prefix")).await;
    let after = events_until(&mut updates, Until::TurnEnd).await;
    assert_eq!(
        of(session, &after)
            .iter()
            .filter(|kind| matches!(kind, EventKind::AssistantMessage { .. }))
            .cloned()
            .collect::<Vec<EventKind>>(),
        [reply("Retained turn one.")]
    );
    let runs = argv(&source);
    let resumed = runs.last().unwrap();
    assert_eq!(
        resumed
            .iter()
            .find(|arg| arg.starts_with("--resume-session-at="))
            .cloned(),
        Some(String::from(
            "--resume-session-at=00000000-0000-4000-8000-000000000201"
        ))
    );
    assert_eq!(
        resumed
            .iter()
            .find(|arg| arg.as_str() == "--fork-session")
            .cloned(),
        Some(String::from("--fork-session"))
    );
    assert_eq!(std::fs::read(source.join(".git/index")).unwrap(), index);
    restarted_retry_endpoint.close().await;
    phone.stop().await.unwrap();
    let Fixture { router, log, .. } = second;
    router.shutdown().await.unwrap();
    drop(router);
    log.shutdown().await.unwrap();
}

#[tokio::test]
async fn new_sessions_require_an_existing_repository_with_a_commit() {
    let host = host().await;
    let (phone, mut updates) = link(host.target.clone(), None).await;
    until_live(&mut updates).await;
    let plain = data_dir("not-a-repository");
    // Stop Git discovery at this fixture instead of finding the enclosing checkout.
    std::fs::write(plain.join(".git"), "gitdir: no-such-git-directory\n").unwrap();
    assert_eq!(
        send(&phone, start(Provider::Claude, &plain)).await,
        CommandResult::Rejected(Rejection::NotGitRepository)
    );
    std::fs::remove_file(plain.join(".git")).unwrap();
    git(&plain, &["init", "--quiet"]);
    assert_eq!(
        send(&phone, start(Provider::Claude, &plain)).await,
        CommandResult::Rejected(Rejection::NotGitRepository)
    );
    phone.stop().await.unwrap();
    host.router.shutdown().await.unwrap();
}

/// Old persisted sessions have no workspace metadata. Continue still starts
/// their CLI in the original directory, even when it is not a Git repository.
#[tokio::test]
async fn a_legacy_session_continues_in_its_original_directory_without_undo() {
    let source = data_dir("legacy-session-source");
    std::fs::write(source.join(".git"), "gitdir: no-such-git-directory\n").unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/claude-worktrees-fork.jsonl"),
        source.join("stdout.jsonl"),
    )
    .unwrap();
    let data = data_dir("legacy-session-host");
    let path = data.join("conductor.redb");
    let log = Log::open(String::from("test-host"), &path).unwrap();
    let session = SessionId(1310);
    log.append(
        session,
        EventKind::SessionCreated {
            provider: Provider::Claude,
            directory: HostPath(source.display().to_string()),
            by: test_client(),
            model: None,
            effort: None,
        },
    )
    .await
    .unwrap();
    log.append(
        session,
        EventKind::SessionClosed {
            reason: CloseReason::Stopped {
                reason: String::from("old host stopped"),
            },
        },
    )
    .await
    .unwrap();
    log.shutdown().await.unwrap();
    let host = durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await;
    let (phone, mut updates) = link(host.target.clone(), None).await;
    until_live(&mut updates).await;
    assert_eq!(
        send(
            &phone,
            CommandBody::UndoTurn {
                session,
                checkpoint: proto::CheckpointId(0)
            }
        )
        .await,
        CommandResult::Rejected(Rejection::CheckpointUnavailable)
    );
    assert_eq!(
        send(&phone, continued(session)).await,
        CommandResult::Accepted
    );
    let ready = events_until(&mut updates, Until::Ready).await;
    assert_eq!(
        ready
            .iter()
            .find(|event| matches!(event.kind, EventKind::WorkspaceCreated { .. })),
        None
    );
    let _ = send(&phone, message(session, "legacy directory")).await;
    events_until(&mut updates, Until::TurnEnd).await;
    assert_eq!(
        std::fs::read_to_string(source.join("after-undo.txt")).unwrap(),
        "continued from retained prefix\n"
    );
    phone.stop().await.unwrap();
    let Fixture { router, log, .. } = host;
    router.shutdown().await.unwrap();
    drop(router);
    log.shutdown().await.unwrap();
}

/// A crash after durable undo intent/failure retains the original transcript
/// and blocks normal commands until the same undo completes on retry.
#[tokio::test]
async fn a_failed_pending_undo_replays_after_restart_and_requires_a_successful_retry() {
    let source = scratch("pending-undo", "claude-worktrees.jsonl");
    let data = data_dir("pending-undo");
    let first = durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await;
    let address = first
        .target
        .addr
        .ip_addrs()
        .copied()
        .find(|address| address.is_ipv4())
        .unwrap();
    let ticket = first.target.clone();
    let (phone, mut updates) = link(ticket.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(send(&phone, start(Provider::Claude, &source)).await);
    events_until(&mut updates, Until::Ready).await;
    let _ = send(&phone, message(session, "turn one")).await;
    let retained = checkpoint(
        &events_until(&mut updates, Until::TurnEnd).await,
        proto::CheckpointId(1),
    );
    let _ = send(&phone, message(session, "turn two")).await;
    let discarded = checkpoint(
        &events_until(&mut updates, Until::TurnEnd).await,
        proto::CheckpointId(2),
    );
    let history = match retained.history {
        proto::HistoryAnchor::Claude {
            conversation,
            message: Some(through),
        } => Some(proto::HistoryAnchor::ClaudeFork {
            source: conversation,
            through,
            target: String::from("00000000-0000-4000-8000-000000000300"),
        }),
        _ => None,
    }
    .expect("a verified retained Claude transcript boundary");
    first
        .log
        .append(
            session,
            EventKind::UndoStarted {
                checkpoint: discarded.id,
                restore: retained.id,
                history,
            },
        )
        .await
        .unwrap();
    first
        .log
        .append(
            session,
            EventKind::UndoFailed {
                checkpoint: discarded.id,
                reason: String::from("injected failure before files were restored"),
            },
        )
        .await
        .unwrap();
    let Fixture { router, log, .. } = first;
    router.shutdown().await.unwrap();
    drop(router);
    log.shutdown().await.unwrap();
    let second = durable_host(&data, address).await;
    phone.reconnect().await;
    until_live(&mut updates).await;
    let replay = second.log.subscribe(HostSeq(0)).await.unwrap().replay;
    let view = fold(replay);
    assert_eq!(
        view.sessions
            .get(&session)
            .map(|session| session.undo.clone()),
        Some(client::host_view::UndoState::Failed {
            checkpoint: discarded.id,
            reason: String::from("injected failure before files were restored")
        })
    );
    assert_eq!(
        items(&view, session)
            .iter()
            .filter(|item| matches!(item, Item::User { .. }))
            .count(),
        2
    );
    assert_eq!(
        send(&phone, message(session, "blocked while recovering")).await,
        CommandResult::Rejected(Rejection::UndoInProgress)
    );
    assert_eq!(
        send(&phone, continued(session)).await,
        CommandResult::Rejected(Rejection::UndoInProgress)
    );
    assert_eq!(
        send(
            &phone,
            CommandBody::UndoTurn {
                session,
                checkpoint: discarded.id
            }
        )
        .await,
        CommandResult::Accepted
    );
    events_until(&mut updates, Until::Undone).await;
    let after = fold(second.log.subscribe(HostSeq(0)).await.unwrap().replay);
    assert_eq!(
        items(&after, session)
            .iter()
            .filter(|item| matches!(item, Item::User { .. }))
            .count(),
        1
    );
    assert_eq!(
        after
            .sessions
            .get(&session)
            .map(|session| session.undo.clone()),
        Some(client::host_view::UndoState::Undone)
    );
    phone.stop().await.unwrap();
    let Fixture { router, log, .. } = second;
    router.shutdown().await.unwrap();
    drop(router);
    log.shutdown().await.unwrap();
}

fn undo_failure(result: CommandResult) -> String {
    match result {
        CommandResult::Rejected(Rejection::UndoFailed { reason }) => Some(reason),
        _ => None,
    }
    .expect("an honest undo phase failure")
}

#[tokio::test]
async fn native_refusal_then_files_failure_requires_retry_across_a_real_restart() {
    let source = scratch("actual-undo-failures", "claude-worktrees.jsonl");
    let data = data_dir("actual-undo-failures");
    let first = durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await;
    let address = first
        .target
        .addr
        .ip_addrs()
        .copied()
        .find(|address| address.is_ipv4())
        .unwrap();
    let ticket = first.target.clone();
    let (phone, mut updates) = link(ticket.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(send(&phone, start(Provider::Claude, &source)).await);
    let managed = workspace(&events_until(&mut updates, Until::Ready).await);
    let worktree = PathBuf::from(&managed.worktree.0);
    let _ = send(&phone, message(session, "turn one")).await;
    let retained = checkpoint(
        &events_until(&mut updates, Until::TurnEnd).await,
        proto::CheckpointId(1),
    );
    let _ = send(&phone, message(session, "turn two")).await;
    let selected = checkpoint(
        &events_until(&mut updates, Until::TurnEnd).await,
        proto::CheckpointId(2),
    );
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/claude-worktrees-fork-refused.jsonl"),
        source.join("stdout.2.jsonl"),
    )
    .unwrap();
    let undo = CommandBody::UndoTurn {
        session,
        checkpoint: selected.id,
    };
    assert_ne!(
        undo_failure(send(&phone, undo.clone()).await).find("temporary fake native fork refusal"),
        None
    );
    let native_failure = events_until(&mut updates, Until::UndoFailure).await;
    assert_eq!(
        native_failure
            .iter()
            .find(|event| matches!(event.kind, EventKind::UndoHistoryRestored { .. })),
        None
    );
    assert_ne!(
        std::fs::read_to_string(worktree.join("changed.txt"))
            .unwrap()
            .find("turn two"),
        None
    );
    assert_eq!(
        send(&phone, message(session, "blocked after native refusal")).await,
        CommandResult::Rejected(Rejection::UndoInProgress)
    );
    let git_dir = PathBuf::from(git(&worktree, &["rev-parse", "--absolute-git-dir"]).trim());
    let lock = git_dir.join("index.lock");
    std::fs::write(&lock, "owned test lock\n").unwrap();
    assert_ne!(
        undo_failure(send(&phone, undo.clone()).await).find("index.lock"),
        None
    );
    let files_failure = events_until(&mut updates, Until::UndoFailure).await;
    assert_eq!(
        files_failure
            .iter()
            .filter(|event| matches!(event.kind, EventKind::UndoHistoryRestored { .. }))
            .count(),
        1
    );
    assert_eq!(
        files_failure
            .iter()
            .find(|event| matches!(event.kind, EventKind::TurnsUndone { .. })),
        None
    );
    assert_eq!(
        send(&phone, continued(session)).await,
        CommandResult::Rejected(Rejection::UndoInProgress)
    );
    let Fixture { router, log, .. } = first;
    router.shutdown().await.unwrap();
    drop(router);
    log.shutdown().await.unwrap();
    let second = durable_host(&data, address).await;
    phone.reconnect().await;
    until_live(&mut updates).await;
    assert_eq!(
        send(&phone, message(session, "blocked after restart")).await,
        CommandResult::Rejected(Rejection::UndoInProgress)
    );
    std::fs::remove_file(&lock).unwrap();
    assert_eq!(send(&phone, undo).await, CommandResult::Accepted);
    let recovered = events_until(&mut updates, Until::Undone).await;
    assert_eq!(
        recovered
            .iter()
            .find(|event| matches!(event.kind, EventKind::UndoHistoryRestored { .. })),
        None
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("changed.txt")).unwrap(),
        "turn one\n"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("deleted.txt")).unwrap(),
        "restore me\n"
    );
    assert_eq!(
        std::fs::metadata(worktree.join("introduced.txt"))
            .err()
            .map(|error| error.kind()),
        Some(std::io::ErrorKind::NotFound)
    );
    let restored = fold(second.log.subscribe(HostSeq(0)).await.unwrap().replay);
    assert_eq!(
        items(&restored, session)
            .iter()
            .filter(|item| matches!(item, Item::User { .. }))
            .count(),
        1
    );
    assert_eq!(
        send(&phone, message(session, "retained prefix after recovery")).await,
        CommandResult::Accepted
    );
    let after = events_until(&mut updates, Until::TurnEnd).await;
    assert_ne!(checkpoint(&after, proto::CheckpointId(3)).id, retained.id);
    assert_eq!(
        of(session, &after)
            .iter()
            .filter(|kind| matches!(kind, EventKind::AssistantMessage { .. }))
            .cloned()
            .collect::<Vec<EventKind>>(),
        [reply("Retained turn one.")]
    );
    phone.stop().await.unwrap();
    let Fixture { router, log, .. } = second;
    router.shutdown().await.unwrap();
    drop(router);
    log.shutdown().await.unwrap();
}

async fn drive_materialization(path: &Path) -> std::io::Result<()> {
    loop {
        match tokio::fs::metadata(path).await {
            Ok(_) => break Ok(()),
            Err(error) => match error.kind() {
                std::io::ErrorKind::NotFound => tokio::time::sleep(Duration::from_millis(10)).await,
                _ => break Err(error),
            },
        }
    }
}

/// The provider persisted the first post-undo input before it notified the
/// daemon. After restart, resume that existing target instead of forking the
/// older prefix again and silently discarding the newly persisted input.
#[tokio::test]
async fn a_materialized_fork_target_is_resumed_when_its_notification_was_lost() {
    let source = scratch("materialized-fork-crash", "claude-worktrees.jsonl");
    let data = data_dir("materialized-fork-crash");
    let first = durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await;
    let address = first
        .target
        .addr
        .ip_addrs()
        .copied()
        .find(|address| address.is_ipv4())
        .unwrap();
    let ticket = first.target.clone();
    let (phone, mut updates) = link(ticket.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(send(&phone, start(Provider::Claude, &source)).await);
    let managed = workspace(&events_until(&mut updates, Until::Ready).await);
    let worktree = PathBuf::from(&managed.worktree.0);
    let _ = send(&phone, message(session, "turn one")).await;
    events_until(&mut updates, Until::TurnEnd).await;
    let _ = send(&phone, message(session, "turn two")).await;
    events_until(&mut updates, Until::TurnEnd).await;
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/claude-worktrees-fork-held.jsonl"),
        source.join("stdout.2.jsonl"),
    )
    .unwrap();
    assert_eq!(
        send(
            &phone,
            CommandBody::UndoTurn {
                session,
                checkpoint: proto::CheckpointId(2)
            }
        )
        .await,
        CommandResult::Accepted
    );
    let undone = events_until(&mut updates, Until::Undone).await;
    let after_undo = undone.last().unwrap().seq;
    let target = undone
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::UndoHistoryRestored {
                history: proto::HistoryAnchor::ClaudeFork { target, .. },
                ..
            } => Some(target.clone()),
            _ => None,
        })
        .expect("the durable lazy fork target");
    assert_eq!(
        send(&phone, message(session, "first post undo input")).await,
        CommandResult::Accepted
    );
    tokio::time::timeout(
        STEP,
        drive_materialization(&source.join("fork-materialized")),
    )
    .await
    .unwrap()
    .unwrap();
    let native_transcript =
        std::fs::read_to_string(source.join(".fake-sessions").join(&target)).unwrap();
    assert_ne!(native_transcript.find("first post undo input"), None);
    let unnotified = first.log.subscribe(after_undo).await.unwrap().replay;
    assert_eq!(
        unnotified.iter().find(|event| matches!(
            event.kind,
            EventKind::NativeTurnStarted { .. } | EventKind::HistoryMaterialized { .. }
        )),
        None
    );
    let before_runs = argv(&source).len();
    let Fixture { router, log, .. } = first;
    router.shutdown().await.unwrap();
    drop(router);
    log.shutdown().await.unwrap();
    let second = durable_host(&data, address).await;
    phone.reconnect().await;
    until_live(&mut updates).await;
    let recovery = second.log.subscribe(HostSeq(0)).await.unwrap().replay;
    let unknown = checkpoint(&recovery, proto::CheckpointId(3));
    assert_eq!(unknown.history, proto::HistoryAnchor::Unknown);
    assert_ne!(
        Some(&unknown.undo)
            .filter(|undo| matches!(undo, proto::UndoAvailability::Unavailable { .. })),
        None
    );
    assert_eq!(
        git(
            &worktree,
            &["show", &format!("{}:unnotified.txt", unknown.commit)]
        ),
        "provider wrote before notification\n"
    );
    assert_eq!(
        send(
            &phone,
            CommandBody::UndoTurn {
                session,
                checkpoint: unknown.id
            }
        )
        .await,
        CommandResult::Rejected(Rejection::CheckpointUnavailable)
    );
    assert_eq!(
        send(&phone, continued(session)).await,
        CommandResult::Accepted
    );
    events_until(&mut updates, Until::Ready).await;
    let runs = argv(&source);
    assert_eq!(runs.len(), before_runs.checked_add(1).unwrap());
    let resumed = runs.last().unwrap();
    assert_eq!(
        resumed
            .iter()
            .find(|arg| arg.starts_with("--resume="))
            .cloned(),
        Some(format!("--resume={target}"))
    );
    assert_eq!(
        resumed.iter().find(|arg| arg.as_str() == "--fork-session"),
        None
    );
    std::fs::write(source.join("fork-release"), "").unwrap();
    let _ = send(&phone, message(session, "continue existing target")).await;
    let after = events_until(&mut updates, Until::TurnEnd).await;
    assert_eq!(
        checkpoint(&after, proto::CheckpointId(4)).history,
        proto::HistoryAnchor::Claude {
            conversation: target,
            message: Some(String::from("00000000-0000-4000-8000-000000000204"))
        }
    );
    phone.stop().await.unwrap();
    let Fixture { router, log, .. } = second;
    router.shutdown().await.unwrap();
    drop(router);
    log.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_failed_snapshot_is_retried_before_accepting_the_next_user_message() {
    let source = scratch("snapshot-retry", "claude-worktrees.jsonl");
    let host = host().await;
    let (phone, mut updates) = link(host.target.clone(), None).await;
    until_live(&mut updates).await;
    let session = started_session(send(&phone, start(Provider::Claude, &source)).await);
    let managed = workspace(&events_until(&mut updates, Until::Ready).await);
    let worktree = PathBuf::from(&managed.worktree.0);
    let _ = send(&phone, message(session, "turn one")).await;
    let retained = checkpoint(
        &events_until(&mut updates, Until::TurnEnd).await,
        proto::CheckpointId(1),
    );
    git(&worktree, &["symbolic-ref", "HEAD", "refs/heads/unrelated"]);
    let _ = send(&phone, message(session, "turn two")).await;
    let failed = events_until(&mut updates, Until::TurnEnd).await;
    assert_eq!(
        failed
            .iter()
            .find(|event| matches!(event.kind, EventKind::CheckpointCreated { .. })),
        None
    );
    assert_eq!(
        failed
            .iter()
            .filter(|event| matches!(event.kind, EventKind::Error { .. }))
            .count(),
        1
    );
    let before = host.log.subscribe(HostSeq(0)).await.unwrap().replay;
    let users = before
        .iter()
        .filter(|event| matches!(event.kind, EventKind::UserMessage { .. }))
        .count();
    let rejected = send(&phone, message(session, "must not enter native history")).await;
    assert_ne!(
        Some(rejected).filter(|result| matches!(
            result,
            CommandResult::Rejected(Rejection::WorkspaceFailure { .. })
        )),
        None
    );
    let still = host.log.subscribe(HostSeq(0)).await.unwrap().replay;
    assert_eq!(
        still
            .iter()
            .filter(|event| matches!(event.kind, EventKind::UserMessage { .. }))
            .count(),
        users
    );
    git(
        &worktree,
        &[
            "symbolic-ref",
            "HEAD",
            &format!("refs/heads/{}", managed.branch),
        ],
    );
    assert_eq!(
        send(&phone, message(session, "turn three")).await,
        CommandResult::Accepted
    );
    let after = events_until(&mut updates, Until::TurnEnd).await;
    let checkpoints = after
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::CheckpointCreated { checkpoint } => Some(checkpoint.clone()),
            _ => None,
        })
        .collect::<Vec<proto::Checkpoint>>();
    assert_eq!(checkpoints.len(), 2);
    let recovered = checkpoints.first().unwrap();
    let latest = checkpoints.last().unwrap();
    assert_eq!(recovered.parent, Some(retained.id));
    assert_eq!(latest.parent, Some(recovered.id));
    assert_eq!(recovered.id.cmp(&proto::CheckpointId(2)), Ordering::Greater);
    let diff = diff_result(
        send(
            &phone,
            CommandBody::GetDiff {
                session,
                checkpoint: latest.id,
            },
        )
        .await,
    );
    assert_eq!(
        diff.files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<String>>(),
        ["changed.txt", "later.txt"]
    );
    phone.stop().await.unwrap();
    host.router.shutdown().await.unwrap();
}
