#![cfg(test)]

//! Host and client in one process over real iroh connections on loopback, with
//! relays off so the test needs no network.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use claude::Sandbox;
use claude_cli::ClaudeLaunch;
use client::host_view::{
    Approval, HostView, Item, ReplyState, RequestState, SessionStatus,
    Subagent, SubagentStatus, ToolState,
};
use codex_cli::{Hub, ServerOptions};
use daemon::Stopped;
use daemon::credentials::Store;
use daemon::home::{HomeClone, Homing};
use daemon::log::Log;
use daemon::registry::{
    Builder, Launcher, Launchers, Offer, OwnSignIn, Registry, Setup,
};
use daemon::serve::Conductor;
use daemon::sign_in::Signer;
use install::Home;
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, RelayMode};
use net::{
    Cursor, HostLink, LinkOptions, LinkState, Network, Status, Target, Update,
};
use proto::{
    Answer, Auth, AuthToken, Availability, Change, ClientName, CloseReason,
    CommandBody, CommandResult, Credential, Decision, Effort, Epoch, Event,
    EventKind, FileChange, HostPath, HostSeq, InstanceId, InstanceStatus,
    Instances, IssueChange, IssueFields, IssueStatus, Kept, ModelChoice,
    ModelId, Models, Priority, ProjectId, Provider, Question, QuestionOption,
    Rejection, RequestId, Resolution, Roster, Selection, SessionId, StartAt,
    SubagentActivity, SubagentEvent, SubagentId, SubagentOutcome, Task, TaskId,
    TaskStatus, ToolCallId, ToolKind, ToolOutcome, ToolPhase, TurnOutcome,
};
use runtime::Settings;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;

const TOKEN: [u8; 16] = [7; 16];
const STEP: Duration = Duration::from_secs(10);
const UNAVAILABLE: &str = "no agents in this test";

type Fallible<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn endpoint() -> Fallible<Endpoint> {
    Ok(Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?)
}

struct Fixture {
    router: Router,
    target: Target,
    log: Log,
}

impl Fixture {
    fn address(&self) -> Fallible<SocketAddr> {
        let address = self
            .target
            .addr
            .ip_addrs()
            .copied()
            .find(SocketAddr::is_ipv4)
            .ok_or("the host has no IPv4 address")?;
        Ok(address)
    }

    async fn shutdown(self) -> Fallible {
        let Self { router, log, .. } = self;
        router.shutdown().await?;
        drop(router);
        log.shutdown().await?;
        Ok(())
    }
}

/// The gated stand-in that plays `stdout.jsonl` from the session's directory.
fn fake_claude() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../claude-cli/tests/fake-claude.sh")
}

/// The gated stand-in that plays `stdout.jsonl` from the app-server's
/// working directory.
fn fake_codex() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../codex-cli/tests/fake-codex.sh")
}

/// A host whose Claude is `claude_path` and whose Codex is unavailable.
async fn host_with(claude_path: PathBuf) -> Fallible<Fixture> {
    host_of(claude_only(claude_path)?).await
}

fn claude_only(claude_path: PathBuf) -> Fallible<Launchers> {
    Ok(launchers([
        (
            Provider::Claude,
            Launcher::Ready {
                start: claude_cli::starter(ClaudeLaunch {
                    claude_path,
                    home: Home::User,
                    credential: None,
                    probe: target_tmp()?.join("probe"),
                }),
                own: OwnSignIn::Unmanaged,
                offer: offer(models()),
            },
        ),
        (
            Provider::Codex,
            Launcher::Unavailable(String::from(UNAVAILABLE)),
        ),
    ]))
}

/// A host on the fake Claude whose home is at `remote`, looking for
/// checkouts in `roots`, and its data dir.
async fn host_at_home(
    remote: String,
    roots: Vec<PathBuf>,
) -> Fallible<(Fixture, PathBuf)> {
    let endpoint = endpoint().await?;
    let data = data_dir(&endpoint.id().to_string())?;
    let (store, stored) = Store::open(&data)?;
    let home = HomeClone::spawn(Homing {
        host: String::from("test-host"),
        data_dir: data.clone(),
        remote: Some(remote),
        roots,
    });
    let log = Log::spawn(String::from("test-host"), Epoch(11));
    let launchers = claude_only(fake_claude())?;
    let fixture =
        serve_home(endpoint, setup(launchers, store, stored), log, home)
            .await?;
    Ok((fixture, data))
}

/// A host whose Codex app-server is the stand-in, playing `recording` from
/// the directory it returns, and whose Claude is unavailable.
async fn codex_host(recording: &str) -> Fallible<(Fixture, PathBuf)> {
    let server = scratch(recording, recording)?;
    let hub = Hub::spawn(ServerOptions {
        codex_path: fake_codex(),
        home: Home::User,
        credential: None,
        cwd: server.clone(),
        stdout_log: None,
    });
    let fixture = host_of(launchers([
        (
            Provider::Claude,
            Launcher::Unavailable(String::from(UNAVAILABLE)),
        ),
        (
            Provider::Codex,
            Launcher::Ready {
                start: codex_cli::starter(hub),
                own: OwnSignIn::Unmanaged,
                offer: offer(models()),
            },
        ),
    ]))
    .await?;
    Ok((fixture, server))
}

/// [`codex_host`], whose Codex runs as the instance of the gateway's
/// account, which its store keeps.
async fn codex_api_host(recording: &str) -> Fallible<(Fixture, PathBuf)> {
    let server = scratch(recording, recording)?;
    let hub = Hub::spawn(ServerOptions {
        codex_path: fake_codex(),
        home: Home::User,
        credential: None,
        cwd: server.clone(),
        stdout_log: None,
    });
    let endpoint = endpoint().await?;
    let data = data_dir(&endpoint.id().to_string())?;
    let _ = seed(&data, &[api(Provider::Codex)])?;
    let mut running = launchers([
        (
            Provider::Claude,
            Launcher::Unavailable(String::from(UNAVAILABLE)),
        ),
        (
            Provider::Codex,
            Launcher::Unavailable(String::from(UNAVAILABLE)),
        ),
    ]);
    running.insert(
        api(Provider::Codex),
        Launcher::Ready {
            start: codex_cli::starter(hub),
            own: OwnSignIn::Unmanaged,
            offer: offer(models()),
        },
    );
    Ok((serve(endpoint, running, &data).await?, server))
}

/// What a host on `data` runs: the Codex app-server, playing in `server`,
/// as the instance of the gateway's account, with no sign-in of its own,
/// which the store keeps; and no own instance.
fn codex_api_launchers(data: &Path, server: &Path) -> Fallible<Launchers> {
    let codex_api = api(Provider::Codex);
    let _ = seed(data, std::slice::from_ref(&codex_api))?;
    let home = Home::private(
        data,
        &host::registry::dir(&codex_api),
        &codex::profile().config,
    )?;
    let hub = Hub::spawn(ServerOptions {
        codex_path: fake_codex(),
        home,
        credential: None,
        cwd: server.to_path_buf(),
        stdout_log: None,
    });
    let mut running = launchers([
        (
            Provider::Claude,
            Launcher::Unavailable(String::from(UNAVAILABLE)),
        ),
        (
            Provider::Codex,
            Launcher::Unavailable(String::from(UNAVAILABLE)),
        ),
    ]);
    running.insert(
        codex_api,
        Launcher::Ready {
            start: codex_cli::starter(hub),
            own: OwnSignIn::Missing,
            offer: offer(models()),
        },
    );
    Ok(running)
}

/// A host with a data dir of its own.
async fn host_of(launchers: Launchers) -> Fallible<Fixture> {
    let endpoint = endpoint().await?;
    let data = data_dir(&endpoint.id().to_string())?;
    serve(endpoint, launchers, &data).await
}

/// `tmp` in the build directory, found from this test binary at
/// `target/debug/deps/`, so scratch stays out of `/tmp` and `cargo clean` or
/// removing the worktree clears it.
fn target_tmp() -> Fallible<PathBuf> {
    let binary = std::env::current_exe()?;
    let dir = binary
        .ancestors()
        .nth(3)
        .ok_or("the test binary is not in target/debug/deps")?
        .join("tmp");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// An empty data dir for a host. Its name has `conductor-test` in it, so
/// the fake CLIs show the paths under it (fake-cli.py).
fn data_dir(name: &str) -> Fallible<PathBuf> {
    let dir = target_tmp()?
        .join(format!("conductor-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// A host on `endpoint`, keeping its credentials in `data`.
async fn serve(
    endpoint: Endpoint,
    launchers: Launchers,
    data: &Path,
) -> Fallible<Fixture> {
    let log = Log::spawn(String::from("test-host"), Epoch(11));
    serve_log(endpoint, launchers, data, log).await
}

async fn serve_log(
    endpoint: Endpoint,
    launchers: Launchers,
    data: &Path,
    log: Log,
) -> Fallible<Fixture> {
    let (store, stored) = Store::open(data)?;
    serve_setup(endpoint, setup(launchers, store, stored), log).await
}

/// A host with no home.
async fn serve_setup(
    endpoint: Endpoint,
    setup: Setup,
    log: Log,
) -> Fallible<Fixture> {
    let home = homeless(setup.store.dir());
    serve_home(endpoint, setup, log, home).await
}

/// The home task of a host given no home.
fn homeless(data: &Path) -> HomeClone {
    HomeClone::spawn(Homing {
        host: String::from("test-host"),
        data_dir: data.to_path_buf(),
        remote: None,
        roots: Vec::new(),
    })
}

async fn serve_home(
    endpoint: Endpoint,
    setup: Setup,
    log: Log,
    home: HomeClone,
) -> Fallible<Fixture> {
    let registry = Registry::spawn(log.clone(), setup);
    let _ = tokio::time::timeout(STEP, registry.instances())
        .await
        .map_err(
            |_| "the registry did not restore within the step timeout",
        )??;
    let target = Target {
        addr: endpoint.addr(),
        token: AuthToken::from_bytes(TOKEN),
    };
    let conductor = Conductor {
        log: log.clone(),
        registry,
        home,
        token: AuthToken::from_bytes(TOKEN),
    };
    let router = Router::builder(endpoint)
        .accept(proto::ALPN, conductor)
        .spawn();
    Ok(Fixture {
        router,
        target,
        log,
    })
}

/// A real host that reopens its database, endpoint identity and ticket secret.
async fn durable_host(data: &Path, address: SocketAddr) -> Fallible<Fixture> {
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .secret_key(daemon::load_or_create_key(data)?)
        .bind_addr(address)?
        .bind()
        .await?;
    let token = daemon::load_or_create_token(data)?;
    let log =
        Log::open(String::from("test-host"), &data.join("conductor.redb"))?;
    let (store, stored) = Store::open(data)?;
    let registry = Registry::spawn(
        log.clone(),
        setup(
            launchers([
                (
                    Provider::Claude,
                    Launcher::Ready {
                        start: claude_cli::starter(ClaudeLaunch {
                            claude_path: fake_claude(),
                            home: Home::User,
                            credential: None,
                            probe: data.join("probe"),
                        }),
                        own: OwnSignIn::Unmanaged,
                        offer: offer(models()),
                    },
                ),
                (
                    Provider::Codex,
                    Launcher::Unavailable(String::from(UNAVAILABLE)),
                ),
            ]),
            store,
            stored,
        ),
    );
    let _ = tokio::time::timeout(STEP, registry.instances())
        .await
        .map_err(
            |_| "the registry did not restore within the step timeout",
        )??;
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
                home: homeless(data),
                token,
            },
        )
        .spawn();
    Ok(Fixture {
        router,
        target,
        log,
    })
}

async fn link(
    target: Target,
    resume: Option<Cursor>,
) -> Fallible<(HostLink, mpsc::Receiver<Update>)> {
    let options = LinkOptions {
        resume,
        network: Network::Local,
        ..LinkOptions::new(ClientName(String::from("test-client")))
    };
    Ok(HostLink::spawn(target, options).await?)
}

async fn next(updates: &mut mpsc::Receiver<Update>) -> Fallible<Update> {
    tokio::time::timeout(STEP, updates.recv())
        .await
        .map_err(|_| "no update within the step timeout")?
        .ok_or_else(|| "the link stopped".into())
}

/// The updates before the link went live.
async fn until_live(
    updates: &mut mpsc::Receiver<Update>,
) -> Fallible<Vec<Update>> {
    let mut seen = Vec::new();
    loop {
        match next(updates).await? {
            Update::Status(Status {
                state: LinkState::Live,
                ..
            }) => break Ok(seen),
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
async fn events_through(
    updates: &mut mpsc::Receiver<Update>,
    last: HostSeq,
) -> Fallible<Vec<Event>> {
    let mut events = Vec::new();
    loop {
        match next(updates).await? {
            Update::Event(event) => match event.seq.cmp(&last) {
                Ordering::Less => events.push(event),
                Ordering::Equal | Ordering::Greater => {
                    events.push(event);
                    break Ok(events);
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
        instance: instance(Provider::Claude),
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

async fn host() -> Fallible<Fixture> {
    host_with(fake_claude()).await
}

fn start(provider: Provider, directory: &Path) -> CommandBody {
    CommandBody::StartSession {
        instance: instance(provider),
        at: StartAt::Directory(HostPath(directory.display().to_string())),
        model: None,
        effort: None,
    }
}

/// What every test host's CLIs offer: a default with two efforts and a
/// model with one.
fn offer(listed: Models) -> Offer {
    Offer {
        listed,
        named: None,
        chosen: None,
    }
}

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
async fn a_client_goes_live_and_start_session_is_checked_at_the_boundary()
-> Fallible {
    let fx = host().await?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    assert_eq!(
        states(until_live(&mut updates).await?),
        [
            LinkState::Connecting,
            LinkState::Authenticating,
            LinkState::CatchingUp
        ]
    );
    assert_eq!(link.status().host, Some(String::from("test-host")));
    assert_eq!(
        link.status().instances,
        Some(statuses(BTreeMap::from([
            (
                Provider::Claude,
                Availability::Available {
                    auth: Auth::Unmanaged,
                    models: models(),
                }
            ),
            (
                Provider::Codex,
                Availability::Unavailable {
                    reason: String::from(UNAVAILABLE),
                }
            )
        ])))
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
            body: start(Provider::Claude, &std::env::current_exe()?),
            result: CommandResult::Rejected(Rejection::NoSuchDirectory),
        },
        Case {
            body: start(Provider::Codex, &std::env::temp_dir()),
            result: CommandResult::Rejected(Rejection::InstanceUnavailable {
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
    let results = n0_future::join_all(
        cases.iter().map(|case| link.send(case.body.clone())),
    )
    .await;
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
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn reconnect_mid_feed_resumes_after_the_cursor_without_gaps_or_duplicates()
-> Fallible {
    let fx = host().await?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    append(&fx.log, Vec::from([created(), reply("Hel"), reply("lo")])).await?;
    let before = events_through(&mut updates, HostSeq(3)).await?;

    append(&fx.log, Vec::from([reply(" wor"), reply("ld")])).await?;
    link.reconnect().await;
    append(&fx.log, Vec::from([reply("!")])).await?;
    let after = events_through(&mut updates, HostSeq(6)).await?;
    let seen = before.into_iter().chain(after).collect::<Vec<Event>>();
    assert_eq!(seqs(&seen), [1, 2, 3, 4, 5, 6].map(HostSeq));
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_new_client_resumes_from_a_dead_clients_cursor() -> Fallible {
    let fx = host().await?;
    let (first, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    append(&fx.log, Vec::from([created(), reply("Hel")])).await?;
    events_through(&mut updates, HostSeq(2)).await?;
    first.stop().await?;
    drop(updates);

    append(&fx.log, Vec::from([reply("Hello")])).await?;
    let cursor = Cursor {
        epoch: Epoch(11),
        seq: HostSeq(2),
    };
    let (second, mut updates) = link(fx.target.clone(), Some(cursor)).await?;
    let rest = events_through(&mut updates, HostSeq(3)).await?;
    assert_eq!(seqs(&rest), [HostSeq(3)]);
    second.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_live_client_sees_the_deltas_and_a_resumed_one_only_the_message()
-> Fallible {
    let fx = host().await?;
    let (first, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    append(&fx.log, Vec::from([created()])).await?;
    let seen = events_through(&mut updates, HostSeq(1)).await?;
    append(
        &fx.log,
        Vec::from([delta("Hel"), delta("lo"), reply("Hello")]),
    )
    .await?;
    let live = events_through(&mut updates, HostSeq(2)).await?;
    assert_eq!(seqs(&live), [1, 1, 2].map(HostSeq));
    assert_eq!(
        of(SessionId(1), &live),
        [delta("Hel"), delta("lo"), reply("Hello")]
    );
    first.stop().await?;
    drop(updates);

    let cursor = Cursor {
        epoch: Epoch(11),
        seq: HostSeq(1),
    };
    let (second, mut updates) = link(fx.target.clone(), Some(cursor)).await?;
    let rest = events_through(&mut updates, HostSeq(2)).await?;
    assert_eq!(of(SessionId(1), &rest), [reply("Hello")]);
    assert_eq!(
        fold(seen.iter().chain(&live).cloned().collect()),
        fold(seen.into_iter().chain(rest).collect())
    );
    second.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_cursor_from_another_epoch_starts_over_from_the_beginning() -> Fallible
{
    let fx = host().await?;
    append(&fx.log, Vec::from([created(), delta("H"), reply("Hi")])).await?;
    let stale = Cursor {
        epoch: Epoch(10),
        seq: HostSeq(40),
    };
    let (link, mut updates) = link(fx.target.clone(), Some(stale)).await?;
    let before_live = until_live(&mut updates).await?;
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
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// Git commands use this test repository's identity, never the user's config.
fn git(directory: &Path, arguments: &[&str]) -> Fallible<String> {
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
        .output()?;
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

fn committed_repository(directory: &Path) -> Fallible {
    git(directory, &["init", "--quiet"])?;
    std::fs::write(directory.join("baseline.txt"), "committed baseline\n")?;
    git(directory, &["add", "baseline.txt"])?;
    git(
        directory,
        &[
            "commit",
            "--quiet",
            "-m",
            "test baseline\n\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>",
        ],
    )?;
    Ok(())
}

/// A session directory holding `fixtures/e2e/<recording>` as the fake's
/// `stdout.jsonl`.
fn scratch(name: &str, recording: &str) -> Fallible<PathBuf> {
    let dir = target_tmp()?
        .join(format!("conductor-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/e2e");
    std::fs::copy(fixture.join(recording), dir.join("stdout.jsonl"))?;
    committed_repository(&dir)?;
    Ok(dir)
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
    Removed,
    Finished,
}

enum Reached {
    Yes,
    No,
}

const fn reached(until: &Until, kind: &EventKind) -> Reached {
    match (until, kind) {
        (Until::Ready, EventKind::SessionReady)
        | (Until::Request, EventKind::RequestOpened { .. })
        | (Until::Question, EventKind::QuestionAsked { .. })
        | (Until::Resolved, EventKind::RequestResolved { .. })
        | (Until::Delta, EventKind::AssistantDelta { .. })
        | (Until::TurnEnd, EventKind::TurnCompleted { .. })
        | (Until::Closed, EventKind::SessionClosed { .. })
        | (Until::Undone, EventKind::TurnsUndone { .. })
        | (Until::UndoFailure, EventKind::UndoFailed { .. })
        | (Until::Removed, EventKind::SessionRemoved { .. })
        | (Until::Finished, EventKind::SessionFinished { .. }) => Reached::Yes,
        _ => Reached::No,
    }
}

/// The kinds of the events the link receives, through the first that
/// reaches `until`, for tests that start one session.
async fn kinds_until(
    updates: &mut mpsc::Receiver<Update>,
    until: Until,
) -> Fallible<Vec<EventKind>> {
    Ok(legacy_events_until(updates, until)
        .await?
        .into_iter()
        .map(|event| event.kind)
        .collect())
}

/// Preserve existing protocol assertions while workspace metadata has dedicated
/// raw-event coverage below.
async fn legacy_events_until(
    updates: &mut mpsc::Receiver<Update>,
    until: Until,
) -> Fallible<Vec<Event>> {
    Ok(events_until(updates, until)
        .await?
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
        .collect())
}

/// The events the link receives, through the first that reaches `until`.
async fn events_until(
    updates: &mut mpsc::Receiver<Update>,
    until: Until,
) -> Fallible<Vec<Event>> {
    let mut events = Vec::new();
    loop {
        match next(updates).await? {
            Update::Event(event) => {
                let done = reached(&until, &event.kind);
                events.push(event);
                match done {
                    Reached::Yes => break Ok(events),
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

async fn send(link: &HostLink, body: CommandBody) -> Fallible<CommandResult> {
    Ok(tokio::time::timeout(STEP, link.send(body))
        .await
        .map_err(|_| "no command result within the step timeout")??)
}

fn started_session(result: &CommandResult) -> Fallible<SessionId> {
    match result {
        CommandResult::Started { session } => Ok(*session),
        other => {
            Err(format!("expected a started session, not {other:?}").into())
        }
    }
}

fn message(session: SessionId, text: &str) -> CommandBody {
    CommandBody::SendMessage {
        session,
        text: String::from(text),
    }
}

const fn interrupt(session: SessionId) -> CommandBody {
    CommandBody::Interrupt { session }
}

const fn resolve(
    session: SessionId,
    request: u64,
    decision: Decision,
) -> CommandBody {
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
            instance: instance(provider),
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

const fn turn_end(outcome: TurnOutcome) -> EventKind {
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

fn decided(
    decision: Decision,
    call: &str,
    outcome: ToolOutcome,
) -> Vec<EventKind> {
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
        .map_or_else(Vec::new, |found| found.items.clone())
}

fn done(text: &str) -> Item {
    Item::Reply {
        text: String::from(text),
        state: ReplyState::Done,
    }
}

/// A Codex command the user decided on, as the client shows it.
fn codex_tool(
    call: &str,
    command: &str,
    decision: Decision,
    outcome: ToolOutcome,
) -> Item {
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
async fn a_claude_turn_streams_asks_and_finishes() -> Fallible {
    let fx = host().await?;
    let dir = scratch("approve", "claude-approve.jsonl")?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&link, start(Provider::Claude, &dir)).await?)?;
    assert_eq!(
        send(&link, message(session, "create a.txt")).await?,
        CommandResult::Accepted
    );
    assert_eq!(
        send(&link, message(session, "and b.txt")).await?,
        CommandResult::Rejected(Rejection::TurnInProgress)
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Request).await?,
        [
            opening(Provider::Claude, &dir),
            Vec::from([user("create a.txt")]),
            write_asked(APPROVED_CALL, "/tmp/s1-scratch/a.txt"),
        ]
        .concat()
    );
    let desk_options = LinkOptions {
        network: Network::Local,
        ..LinkOptions::new(ClientName(String::from("desk")))
    };
    let (desk, mut at_desk) =
        HostLink::spawn(fx.target.clone(), desk_options).await?;
    until_live(&mut at_desk).await?;
    assert_eq!(
        [
            send(&link, resolve(session, 1, Decision::Approve)).await?,
            send(&desk, resolve(session, 1, Decision::Deny)).await?,
            send(&link, resolve(session, 2, Decision::Approve)).await?,
        ],
        [
            CommandResult::Accepted,
            CommandResult::Rejected(Rejection::AlreadyResolved {
                by: test_client()
            }),
            CommandResult::Rejected(Rejection::UnknownRequest),
        ]
    );
    let text = "I created `a.txt` in `/tmp/s1-scratch` with the content `hello from s1`.";
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await?,
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
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn removing_a_session_stops_it_and_removes_its_workspace() -> Fallible {
    let fx = host().await?;
    let dir = scratch("remove", "claude-approve.jsonl")?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&link, start(Provider::Claude, &dir)).await?)?;
    let opened = events_until(&mut updates, Until::Ready).await?;
    let worktree = opened
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::WorkspaceCreated { workspace } => {
                Some(PathBuf::from(&workspace.worktree.0))
            }
            _ => None,
        })
        .ok_or("the session made no workspace")?;
    assert!(worktree.is_dir());
    assert_eq!(
        send(&link, CommandBody::RemoveSession { session }).await?,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Removed).await?,
        [
            EventKind::SessionClosed {
                reason: CloseReason::Stopped {
                    reason: String::from(host::session::REMOVED),
                },
            },
            EventKind::SessionRemoved {
                kept: Some(Kept::Local),
            },
        ]
    );
    assert!(!worktree.exists());
    assert_eq!(
        send(&link, CommandBody::Continue { session }).await?,
        CommandResult::Rejected(Rejection::SessionRemoved)
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_denied_request_continues_and_an_interrupt_ends_the_turn() -> Fallible
{
    let fx = host().await?;
    let dir = scratch("deny", "claude-deny-interrupt.jsonl")?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&link, start(Provider::Claude, &dir)).await?)?;
    assert_eq!(
        send(&link, interrupt(session)).await?,
        CommandResult::Rejected(Rejection::NoTurnRunning)
    );
    let _ = send(&link, message(session, "create b.txt")).await?;
    assert_eq!(
        kinds_until(&mut updates, Until::Request).await?,
        [
            opening(Provider::Claude, &dir),
            Vec::from([user("create b.txt")]),
            write_asked(DENIED_CALL, "/tmp/s1-scratch/b.txt"),
        ]
        .concat()
    );
    assert_eq!(
        send(&link, resolve(session, 1, Decision::Deny)).await?,
        CommandResult::Accepted
    );
    let text = "The write to `b.txt` was denied (\"The user does not want b.txt.\"), so I did not retry and no file was created.";
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await?,
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

    let _ = send(&link, message(session, "count to 100")).await?;
    assert_eq!(
        kinds_until(&mut updates, Until::Delta).await?,
        [user("count to 100"), delta("1")]
    );
    assert_eq!(
        send(&link, interrupt(session)).await?,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await?,
        [reply("1"), turn_end(TurnOutcome::Interrupted)]
    );

    let _ = send(&link, message(session, "ping")).await?;
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await?,
        [
            user("ping"),
            delta("pong"),
            reply("pong"),
            turn_end(TurnOutcome::Completed)
        ]
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// The recording starts the turn only once `release` exists, and the fake
/// answers only an interrupt sent after that (Codex's of that turn's id), so
/// this passes only if the session holds the interrupt until the turn starts.
async fn held_interrupt(
    fx: Fixture,
    dir: PathBuf,
    provider: Provider,
) -> Fallible {
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session = started_session(&send(&link, start(provider, &dir)).await?)?;
    let _ = send(&link, message(session, "count to 300")).await?;
    assert_eq!(
        send(&link, interrupt(session)).await?,
        CommandResult::Accepted
    );
    std::fs::write(dir.join("release"), "")?;
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await?,
        [
            opening(provider, &dir),
            Vec::from([
                user("count to 300"),
                turn_end(TurnOutcome::Interrupted)
            ]),
        ]
        .concat()
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn an_interrupt_before_the_turn_starts_is_held_until_it_does() -> Fallible
{
    let dir = scratch("held", "claude-held-interrupt.jsonl")?;
    held_interrupt(host().await?, dir, Provider::Claude).await
}

/// The CLI withdraws the open request when the turn is interrupted, and the
/// tool call it asked about fails.
#[tokio::test]
async fn an_interrupt_withdraws_the_open_request() -> Fallible {
    let fx = host().await?;
    let dir = scratch("withdraw", "claude-withdraw.jsonl")?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&link, start(Provider::Claude, &dir)).await?)?;
    let _ = send(&link, message(session, "create d.txt")).await?;
    let call = "toolu_bdrk_01KEnaUr83k9BPcLoGpArHUq";
    assert_eq!(
        kinds_until(&mut updates, Until::Request).await?,
        [
            opening(Provider::Claude, &dir),
            Vec::from([user("create d.txt")]),
            write_asked(call, "/tmp/s1-scratch/d.txt"),
        ]
        .concat()
    );
    assert_eq!(
        send(&link, interrupt(session)).await?,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await?,
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
        send(&link, resolve(session, 1, Decision::Approve)).await?,
        CommandResult::Rejected(Rejection::UnknownRequest)
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

fn answer(session: SessionId, request: u64, words: &[&str]) -> CommandBody {
    CommandBody::Answer {
        session,
        request: RequestId(request),
        answers: Vec::from([Answer(
            words.iter().copied().map(String::from).collect(),
        )]),
    }
}

/// A question with the options the recorded runs asked for, as the model
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
                EventKind::AssistantDelta { .. }
                    | EventKind::AssistantMessage { .. }
            )
        })
        .collect()
}

/// What the session wrote to the stand-in's stdin.
fn sent_to_agent(dir: &Path) -> Fallible<String> {
    Ok(std::fs::read_to_string(dir.join("stdin.jsonl"))?)
}

/// Claude asks with `AskUserQuestion`. The first client's answer goes back
/// in the tool's input; a second client, a decision and the wrong number of
/// answers are refused.
#[tokio::test]
async fn a_claude_question_is_answered_once_by_the_first_client() -> Fallible {
    let fx = host().await?;
    let dir = scratch("question", "claude-question.jsonl")?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&link, start(Provider::Claude, &dir)).await?)?;
    let _ = send(&link, message(session, "ask me")).await?;
    assert_eq!(
        kinds_until(&mut updates, Until::Question).await?,
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
    let desk_options = LinkOptions {
        network: Network::Local,
        ..LinkOptions::new(ClientName(String::from("desk")))
    };
    let (desk, mut at_desk) =
        HostLink::spawn(fx.target.clone(), desk_options).await?;
    until_live(&mut at_desk).await?;
    assert_eq!(
        [
            send(&link, resolve(session, 1, Decision::Approve)).await?,
            send(
                &link,
                CommandBody::Answer {
                    session,
                    request: RequestId(1),
                    answers: Vec::new(),
                }
            )
            .await?,
            send(&link, answer(session, 1, &["Blue"])).await?,
            send(&desk, answer(session, 1, &["Red"])).await?,
        ],
        [
            CommandResult::Rejected(Rejection::WrongReply {
                reason: String::from(
                    "a question takes answers, not a decision"
                ),
            }),
            CommandResult::Rejected(Rejection::WrongReply {
                reason: String::from("0 answers for 1 questions"),
            }),
            CommandResult::Accepted,
            CommandResult::Rejected(Rejection::AlreadyResolved {
                by: test_client()
            }),
        ]
    );
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await?,
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
        sent_to_agent(&dir)?
            .matches(r#""answers":{"Which color do you prefer?":"Blue"}"#)
            .count(),
        1
    );
    link.stop().await?;
    desk.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// Codex asks with `requestUserInput`, answered as an RPC result keyed by
/// the question's id, which the stand-in waits for.
#[tokio::test]
async fn a_codex_question_is_answered_by_its_question_id() -> Fallible {
    let (fx, dir) = codex_host("codex-question.jsonl").await?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&link, start(Provider::Codex, &dir)).await?)?;
    let _ = send(&link, message(session, "ask me")).await?;
    assert_eq!(
        kinds_until(&mut updates, Until::Question).await?,
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
        send(&link, answer(session, 1, &["Blue"])).await?,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await?,
        [
            answered_blue(),
            delta("Blue"),
            reply("Blue"),
            turn_end(TurnOutcome::Completed),
        ]
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// Approving for the session adds the CLI's suggested rule for the session
/// only, so the same command runs again without a request.
#[tokio::test]
async fn a_claude_call_approved_for_the_session_runs_again_without_asking()
-> Fallible {
    let fx = host().await?;
    let dir = scratch("for-session", "claude-for-session.jsonl")?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&link, start(Provider::Claude, &dir)).await?)?;
    let _ = send(&link, message(session, "touch t1.txt")).await?;
    let (first, again) = (
        "toolu_bdrk_0194hqw8EXmFKbEqNMeNMkWf",
        "toolu_bdrk_015276Epur1cWgLdAb2EXFPF",
    );
    let touch = ToolKind::Command {
        command: String::from("touch t1.txt"),
    };
    assert_eq!(
        kinds_until(&mut updates, Until::Request).await?,
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
        send(&link, resolve(session, 1, Decision::ApproveForSession)).await?,
        CommandResult::Accepted
    );
    let first_turn = kinds_until(&mut updates, Until::TurnEnd).await?;
    let _ = send(&link, message(session, "again")).await?;
    let second_turn = kinds_until(&mut updates, Until::TurnEnd).await?;
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
        sent_to_agent(&dir)?
            .matches(r#""updatedPermissions":[{"behavior":"allow","destination":"session","rules":[{"ruleContent":"touch t1.txt","toolName":"Bash"}],"type":"addRules"}]"#)
            .count(),
        1
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// Codex takes `acceptForSession` for a command, which the stand-in waits
/// for, and runs the same command again without a request.
#[tokio::test]
async fn a_codex_command_accepted_for_the_session_runs_again_without_asking()
-> Fallible {
    let (fx, dir) = codex_host("codex-for-session.jsonl").await?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&link, start(Provider::Codex, &dir)).await?)?;
    let _ = send(&link, message(session, "touch t1.txt")).await?;
    let command = "/usr/bin/zsh -c 'touch t1.txt'";
    let (first, again) = (
        "exec-6a70037b-22b5-482b-955f-e442841e7e21",
        "exec-0e77cebc-6940-4892-b90c-558be7ccb2b4",
    );
    let touch = ToolKind::Command {
        command: String::from(command),
    };
    let asked =
        calls_and_requests(kinds_until(&mut updates, Until::Request).await?);
    assert_eq!(
        send(&link, resolve(session, 1, Decision::ApproveForSession)).await?,
        CommandResult::Accepted
    );
    let first_turn = kinds_until(&mut updates, Until::TurnEnd).await?;
    let _ = send(&link, message(session, "again")).await?;
    let second_turn = kinds_until(&mut updates, Until::TurnEnd).await?;
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
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// An interrupt withdraws an open question, as it does an approval: Claude
/// cancels it, Codex withdraws it after the turn ends, and either way the
/// view shows it withdrawn and a late answer is refused.
#[tokio::test]
async fn an_interrupt_withdraws_an_open_question() -> Fallible {
    let fx = host().await?;
    let dir = scratch("question-withdraw", "claude-question-withdraw.jsonl")?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&link, start(Provider::Claude, &dir)).await?)?;
    let _ = send(&link, message(session, "ask me")).await?;
    let mut events = legacy_events_until(&mut updates, Until::Question).await?;
    assert_eq!(
        send(&link, interrupt(session)).await?,
        CommandResult::Accepted
    );
    events.extend(legacy_events_until(&mut updates, Until::TurnEnd).await?);
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
            finished(
                "toolu_bdrk_01Ctjtd63ZgJ5uKQpWhcgJUa",
                ToolOutcome::Failed
            ),
            turn_end(TurnOutcome::Interrupted),
        ]
    );
    assert_eq!(
        send(&link, answer(session, 1, &["Pear"])).await?,
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
            | Item::Finished(_)
            | Item::Error(_) => None,
        })
        .collect::<Vec<RequestState>>();
    assert_eq!(questions, [RequestState::Resolved(Resolution::Withdrawn)]);
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_codex_interrupt_withdraws_the_open_question_at_the_turn_end()
-> Fallible {
    let (fx, dir) = codex_host("codex-question-withdraw.jsonl").await?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&link, start(Provider::Codex, &dir)).await?)?;
    let _ = send(&link, message(session, "ask me")).await?;
    let question = kinds_until(&mut updates, Until::Question).await?;
    assert_eq!(
        send(&link, interrupt(session)).await?,
        CommandResult::Accepted
    );
    assert_eq!(
        (
            question.last().cloned(),
            kinds_until(&mut updates, Until::Resolved).await?
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
        send(&link, answer(session, 1, &["Pear"])).await?,
        CommandResult::Rejected(Rejection::UnknownRequest)
    );
    std::fs::write(dir.join("release"), "")?;
    assert_eq!(sent_to_agent(&dir)?.matches("\"answers\"").count(), 0);
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// The stand-in stops reading after `initialize`, so the message can't
/// reach it. The turn must end, or every client shows it running forever.
#[tokio::test]
async fn a_message_the_agent_never_gets_fails_its_turn() -> Fallible {
    let fx = host().await?;
    let dir = scratch("hang-up", "claude-hang-up.jsonl")?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&link, start(Provider::Claude, &dir)).await?)?;
    assert_eq!(
        send(&link, message(session, "hi")).await?,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await?,
        [
            opening(Provider::Claude, &dir),
            Vec::from([
                user("hi"),
                turn_end(TurnOutcome::Failed {
                    message: String::from(
                        "claude i/o: Broken pipe (os error 32)"
                    )
                }),
            ]),
        ]
        .concat()
    );
    assert_eq!(
        send(&link, interrupt(session)).await?,
        CommandResult::Rejected(Rejection::NoTurnRunning)
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Closed).await?,
        [EventKind::SessionClosed {
            reason: CloseReason::Exited {
                status: String::from("exit status: 0"),
            },
        }]
    );
    assert_eq!(
        send(&link, message(session, "hi")).await?,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_missing_claude_binary_closes_the_session() -> Fallible {
    let fx = host_with(PathBuf::from("/nonexistent/claude")).await?;
    let dir = scratch("missing-binary", "claude-approve.jsonl")?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&link, start(Provider::Claude, &dir)).await?)?;
    assert_eq!(
        kinds_until(&mut updates, Until::Closed).await?,
        [
            EventKind::SessionCreated {
                instance: instance(Provider::Claude),
                provider: Provider::Claude,
                directory: HostPath(dir.display().to_string()),
                by: test_client(),
                model: Some(ModelId(String::from("big"))),
                effort: None,
            },
            EventKind::SessionClosed {
                reason: CloseReason::FailedToStart {
                    message: String::from(
                        "claude i/o: No such file or directory (os error 2)"
                    ),
                },
            },
        ]
    );
    assert_eq!(
        send(&link, message(session, "hi")).await?,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// Two sessions' turns run at once on one app-server: each session's events
/// are its own thread's, one approves and the other declines.
#[tokio::test]
async fn two_codex_sessions_share_one_server() -> Fallible {
    let (fx, dir) = codex_host("codex-two-threads.jsonl").await?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let first_session =
        started_session(&send(&link, start(Provider::Codex, &dir)).await?)?;
    // The second session starts once the first has its thread, so the first
    // gets the recording's first thread.
    let first = legacy_events_until(&mut updates, Until::Ready).await?;
    let second_session =
        started_session(&send(&link, start(Provider::Codex, &dir)).await?)?;
    assert_ne!(first_session, second_session);
    let second = legacy_events_until(&mut updates, Until::Ready).await?;
    assert_eq!(
        [
            send(&link, message(first_session, "create a.txt")).await?,
            send(&link, message(second_session, "create b.txt")).await?,
        ],
        [CommandResult::Accepted, CommandResult::Accepted]
    );
    let asked = [
        legacy_events_until(&mut updates, Until::Request).await?,
        legacy_events_until(&mut updates, Until::Request).await?,
    ]
    .concat();
    assert_eq!(
        [
            send(&link, resolve(first_session, 1, Decision::Approve)).await?,
            send(&link, resolve(second_session, 1, Decision::Deny)).await?,
        ],
        [CommandResult::Accepted, CommandResult::Accepted]
    );
    let ended = [
        legacy_events_until(&mut updates, Until::TurnEnd).await?,
        legacy_events_until(&mut updates, Until::TurnEnd).await?,
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
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_codex_interrupt_before_the_turn_starts_is_sent_by_turn_id()
-> Fallible {
    let (fx, dir) = codex_host("codex-held-interrupt.jsonl").await?;
    held_interrupt(fx, dir, Provider::Codex).await
}

#[tokio::test]
async fn a_codex_interrupt_mid_stream_ends_the_turn_and_the_next_runs()
-> Fallible {
    let (fx, dir) = codex_host("codex-interrupt.jsonl").await?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&link, start(Provider::Codex, &dir)).await?)?;
    let _ = send(&link, message(session, "count to 300")).await?;
    assert_eq!(
        kinds_until(&mut updates, Until::Delta).await?,
        [
            opening(Provider::Codex, &dir),
            Vec::from([user("count to 300"), delta("1")]),
        ]
        .concat()
    );
    assert_eq!(
        send(&link, interrupt(session)).await?,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await?,
        [delta("\n"), turn_end(TurnOutcome::Interrupted)]
    );
    let _ = send(&link, message(session, "ping")).await?;
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await?,
        [
            user("ping"),
            delta("pong"),
            reply("pong"),
            turn_end(TurnOutcome::Completed)
        ]
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// Codex withdraws the open request only after the interrupted turn ends,
/// and the recording holds that withdrawal until `release` exists. The
/// session withdraws the request at the turn's end, so an answer in between
/// is refused and never reaches Codex.
#[tokio::test]
async fn a_codex_interrupt_withdraws_the_open_request_at_the_turn_end()
-> Fallible {
    let (fx, dir) = codex_host("codex-withdraw.jsonl").await?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&link, start(Provider::Codex, &dir)).await?)?;
    let _ = send(&link, message(session, "create d.txt")).await?;
    let call =
        ToolCallId(String::from("exec-c1e73e4d-1cac-431e-baf8-8bf5b779606b"));
    let tool = ToolKind::Command {
        command: String::from("/usr/bin/zsh -lc 'printf %s d > d.txt'"),
    };
    assert_eq!(
        kinds_until(&mut updates, Until::Request).await?,
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
                    for_session: Some(String::from(
                        "/usr/bin/zsh -lc 'printf %s d > d.txt'"
                    )),
                },
            ]),
        ]
        .concat()
    );
    assert_eq!(
        send(&link, interrupt(session)).await?,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Resolved).await?,
        [
            turn_end(TurnOutcome::Interrupted),
            EventKind::RequestResolved {
                request: RequestId(1),
                resolution: Resolution::Withdrawn,
            },
        ]
    );
    assert_eq!(
        send(&link, resolve(session, 1, Decision::Approve)).await?,
        CommandResult::Rejected(Rejection::UnknownRequest)
    );
    std::fs::write(dir.join("release"), "")?;
    // Codex's withdrawal plays before the next turn and is ignored.
    let _ = send(&link, message(session, "ping")).await?;
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd).await?,
        [
            user("ping"),
            delta("pong"),
            reply("pong"),
            turn_end(TurnOutcome::Completed)
        ]
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("stdin.jsonl"))?
            .matches("\"decision\"")
            .count(),
        0
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// The stand-in exits once both threads have started. Both sessions close,
/// and the next session runs on a new app-server.
#[tokio::test]
async fn the_app_server_exiting_closes_every_codex_session() -> Fallible {
    let (fx, dir) = codex_host("codex-exit.jsonl").await?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let first_session =
        started_session(&send(&link, start(Provider::Codex, &dir)).await?)?;
    let first = legacy_events_until(&mut updates, Until::Ready).await?;
    let second_session =
        started_session(&send(&link, start(Provider::Codex, &dir)).await?)?;
    assert_ne!(first_session, second_session);
    let seen = [
        first,
        legacy_events_until(&mut updates, Until::Closed).await?,
        legacy_events_until(&mut updates, Until::Closed).await?,
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
            [opening(Provider::Codex, &dir), Vec::from([closed.clone()])]
                .concat(),
            [opening(Provider::Codex, &dir), Vec::from([closed])].concat(),
        )
    );
    assert_eq!(
        send(&link, message(first_session, "hi")).await?,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    let next_session =
        started_session(&send(&link, start(Provider::Codex, &dir)).await?)?;
    assert_ne!(next_session, first_session);
    assert_ne!(next_session, second_session);
    assert_eq!(
        kinds_until(&mut updates, Until::Ready).await?,
        opening(Provider::Codex, &dir)
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// The states before the link reported an error, and the error.
async fn until_error(
    updates: &mut mpsc::Receiver<Update>,
) -> Fallible<(Vec<LinkState>, String)> {
    let mut seen = Vec::new();
    loop {
        match next(updates).await? {
            Update::Status(Status {
                error: Some(error), ..
            }) => break Ok((seen, error)),
            Update::Status(status) => seen.push(status.state),
            Update::Reset(_) | Update::Event(_) => {}
        }
    }
}

#[tokio::test]
async fn a_wrong_token_is_refused_and_never_goes_live() -> Fallible {
    let fx = host().await?;
    let target = Target {
        token: AuthToken::from_bytes([0; 16]),
        ..fx.target.clone()
    };
    let (link, mut updates) = link(target, None).await?;
    assert_eq!(
        until_error(&mut updates).await?,
        (
            Vec::from([LinkState::Connecting, LinkState::Authenticating]),
            String::from("the host refused us: wrong token")
        )
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
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
    async fn spawn(host: SocketAddr) -> Fallible<SocketAddr> {
        let outer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let inner = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        inner.connect(host).await?;
        let addr = outer.local_addr()?;
        let hop = Self {
            outer,
            inner,
            dead: None,
            live: None,
        };
        tokio::spawn(hop.run());
        Ok(addr)
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
            (Datagram::FromClient { bytes, from }, Some(dead), _)
                if from != dead =>
            {
                self.live = Some(from);
                let _ = self.inner.send(&bytes).await?;
                Ok(())
            }
            (Datagram::FromClient { .. }, Some(_), _)
            | (Datagram::FromHost(_), _, None) => Ok(()),
            (Datagram::FromHost(bytes), _, Some(live)) => {
                let _ = self.outer.send_to(&bytes, live).await?;
                Ok(())
            }
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
fn loopback(endpoint: &Endpoint) -> Fallible<SocketAddr> {
    let bound = endpoint
        .bound_sockets()
        .into_iter()
        .find(|addr| matches!(addr, SocketAddr::V4(_)))
        .ok_or("the endpoint has no IPv4 socket")?;
    Ok(SocketAddr::from((Ipv4Addr::LOCALHOST, bound.port())))
}

#[tokio::test]
async fn a_stalled_dial_goes_live_through_a_fresh_endpoint_and_stops_cleanly()
-> Fallible {
    let (log_path, _logging) = capture_log("stalled-dial")?;
    let fx = host().await?;
    let hop = Hop::spawn(loopback(fx.router.endpoint())?).await?;
    let target = Target {
        addr: EndpointAddr::new(fx.target.addr.id).with_ip_addr(hop),
        ..fx.target.clone()
    };
    let (link, mut updates) = link(target, None).await?;

    // No deadline: only the hedge can take the link live, and STEP bounds each update.
    let before_live = until_live(&mut updates).await?;
    assert_eq!(
        states(before_live),
        [
            LinkState::Connecting,
            LinkState::Authenticating,
            LinkState::CatchingUp
        ]
    );
    link.stop().await?;
    drop(updates);
    fx.router.shutdown().await?;

    let logged = captured(&log_path)?;
    assert_eq!(
        (
            logged.matches("hedged dial connected").count(),
            logged.matches(UNGRACEFUL).count()
        ),
        (1, 0),
        "{logged}"
    );
    Ok(())
}

/// Sends this thread's logs, at debug and above, to a file named after
/// `test`. A `tokio::test` runs on one thread, so they include the host's.
fn capture_log(
    test: &str,
) -> Fallible<(PathBuf, tracing::subscriber::DefaultGuard)> {
    let path = target_tmp()?
        .join(format!("conductor-e2e-{test}-{}.log", std::process::id()));
    let log = std::fs::File::create(&path)?;
    let guard = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_writer(Arc::new(log))
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .finish(),
    );
    Ok((path, guard))
}

/// The captured log, whose file is then removed.
fn captured(path: &Path) -> Fallible<String> {
    let logged = std::fs::read_to_string(path)?;
    std::fs::remove_file(path)?;
    Ok(logged)
}

const GATEWAY: &str = "https://conductor-test.invalid/gateway";
const CLAUDE_TOKEN: &str = "conductor-test-claude-token";
const NO_CREDENTIAL: &str = "no credential configured";
const UNPROTECTED: &str = "the CLI's commands can read the gateway token; install bubblewrap and socat to sandbox them provider=Claude";

fn read(path: &Path) -> Fallible<String> {
    Ok(std::fs::read_to_string(path)?)
}

/// How many times `text` is in the file at `path`.
fn count(path: &Path, text: &str) -> Fallible<usize> {
    Ok(std::fs::read(path)?
        .windows(text.len())
        .filter(|window| window == &text.as_bytes())
        .count())
}

/// A host on `data` whose own Claude is the fake in a private home, with
/// no sign-in of its own, and whose Codex is unavailable. The instance of
/// a credential's account runs on the fake as `conductord host --cli` runs
/// one, with the credential the store holds, and lists `PROBED`.
async fn claude_host_in(data: &Path) -> Fallible<Fixture> {
    let home = Home::private(data, "claude", &claude::profile().config)?;
    std::fs::create_dir_all(data.join("probe"))?;
    std::fs::write(data.join("probe/stdout.jsonl"), format!("{PROBED}\n"))?;
    probed_in(data, &host::registry::dir(&api(Provider::Claude)))?;
    let plugins = fake_plugins(data);
    let mut launchers = launchers([
        (
            Provider::Claude,
            Launcher::Ready {
                start: claude_cli::starter(ClaudeLaunch {
                    claude_path: fake_claude(),
                    home,
                    credential: None,
                    probe: data.join("probe"),
                }),
                own: OwnSignIn::Missing,
                offer: offer(models()),
            },
        ),
        (
            Provider::Codex,
            Launcher::Unavailable(String::from(UNAVAILABLE)),
        ),
    ]);
    let (store, stored) = Store::open(data)?;
    for (id, configured) in stored.roster(&catalog()).0 {
        if let std::collections::btree_map::Entry::Vacant(vacant) =
            launchers.entry(id.clone())
        {
            vacant.insert(
                daemon::start::launcher(&plugins, data, id, configured).await,
            );
        }
    }
    let setup = Setup {
        builder: daemon::start::builder(plugins, data.to_path_buf()),
        ..setup(launchers, store, stored)
    };
    serve_setup(
        endpoint().await?,
        setup,
        Log::spawn(String::from("test-host"), Epoch(11)),
    )
    .await
}

fn with_claude(claude: Availability) -> Instances {
    statuses(BTreeMap::from([
        (Provider::Claude, claude),
        (
            Provider::Codex,
            Availability::Unavailable {
                reason: String::from(UNAVAILABLE),
            },
        ),
    ]))
}

fn gateway(models: Models) -> Availability {
    Availability::Available {
        auth: Auth::Stored {
            kind: String::from("gateway"),
            url: Some(String::from(GATEWAY)),
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
    CommandResult::Rejected(Rejection::InstanceUnavailable {
        reason: String::from(NO_CREDENTIAL),
    })
}

fn claude_gateway() -> CommandBody {
    CommandBody::SetCredential {
        provider: Provider::Claude,
        credential: claude_credential(CLAUDE_TOKEN),
    }
}

/// A session of `instance` in `directory`.
fn start_on(instance: InstanceId, directory: &Path) -> CommandBody {
    CommandBody::StartSession {
        instance,
        at: StartAt::Directory(HostPath(directory.display().to_string())),
        model: None,
        effort: None,
    }
}

/// A fresh link to `fx`, live, and the providers its `Welcome` reported.
async fn live(
    fx: &Fixture,
) -> Fallible<(HostLink, mpsc::Receiver<Update>, Option<Instances>)> {
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let instances = link.status().instances;
    Ok((link, updates, instances))
}

#[tokio::test]
async fn a_claude_gateway_reaches_the_cli_survives_a_restart_and_is_removed()
-> Fallible {
    let data = data_dir("claude-gateway")?;
    let (log, logging) = capture_log("claude-gateway")?;
    let dir = scratch("claude-gateway", "claude-approve.jsonl")?;
    let fx = claude_host_in(&data).await?;
    let (link, mut updates, welcomed) = live(&fx).await?;
    let claude_api = api(Provider::Claude);
    let with_api =
        and(with_claude(no_credential()), &claude_api, gateway(probed()));
    assert_eq!(welcomed, Some(with_claude(no_credential())));
    assert_eq!(
        send(&link, start(Provider::Claude, &dir)).await?,
        refused_for_no_credential()
    );
    assert_eq!(
        send(&link, claude_gateway()).await?,
        CommandResult::Instances(with_api.clone())
    );
    assert_eq!(
        (
            link.status().instances,
            read(&dir_of(&data, "work", &claude_api).join("env.txt"))?
                .contains(GATEWAY)
        ),
        (Some(with_api.clone()), true),
        "the gateway's account is an instance, its models listed with the credential"
    );
    let _ = started_session(
        &send(&link, start_on(claude_api.clone(), &dir)).await?,
    )?;
    let events = legacy_events_until(&mut updates, Until::Ready).await?;
    assert_eq!(
        read(&dir.join("env.txt"))?,
        format!(
            "ANTHROPIC_AUTH_TOKEN={CLAUDE_TOKEN}\nANTHROPIC_BASE_URL={GATEWAY}\n{}CLAUDE_CONFIG_DIR={}\n",
            match claude_cli::sandbox() {
                Sandbox::Available => "CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1\n",
                Sandbox::Missing => "",
            },
            dir_of(&data, "home", &claude_api).display()
        )
    );
    assert_eq!(count(&data.join("credentials"), CLAUDE_TOKEN)?, 0);
    link.stop().await?;
    fx.router.shutdown().await?;

    let fx = claude_host_in(&data).await?;
    let (link, _updates, welcomed) = live(&fx).await?;
    assert_eq!(welcomed, Some(with_api));
    assert_eq!(
        send(
            &link,
            CommandBody::RemoveInstance {
                instance: claude_api.clone(),
            }
        )
        .await?,
        CommandResult::Instances(with_claude(no_credential()))
    );
    assert_eq!(
        send(&link, start_on(claude_api, &dir)).await?,
        CommandResult::Rejected(Rejection::UnknownInstance)
    );
    link.stop().await?;
    fx.router.shutdown().await?;

    let fx = claude_host_in(&data).await?;
    let (link, _updates, welcomed) = live(&fx).await?;
    assert_eq!(welcomed, Some(with_claude(no_credential())));
    link.stop().await?;
    fx.router.shutdown().await?;
    drop(logging);
    let logged = captured(&log)?;
    assert_eq!(
        (
            logged.matches("added an instance").count(),
            logged.matches(CLAUDE_TOKEN).count(),
            format!("{events:?}").matches(CLAUDE_TOKEN).count(),
            logged.matches(UNPROTECTED).count(),
        ),
        (
            1,
            0,
            0,
            // Once when the gateway is set, once when the restart finds it.
            match claude_cli::sandbox() {
                Sandbox::Available => 0,
                Sandbox::Missing => 2,
            }
        )
    );
    Ok(())
}

fn with_codex(codex: Availability) -> Instances {
    statuses(BTreeMap::from([
        (
            Provider::Claude,
            Availability::Unavailable {
                reason: String::from(UNAVAILABLE),
            },
        ),
        (Provider::Codex, codex),
    ]))
}

/// How Codex signs in and the models it offers, by id, with the default
/// first, from a host's answer to a credential change.
fn codex_offer(
    result: CommandResult,
) -> Option<(Auth, Option<String>, Vec<String>)> {
    match result {
        CommandResult::Instances(Instances(mut instances)) => {
            match instances
                .remove(&api(Provider::Codex))
                .map(|it| it.availability)
            {
                Some(Availability::Available { auth, models }) => Some((
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
fn codex_gateway() -> (Auth, Option<String>, Vec<String>) {
    (
        Auth::Stored {
            kind: String::from("provider"),
            url: Some(String::from(GATEWAY)),
        },
        Some(String::from("conductor-test-model")),
        LISTED.iter().copied().map(String::from).collect(),
    )
}

fn codex_provider(key: &str) -> CommandBody {
    CommandBody::SetCredential {
        provider: Provider::Codex,
        credential: credential(
            "provider",
            &[
                ("name", "Conductor test"),
                ("base_url", GATEWAY),
                ("key", key),
                ("model", "conductor-test-model"),
            ],
        ),
    }
}

#[tokio::test]
async fn a_codex_provider_reaches_the_app_server_and_a_new_one_restarts_it()
-> Fallible {
    const FIRST_KEY: &str = "conductor-test-first-key";
    const SECOND_KEY: &str = "conductor-test-second-key";
    let data = data_dir("codex-provider")?;
    let (log, logging) = capture_log("codex-provider")?;
    let server = scratch("codex-provider", "codex-two-threads-listed.jsonl")?;
    let codex_api = api(Provider::Codex);
    let fx = serve(
        endpoint().await?,
        codex_api_launchers(&data, &server)?,
        &data,
    )
    .await?;
    let (link, mut updates, welcomed) = live(&fx).await?;
    assert_eq!(
        welcomed,
        Some(and(
            with_codex(Availability::Unavailable {
                reason: String::from(UNAVAILABLE),
            }),
            &codex_api,
            no_credential()
        ))
    );
    assert_eq!(
        codex_offer(send(&link, codex_provider(FIRST_KEY)).await?),
        Some(codex_gateway())
    );
    let _ = started_session(
        &send(&link, start_on(codex_api.clone(), &server)).await?,
    )?;
    let first = legacy_events_until(&mut updates, Until::Ready).await?;
    let home = dir_of(&data, "home", &codex_api);
    assert_eq!(
        read(&server.join("env.txt"))?,
        format!(
            "CODEX_HOME={}\nCONDUCTOR_CODEX_KEY={FIRST_KEY}\n",
            home.display()
        )
    );
    assert_eq!(
        read(&server.join("codex-config.toml"))?,
        format!(
            "check_for_update_on_startup = false\nmodel_provider = \"conductor\"\nmodel = \"conductor-test-model\"\n\n[model_providers.conductor]\nname = \"Conductor test\"\nbase_url = \"{GATEWAY}\"\nenv_key = \"CONDUCTOR_CODEX_KEY\"\nwire_api = \"responses\"\n\n[shell_environment_policy.filters]\nCONDUCTOR_CODEX_KEY = \"exclude\"\n"
        )
    );
    assert_eq!(
        codex_offer(send(&link, codex_provider(SECOND_KEY)).await?),
        Some(codex_gateway())
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Closed).await?,
        [EventKind::SessionClosed {
            reason: CloseReason::Stopped {
                reason: String::from("new Codex credential"),
            },
        }]
    );
    let _ = started_session(&send(&link, start_on(codex_api, &server)).await?)?;
    let second = legacy_events_until(&mut updates, Until::Ready).await?;
    assert_eq!(
        read(&server.join("env.txt"))?,
        format!(
            "CODEX_HOME={}\nCONDUCTOR_CODEX_KEY={SECOND_KEY}\n",
            home.display()
        )
    );
    assert_eq!(count(&home.join("config.toml"), FIRST_KEY)?, 0);
    link.stop().await?;
    fx.router.shutdown().await?;
    drop(logging);
    let logged = captured(&log)?;
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
    Ok(())
}

#[tokio::test]
async fn a_credential_sent_before_hello_is_refused_and_not_stored() -> Fallible
{
    let data = data_dir("unauthenticated")?;
    let fx = claude_host_in(&data).await?;
    let endpoint = endpoint().await?;
    let conn = endpoint
        .connect(fx.target.addr.clone(), proto::ALPN)
        .await?;
    let (mut send_half, mut recv_half) = conn.open_bi().await?;
    net::wire::write(
        &mut send_half,
        &proto::Open::Command(proto::Command {
            id: proto::CommandId(1),
            body: claude_gateway(),
        }),
    )
    .await?;
    assert_eq!(
        net::wire::read::<proto::HelloReply>(&mut recv_half).await?,
        Some(proto::HelloReply::Refused {
            reason: String::from("the first stream must be a hello"),
        })
    );
    endpoint.close().await;
    let (link, _updates, welcomed) = live(&fx).await?;
    assert_eq!(
        (welcomed, data.join("credentials").exists()),
        (Some(with_claude(no_credential())), false)
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn an_unreadable_store_keeps_the_host_up_refuses_a_set_and_moves_aside()
-> Fallible {
    let data = data_dir("unreadable")?;
    // A store key of its own, then a store that doesn't open with it.
    let _ = Store::open(&data)?;
    std::fs::write(data.join("credentials"), b"not a sealed store")?;
    let fx = claude_host_in(&data).await?;
    let (link, _updates, welcomed) = live(&fx).await?;
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
    assert_eq!(send(&link, claude_gateway()).await?, refused);
    assert_eq!(
        read(&data.join("credentials"))?,
        "not a sealed store",
        "a refused set leaves the file as it was"
    );
    assert_eq!(
        send(
            &link,
            CommandBody::RemoveInstance {
                instance: instance(Provider::Codex),
            }
        )
        .await?,
        CommandResult::Instances(with_claude(no_credential()))
    );
    assert_eq!(
        read(&data.join("credentials.unreadable"))?,
        "not a sealed store"
    );
    assert_eq!(
        send(&link, claude_gateway()).await?,
        CommandResult::Instances(and(
            with_claude(no_credential()),
            &api(Provider::Claude),
            gateway(probed())
        ))
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_base_url_that_could_hide_a_secret_is_refused() -> Fallible {
    let data = data_dir("base-url")?;
    let fx = claude_host_in(&data).await?;
    let (link, _updates, _) = live(&fx).await?;
    let body = CommandBody::SetCredential {
        provider: Provider::Claude,
        credential: credential(
            "gateway",
            &[
                ("base_url", "https://gateway.example/?key=conductor-test"),
                ("token", CLAUDE_TOKEN),
            ],
        ),
    };
    assert_eq!(
        (send(&link, body).await?, data.join("credentials").exists()),
        (
            CommandResult::Rejected(Rejection::InvalidCredential {
                reason: String::from(
                    "Gateway URL may not carry a user, a password, a query or a fragment"
                ),
            }),
            false
        )
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// The plugins `conductord host --cli claude=<the stand-in>` runs, with a
/// Codex that doesn't.
fn fake_plugins(data: &Path) -> daemon::start::Plugins {
    daemon::start::Plugins::from_iter([
        claude_cli::plugin(Settings {
            path: Some(fake_claude()),
            model: None,
        }),
        codex_cli::plugin(Settings {
            path: Some(data.join("no-codex")),
            model: None,
        }),
    ])
}

/// Makes the Claude stand-in list `PROBED` in the work dir of `name`.
fn probed_in(data: &Path, name: &str) -> Fallible {
    let work = data.join("work").join(name);
    std::fs::create_dir_all(&work)?;
    std::fs::write(work.join("stdout.jsonl"), format!("{PROBED}\n"))?;
    Ok(())
}

/// A host on `data` as `conductord host` starts one: each instance its
/// store holds runs on its plugin, and an added one starts the same way.
async fn plugin_host(data: &Path) -> Fallible<Fixture> {
    let log = Log::spawn(String::from("test-host"), Epoch(11));
    plugin_host_logging(data, log).await
}

/// [`plugin_host`], writing its events to `log`.
async fn plugin_host_logging(data: &Path, log: Log) -> Fallible<Fixture> {
    let plugins = fake_plugins(data);
    let (store, stored) = Store::open(data)?;
    let mut launchers = Launchers::new();
    for (id, configured) in stored.roster(&catalog()).0 {
        let started =
            daemon::start::launcher(&plugins, data, id.clone(), configured);
        launchers.insert(id, started.await);
    }
    let setup = Setup {
        launchers,
        store,
        stored,
        catalog: catalog(),
        builder: daemon::start::builder(plugins.clone(), data.to_path_buf()),
        signer: daemon::start::signer(plugins, data.to_path_buf()),
    };
    serve_setup(endpoint().await?, setup, log).await
}

fn status_of(
    result: &CommandResult,
    id: &InstanceId,
) -> Option<InstanceStatus> {
    match result {
        CommandResult::Instances(listed) => listed.of(id).cloned(),
        _ => None,
    }
}

/// The session each `SessionCreated` names, with its instance.
fn created_on(events: &[Event]) -> Vec<(SessionId, InstanceId)> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::SessionCreated { instance, .. } => {
                Some((event.session, instance.clone()))
            }
            _ => None,
        })
        .collect()
}

/// What the Claude stand-in reads of a gateway at `base_url` with `token`
/// in the home `home`.
fn gateway_env(token: &str, base_url: &str, home: &Path) -> String {
    format!(
        "ANTHROPIC_AUTH_TOKEN={token}\nANTHROPIC_BASE_URL={base_url}\n{}CLAUDE_CONFIG_DIR={}\n",
        match claude_cli::sandbox() {
            Sandbox::Available => "CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1\n",
            Sandbox::Missing => "",
        },
        home.display()
    )
}

/// Two instances of Claude run side by side, one for each gateway account:
/// each has its own credential, its own private home and its own agents,
/// and a session runs on the instance it was started on, which can't be
/// removed while it runs.
#[tokio::test]
async fn two_claude_accounts_keep_their_own_credentials_and_homes() -> Fallible
{
    const API_TOKEN: &str = "conductor-test-api-token";
    const OTHER_TOKEN: &str = "conductor-test-other-token";
    const OTHER: &str = "https://conductor-test.invalid/other";
    let data = data_dir("two-claudes")?;
    let on_api = scratch("two-claudes-api", "claude-approve.jsonl")?;
    let on_other = scratch("two-claudes-other", "claude-approve.jsonl")?;
    let claude_api = api(Provider::Claude);
    let other = InstanceId::named(Provider::Claude, OTHER);
    probed_in(&data, &host::registry::dir(&claude_api))?;
    probed_in(&data, &host::registry::dir(&other))?;
    let fx = plugin_host(&data).await?;
    let (link, mut updates, _) = live(&fx).await?;
    let set = |base_url: &str, token: &str| CommandBody::SetCredential {
        provider: Provider::Claude,
        credential: credential(
            "gateway",
            &[("base_url", base_url), ("token", token)],
        ),
    };
    let after_api = send(&link, set(GATEWAY, API_TOKEN)).await?;
    let after_other = send(&link, set(OTHER, OTHER_TOKEN)).await?;
    let other_gateway = Availability::Available {
        auth: Auth::Stored {
            kind: String::from("gateway"),
            url: Some(String::from(OTHER)),
        },
        models: probed(),
    };
    assert_eq!(
        (
            status_of(&after_api, &claude_api).map(|it| it.availability),
            status_of(&after_api, &other),
            status_of(&after_other, &other).map(|it| it.availability),
        ),
        (Some(gateway(probed())), None, Some(other_gateway))
    );
    let first = started_session(
        &send(&link, start_on(claude_api.clone(), &on_api)).await?,
    )?;
    let api_events = events_until(&mut updates, Until::Ready).await?;
    let second = started_session(
        &send(&link, start_on(other.clone(), &on_other)).await?,
    )?;
    let other_events = events_until(&mut updates, Until::Ready).await?;
    assert_eq!(
        (
            created_on(&api_events),
            created_on(&other_events),
            read(&on_api.join("env.txt"))?,
            read(&on_other.join("env.txt"))?,
        ),
        (
            Vec::from([(first, claude_api.clone())]),
            Vec::from([(second, other.clone())]),
            gateway_env(
                API_TOKEN,
                GATEWAY,
                &dir_of(&data, "home", &claude_api)
            ),
            gateway_env(OTHER_TOKEN, OTHER, &dir_of(&data, "home", &other)),
        )
    );
    assert_eq!(
        (
            send(
                &link,
                CommandBody::RemoveInstance {
                    instance: claude_api.clone()
                }
            )
            .await?,
            dir_of(&data, "home", &claude_api)
                .join("settings.json")
                .exists(),
        ),
        (
            CommandResult::Rejected(Rejection::InstanceInUse { sessions: 1 }),
            true,
        )
    );
    link.stop().await?;
    fx.shutdown().await?;
    Ok(())
}

fn start_with(
    provider: Provider,
    directory: &Path,
    model: &str,
    effort: Effort,
) -> CommandBody {
    CommandBody::StartSession {
        instance: instance(provider),
        at: StartAt::Directory(HostPath(directory.display().to_string())),
        model: Some(ModelId(String::from(model))),
        effort: Some(effort),
    }
}

const fn continued(session: SessionId) -> CommandBody {
    CommandBody::Continue { session }
}

/// The arguments of each run of the Claude stand-in in `dir`.
fn argv(dir: &Path) -> Fallible<Vec<Vec<String>>> {
    Ok(read(&dir.join("argv.txt"))?
        .split("run\n")
        .filter(|run| !run.is_empty())
        .map(|run| run.lines().map(String::from).collect())
        .collect())
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
fn requests(
    dir: &Path,
    log: &str,
    method: &str,
) -> Fallible<Vec<serde_json::Value>> {
    let parsed = read(&dir.join(log))?
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<serde_json::Value>, serde_json::Error>>()?;
    Ok(parsed
        .into_iter()
        .filter(|request| {
            request.get("method").and_then(|m| m.as_str()) == Some(method)
        })
        .filter_map(|request| request.get("params").cloned())
        .collect())
}

fn created_with(
    provider: Provider,
    directory: &Path,
    model: &str,
    effort: Effort,
) -> EventKind {
    EventKind::SessionCreated {
        instance: instance(provider),
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
async fn start_session_takes_only_a_model_and_effort_the_host_offers()
-> Fallible {
    let fx = host().await?;
    let dir = scratch("chosen", "claude-approve.jsonl")?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    assert_eq!(
        [
            send(
                &link,
                start_with(Provider::Claude, &dir, "huge", Effort::Low)
            )
            .await?,
            send(
                &link,
                start_with(Provider::Claude, &dir, "small", Effort::High)
            )
            .await?,
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
        &send(
            &link,
            start_with(Provider::Claude, &dir, "small", Effort::Low),
        )
        .await?,
    )?;
    assert_eq!(
        kinds_until(&mut updates, Until::Ready).await?,
        [
            created_with(Provider::Claude, &dir, "small", Effort::Low),
            EventKind::SessionReady,
        ]
    );
    let runs = argv(&dir)?;
    assert_eq!(
        runs.iter()
            .map(|run| (flag(run, "--model"), flag(run, "--effort")))
            .collect::<Vec<(Option<String>, Option<String>)>>(),
        [(Some(String::from("small")), Some(String::from("low")))]
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

const THOUGHT: &str = "\"All but 9 run away\" means 9 stay, so he buys double that, 18, bringing the total to 27, then sells a third (9), leaving 18 sheep.\n\n";

/// Reasoning streams to live clients and is stored once finished, so a
/// client that catches up later sees the finished text without deltas.
/// The CLI sends this traffic only with betas on, which the owner's
/// gateway refuses.
#[tokio::test]
async fn claude_reasoning_streams_live_and_is_stored_once_finished() -> Fallible
{
    let fx = host().await?;
    let dir = scratch("reasoning", "claude-reasoning.jsonl")?;
    let (phone, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&phone, start(Provider::Claude, &dir)).await?)?;
    let _ = send(&phone, message(session, "sheep?")).await?;
    let live = kinds_until(&mut updates, Until::TurnEnd).await?;
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
                    EventKind::ReasoningDelta { .. }
                        | EventKind::AssistantDelta { .. }
                ))
                .collect::<Vec<EventKind>>()
        ),
        (String::from(THOUGHT), stored.clone())
    );
    let (late, mut late_updates) = link(fx.target.clone(), None).await?;
    let caught_up = until_live(&mut late_updates)
        .await?
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
    late.stop().await?;
    phone.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// `probe-subagent-todo.jsonl`: the main agent's `Agent` call runs a
/// sub-agent, which calls `TodoWrite` twice and reports "ok". Its events are
/// the session's, and its checklist is not the session's tasks.
#[tokio::test]
async fn a_claude_subagent_runs_in_its_session_under_the_call_that_started_it()
-> Fallible {
    let fx = host().await?;
    let dir = scratch("subagent", "claude-subagent.jsonl")?;
    let (phone, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&phone, start(Provider::Claude, &dir)).await?)?;
    let _ = send(&phone, message(session, "delegate")).await?;
    let events = legacy_events_until(&mut updates, Until::TurnEnd).await?;
    let agent = || SubagentId(String::from("af19920e77caaf1de"));
    let call = || ToolCallId(String::from("toolu_015av4j2jHs5CwVCv76qxYf2"));
    let description = || String::from("Run TodoWrite checklist task");
    let todo_done = |id: &str| EventKind::Subagent {
        agent: agent(),
        event: SubagentEvent::Activity(SubagentActivity::ToolCall {
            call: ToolCallId(String::from(id)),
            phase: ToolPhase::Finished {
                outcome: ToolOutcome::Succeeded,
            },
        }),
    };
    assert_eq!(
        of(session, &events)
            .into_iter()
            .filter(|kind| !matches!(kind, EventKind::AssistantDelta { .. }))
            .collect::<Vec<EventKind>>(),
        [
            opening(Provider::Claude, &dir),
            Vec::from([
                user("delegate"),
                EventKind::ToolCall {
                    call: call(),
                    phase: ToolPhase::Started {
                        tool: ToolKind::Other {
                            name: String::from("Agent"),
                            detail: description(),
                        },
                    },
                },
                EventKind::Subagent {
                    agent: agent(),
                    event: SubagentEvent::Started {
                        call: call(),
                        description: description()
                    }
                },
                todo_done("toolu_01WSCsnAQWyaEonxDVnmncap"),
                todo_done("toolu_01H4nEBx6jVuR4PwHBWBgvPw"),
                EventKind::Subagent {
                    agent: agent(),
                    event: SubagentEvent::Ended {
                        outcome: SubagentOutcome::Finished,
                        summary: Some(String::from("ok"))
                    }
                },
                finished(
                    "toolu_015av4j2jHs5CwVCv76qxYf2",
                    ToolOutcome::Succeeded,
                ),
                reply("done"),
                turn_end(TurnOutcome::Completed),
            ]),
        ]
        .concat()
    );
    let view = fold(events);
    assert_eq!(
        view.sessions
            .get(&session)
            .map(|shown| (shown.tasks.clone(), shown.subagents.clone())),
        Some((
            Vec::new(),
            Vec::from([Subagent {
                id: agent(),
                call: call(),
                description: description(),
                status: SubagentStatus::Ended(SubagentOutcome::Finished),
                items: Vec::from([done("ok")]),
            }])
        ))
    );
    phone.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// Through the owner's gateway, with betas off, the CLI streams an empty
/// thinking block (`claude-killed.jsonl`'s first turn), which shows nothing.
#[tokio::test]
async fn empty_gateway_thinking_logs_no_reasoning_and_folds_to_no_disclosure()
-> Fallible {
    let fx = host().await?;
    let dir = scratch("betas-off", "claude-killed.jsonl")?;
    let (phone, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&phone, start(Provider::Claude, &dir)).await?)?;
    let _ = send(&phone, message(session, "sheep?")).await?;
    let turn = legacy_events_until(&mut updates, Until::TurnEnd).await?;
    assert_eq!(
        (
            turn.iter()
                .filter(|event| matches!(
                    event.kind,
                    EventKind::Reasoning { .. }
                        | EventKind::ReasoningDelta { .. }
                ))
                .count(),
            items(&fold(turn), session)
                .into_iter()
                .filter(|item| matches!(
                    item,
                    Item::Reasoning { .. } | Item::Reply { .. }
                ))
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
    phone.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// A Claude CLI killed mid-turn closes the session; `Continue` starts it
/// again with `--resume` on the same conversation, model and effort, and
/// the next turn runs.
#[tokio::test]
async fn a_killed_claude_session_continues_its_conversation() -> Fallible {
    let fx = host().await?;
    let dir = scratch("killed", "claude-killed.jsonl")?;
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/claude-resumed.jsonl"),
        dir.join("stdout.2.jsonl"),
    )?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    assert_eq!(
        send(&link, continued(SessionId(1))).await?,
        CommandResult::Rejected(Rejection::UnknownSession)
    );
    let session = started_session(
        &send(
            &link,
            start_with(Provider::Claude, &dir, "small", Effort::Low),
        )
        .await?,
    )?;
    let _ = send(&link, message(session, "sheep?")).await?;
    let first = legacy_events_until(&mut updates, Until::TurnEnd).await?;
    assert_eq!(
        send(&link, continued(session)).await?,
        CommandResult::Accepted
    );
    let _ = send(&link, message(session, "count")).await?;
    let killed = legacy_events_until(&mut updates, Until::Closed).await?;
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
        send(&link, message(session, "hello?")).await?,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    assert_eq!(
        send(&link, continued(session)).await?,
        CommandResult::Accepted
    );
    let ready = legacy_events_until(&mut updates, Until::Ready).await?;
    let _ = send(&link, message(session, "how many sheep?")).await?;
    let after = legacy_events_until(&mut updates, Until::TurnEnd).await?;
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
    let runs = argv(&dir)?;
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
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// A `Continue` whose transcript is gone leaves the session closed, with
/// the CLI's own reason (the recorded `result` of the pinned CLI).
#[tokio::test]
async fn a_continue_with_no_transcript_stays_closed_with_the_clis_reason()
-> Fallible {
    let fx = host().await?;
    let dir = scratch("resume-missing", "claude-killed.jsonl")?;
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/claude-resume-missing.jsonl"),
        dir.join("stdout.2.jsonl"),
    )?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&link, start(Provider::Claude, &dir)).await?)?;
    let _ = send(&link, message(session, "sheep?")).await?;
    let _ = legacy_events_until(&mut updates, Until::TurnEnd).await?;
    let _ = send(&link, message(session, "count")).await?;
    let _ = legacy_events_until(&mut updates, Until::Closed).await?;
    assert_eq!(
        send(&link, continued(session)).await?,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Closed).await?,
        [EventKind::SessionClosed {
            reason: CloseReason::FailedToStart {
                message: String::from(
                    "claude exited: No conversation found with session ID: 5b0b7a52-1c9e-4d1e-9a4e-3f2f8c1d6a10"
                ),
            },
        }]
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

const COUNTING: &str = "**Compiling numbers**\n\nI'm tasked with creating a list of 300 numbers, one per line. It sounds massive but straightforward—I'll ensure each number is on its own line. I need to focus on accuracy while keeping the format neat. There are around 600 tokens to work with overall. Luckily, it seems I won’t need any tools for this task; I just need to produce the exact output as required. Let's get started on this!";

/// The app-server killed mid-turn, as live: after `turn/started`, with
/// deltas streamed. The session closes with no failed turn, and `Continue`
/// resumes the thread.
#[tokio::test]
async fn a_codex_server_killed_mid_turn_closes_and_continues() -> Fallible {
    let (fx, server) = codex_host("codex-killed-mid-turn.jsonl").await?;
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/codex-resumed.jsonl"),
        server.join("stdout.2.jsonl"),
    )?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session = started_session(
        &send(
            &link,
            start_with(Provider::Codex, &server, "small", Effort::Low),
        )
        .await?,
    )?;
    let _ = send(&link, message(session, "prime?")).await?;
    let _ = kinds_until(&mut updates, Until::TurnEnd).await?;
    let _ = send(&link, message(session, COUNT)).await?;
    let killed = kinds_until(&mut updates, Until::Closed).await?;
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
                    EventKind::ReasoningDelta { .. }
                        | EventKind::AssistantDelta { .. }
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
        send(&link, continued(session)).await?,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Ready).await?,
        [EventKind::SessionReady]
    );
    let _ = send(&link, message(session, "codeword?")).await?;
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd)
            .await?
            .into_iter()
            .filter(|kind| !matches!(
                kind,
                EventKind::ReasoningDelta { .. }
                    | EventKind::AssistantDelta { .. }
            ))
            .collect::<Vec<EventKind>>(),
        [
            user("codeword?"),
            reply("quince"),
            turn_end(TurnOutcome::Completed),
        ]
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// A new credential that doesn't offer a Codex session's model: `Continue`
/// starts it on the new default and logs why. (It closed before its first
/// turn, so it starts a thread afresh; a resume takes the same choice.)
#[tokio::test]
async fn a_codex_session_continues_on_the_default_when_a_new_credential_drops_its_model()
-> Fallible {
    let (fx, server) = codex_api_host("codex-resumed.jsonl").await?;
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/codex-resumed-listed.jsonl"),
        server.join("stdout.2.jsonl"),
    )?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session = started_session(
        &send(
            &link,
            CommandBody::StartSession {
                instance: api(Provider::Codex),
                at: StartAt::Directory(HostPath(server.display().to_string())),
                model: Some(ModelId(String::from("small"))),
                effort: Some(Effort::Low),
            },
        )
        .await?,
    )?;
    let _ = kinds_until(&mut updates, Until::Ready).await?;
    assert_eq!(
        codex_offer(send(&link, codex_provider("conductor-test-key")).await?),
        Some(codex_gateway())
    );
    let _ = kinds_until(&mut updates, Until::Closed).await?;
    assert_eq!(
        send(&link, continued(session)).await?,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Ready).await?,
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
        requests(&server, "stdin.2.jsonl", "thread/start")?
            .iter()
            .map(thread_settings)
            .collect::<Vec<_>>(),
        [(
            None,
            Some(serde_json::json!("conductor-test-model")),
            Some(serde_json::json!("low"))
        )]
    );
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
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
async fn a_killed_codex_server_continues_the_thread() -> Fallible {
    let (fx, server) = codex_host("codex-killed.jsonl").await?;
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/codex-resumed.jsonl"),
        server.join("stdout.2.jsonl"),
    )?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session = started_session(
        &send(
            &link,
            start_with(Provider::Codex, &server, "small", Effort::Low),
        )
        .await?,
    )?;
    let _ = send(&link, message(session, "prime?")).await?;
    let first = kinds_until(&mut updates, Until::TurnEnd).await?;
    assert_eq!(
        first
            .into_iter()
            .filter(|kind| !matches!(
                kind,
                EventKind::ReasoningDelta { .. }
                    | EventKind::AssistantDelta { .. }
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
    let _ = send(&link, message(session, COUNT)).await?;
    assert_eq!(
        kinds_until(&mut updates, Until::Closed).await?,
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
        send(&link, continued(session)).await?,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Ready).await?,
        [EventKind::SessionReady]
    );
    let _ = send(&link, message(session, "codeword?")).await?;
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd)
            .await?
            .into_iter()
            .filter(|kind| !matches!(
                kind,
                EventKind::ReasoningDelta { .. }
                    | EventKind::AssistantDelta { .. }
            ))
            .collect::<Vec<EventKind>>(),
        [
            user("codeword?"),
            reply("quince"),
            turn_end(TurnOutcome::Completed),
        ]
    );
    let started = requests(&server, "stdin.jsonl", "thread/start")?;
    let resumed = requests(&server, "stdin.2.jsonl", "thread/resume")?;
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
    link.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// The original ticket and live link survive a real transport restart. The
/// saved cursor replays only restart cleanup, and Continue resumes the same
/// conversation after withdrawing an unanswered approval.
#[tokio::test]
async fn a_durable_host_restarts_with_the_same_ticket_cursor_and_conversation()
-> Fallible {
    let dir = scratch("transport-restart", "claude-approve.jsonl")?;
    let data = data_dir("transport-restart")?;
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/claude-resumed.jsonl"),
        dir.join("stdout.2.jsonl"),
    )?;
    let first =
        durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let address = first.address()?;
    let ticket = first.target.clone();
    let (phone, mut updates) = link(ticket.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&phone, start(Provider::Claude, &dir)).await?)?;
    let _ = send(&phone, message(session, "create a.txt")).await?;
    let before = legacy_events_until(&mut updates, Until::Request).await?;
    let cursor = before.last().ok_or("expected a value")?.seq;
    let conversation = argv(&dir)?
        .first()
        .and_then(|run| flag(run, "--session-id"))
        .ok_or("expected a value")?;
    first.shutdown().await?;

    let second = durable_host(&data, address).await?;
    assert_eq!(second.target.addr.id, ticket.addr.id);
    assert_eq!(second.target.token, ticket.token);
    phone.reconnect().await;
    let caught_up = until_live(&mut updates).await?;
    assert_restart_cleanup(caught_up, cursor, session)?;
    assert_eq!(
        send(&phone, resolve(session, 1, Decision::Approve)).await?,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    assert_eq!(
        send(&phone, continued(session)).await?,
        CommandResult::Accepted
    );
    assert_eq!(
        kinds_until(&mut updates, Until::Ready).await?,
        [EventKind::SessionReady]
    );
    assert_eq!(
        send(&phone, resolve(session, 1, Decision::Approve)).await?,
        CommandResult::Rejected(Rejection::UnknownRequest)
    );
    let _ = send(&phone, message(session, "how many sheep?")).await?;
    assert_eq!(
        kinds_until(&mut updates, Until::TurnEnd)
            .await?
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
        argv(&dir)?.last().and_then(|run| run
            .iter()
            .find(|argument| argument.starts_with("--resume="))),
        Some(&format!("--resume={conversation}"))
    );
    phone.stop().await?;
    second.shutdown().await?;
    Ok(())
}

/// After a restart, the replay from `cursor` holds only the cleanup: the
/// open request withdrawn, the turn interrupted with a checkpoint, and the
/// session closed. Nothing is reset.
fn assert_restart_cleanup(
    caught_up: Vec<Update>,
    cursor: HostSeq,
    session: SessionId,
) -> Fallible {
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
            HostSeq(cursor.0.checked_add(1).ok_or("expected a value")?),
            HostSeq(cursor.0.checked_add(2).ok_or("expected a value")?),
            HostSeq(cursor.0.checked_add(3).ok_or("expected a value")?),
            HostSeq(cursor.0.checked_add(4).ok_or("expected a value")?),
            HostSeq(cursor.0.checked_add(5).ok_or("expected a value")?),
        ])
    );
    assert_eq!(
        checkpoint(&cleanup, proto::CheckpointId(1))?.parent,
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
    Ok(())
}

/// A raw connection permits an explicit retry ID, including from a fresh
/// endpoint identity after the host restarts.
async fn authenticated(
    endpoint: &Endpoint,
    target: &Target,
) -> Fallible<iroh::endpoint::Connection> {
    let conn = endpoint.connect(target.addr.clone(), proto::ALPN).await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    net::wire::write(
        &mut send,
        &proto::Open::Hello(proto::Hello {
            client: test_client(),
            token: target.token,
        }),
    )
    .await?;
    send.finish()?;
    let welcome = net::wire::read::<proto::HelloReply>(&mut recv)
        .await?
        .ok_or("the host closed the stream before replying")?;
    assert_eq!(
        Some(&welcome)
            .filter(|reply| matches!(reply, proto::HelloReply::Welcome(_))),
        Some(&welcome)
    );
    Ok(conn)
}

async fn retry_command(
    conn: &iroh::endpoint::Connection,
    command: &proto::Command,
) -> Fallible<CommandResult> {
    let (mut send, mut recv) = conn.open_bi().await?;
    net::wire::write(&mut send, &proto::Open::Command(command.clone())).await?;
    send.finish()?;
    Ok(net::wire::read::<CommandResult>(&mut recv)
        .await?
        .ok_or("the host closed the stream before answering")?)
}

// The refresh tests hold a provider's listing until they write its `go`
// file, so what answers before then did not wait on the refresh.
const REFRESH_SESSION: SessionId = SessionId(2300);
const REFRESHING: &str = "refreshing the instance's models";

fn queued_command(
    conn: &iroh::endpoint::Connection,
    id: u128,
    body: CommandBody,
) -> tokio::task::JoinHandle<Fallible<CommandResult>> {
    let shared = conn.clone();
    let command = proto::Command {
        id: proto::CommandId(id),
        body,
    };
    tokio::spawn(async move { retry_command(&shared, &command).await })
}

async fn finished_command(
    task: tokio::task::JoinHandle<Fallible<CommandResult>>,
) -> Fallible<CommandResult> {
    tokio::time::timeout(STEP, task)
        .await
        .map_err(|_| "no command result within the step timeout")??
}

fn claude_credential(token: &str) -> Credential {
    credential("gateway", &[("base_url", GATEWAY), ("token", token)])
}

/// A credential of `kind`, with its form's values by field id.
fn credential(kind: &str, values: &[(&str, &str)]) -> Credential {
    Credential {
        kind: String::from(kind),
        values: values
            .iter()
            .map(|(id, value)| (String::from(*id), String::from(*value)))
            .collect(),
    }
}

/// A provider's own instance, which every host has.
fn instance(provider: Provider) -> InstanceId {
    InstanceId::own(provider)
}

/// The instance of the test gateway's account, which its credential names.
fn api(provider: Provider) -> InstanceId {
    InstanceId::named(provider, GATEWAY)
}

/// Where the host keeps `instance`'s private home or work dir.
fn dir_of(data: &Path, kind: &str, instance: &InstanceId) -> PathBuf {
    data.join(kind).join(host::registry::dir(instance))
}

/// The own instances of a host whose plugins are `catalog()`'s.
fn defaults() -> Roster {
    daemon::credentials::Stored::Fresh.roster(&catalog())
}

/// Keeps `accounts` in the store in `data`, with no credential, as a host
/// that had them.
fn seed(data: &Path, accounts: &[InstanceId]) -> Fallible<Roster> {
    let roster = Roster(
        accounts
            .iter()
            .map(|id| (id.clone(), proto::Instance::default()))
            .collect(),
    );
    Store::open(data)?.0.save(&roster)?;
    Ok(roster)
}

/// `instances` with `id` as `availability` says.
fn and(
    instances: Instances,
    id: &InstanceId,
    availability: Availability,
) -> Instances {
    let mut more = instances;
    more.0.insert(id.clone(), InstanceStatus { availability });
    more
}

/// Each provider's default instance, as the host runs it.
fn launchers<const N: usize>(by: [(Provider, Launcher); N]) -> Launchers {
    by.into_iter()
        .map(|(provider, launcher)| (instance(provider), launcher))
        .collect()
}

/// What `Welcome` says of each provider's own instance.
fn statuses(by: BTreeMap<Provider, Availability>) -> Instances {
    Instances(
        by.into_iter()
            .map(|(provider, availability)| {
                (instance(provider), InstanceStatus { availability })
            })
            .collect(),
    )
}

/// A registry over `launchers`, which adds no instance it can run.
fn setup(
    launchers: Launchers,
    store: Store,
    stored: daemon::credentials::Stored,
) -> Setup {
    Setup {
        launchers,
        store,
        stored,
        catalog: catalog(),
        builder: Builder::new(|_, _| async {
            Launcher::Unavailable(String::from(UNAVAILABLE))
        }),
        signer: Signer::new(|_, _| async { Err(String::from(UNAVAILABLE)) }),
    }
}

/// What the providers' plugins describe, as `conductord` hands them to
/// the daemon.
fn catalog() -> proto::Catalog {
    daemon::start::Plugins::from_iter([
        claude_cli::plugin(Settings::default()),
        codex_cli::plugin(Settings::default()),
    ])
    .catalog()
}

async fn drive_saved(
    data: &Path,
    expected: daemon::credentials::Stored,
) -> Fallible<daemon::credentials::Stored> {
    loop {
        tokio::time::sleep(Duration::from_millis(10)).await;
        match Store::open(data)?.1 {
            stored if stored == expected => break Ok(stored),
            _ => {}
        }
    }
}

/// Waits until the store holds `seeded` and the own instances, with the
/// credential `body` sets on the gateway's account.
async fn saved(data: &Path, seeded: &Roster, body: CommandBody) -> Fallible {
    let CommandBody::SetCredential {
        provider,
        credential,
    } = body
    else {
        return Err(format!("expected a credential, not {body:?}").into());
    };
    let mut roster = defaults();
    roster.0.extend(seeded.0.clone());
    if let Some(changed) = roster.0.get_mut(&api(provider)) {
        changed.credential = Some(credential);
    }
    let expected = daemon::credentials::Stored::Opened(roster);
    let _ = tokio::time::timeout(STEP, drive_saved(data, expected))
        .await
        .map_err(|_| "the credential was not saved in time")??;
    Ok(())
}

async fn marker(path: &Path) -> Fallible {
    tokio::time::timeout(STEP, drive_materialization(path))
        .await
        .map_err(|_| "the marker did not appear in time")??;
    Ok(())
}

async fn refresh_host(
    data: &Path,
    source: &Path,
    launchers: Launchers,
) -> Fallible<Fixture> {
    let log =
        Log::open(String::from("refresh-host"), &data.join("conductor.redb"))?;
    log.append(
        REFRESH_SESSION,
        EventKind::SessionCreated {
            instance: api(Provider::Claude),
            provider: Provider::Claude,
            directory: HostPath(source.display().to_string()),
            by: test_client(),
            model: None,
            effort: None,
        },
    )
    .await?;
    serve_log(endpoint().await?, launchers, data, log).await
}

fn refresh_claude(home: Home, data: &Path) -> Launcher {
    Launcher::Ready {
        start: claude_cli::starter(ClaudeLaunch {
            claude_path: fake_claude(),
            home,
            credential: None,
            probe: data.join("probe"),
        }),
        own: OwnSignIn::Unmanaged,
        offer: offer(models()),
    }
}

/// A probe directory whose first listing waits for `go` and then fails, and
/// whose second lists `PROBED`.
fn held_claude_probe(data: &Path) -> Fallible<PathBuf> {
    let probe = data.join("probe");
    std::fs::create_dir_all(&probe)?;
    std::fs::write(
        probe.join("stdout.jsonl"),
        "{\"fake\":\"files\",\"write\":{\"held\":\"ready\"}}\n{\"fake\":\"release\",\"file\":\"go\"}\n{\"fake\":\"fail\",\"stderr\":\"superseded probe failed\",\"status\":1}\n",
    )?;
    std::fs::write(probe.join("stdout.2.jsonl"), format!("{PROBED}\n"))?;
    Ok(probe)
}

/// While Claude refreshes, starting or continuing a Claude session is
/// refused at once, and other commands still answer.
async fn assert_refreshing_claude_refuses(
    phone: &HostLink,
    source: &Path,
) -> Fallible {
    let refused = CommandResult::Rejected(Rejection::InstanceUnavailable {
        reason: String::from(REFRESHING),
    });
    assert_eq!(
        send(phone, start_on(api(Provider::Claude), source)).await?,
        refused
    );
    assert_eq!(continue_refresh_session(phone).await?, refused);
    assert_eq!(
        send(phone, message(SessionId(999), "lookup")).await?,
        CommandResult::Rejected(Rejection::UnknownSession)
    );
    Ok(())
}

async fn continue_refresh_session(phone: &HostLink) -> Fallible<CommandResult> {
    send(
        phone,
        CommandBody::Continue {
            session: REFRESH_SESSION,
        },
    )
    .await
}

/// An event appended now reaches a live subscriber.
async fn assert_log_still_live(log: &Log) -> Fallible {
    let mut live = log.subscribe(HostSeq(0)).await?.live;
    log.append(
        REFRESH_SESSION,
        EventKind::Error {
            message: String::from("still live"),
        },
    )
    .await?;
    assert_eq!(
        tokio::time::timeout(STEP, live.recv()).await??.kind,
        EventKind::Error {
            message: String::from("still live")
        }
    );
    Ok(())
}

/// A credential whose probe fails leaves the account's instance
/// unavailable with the probe's reason; the next lists its models again.
async fn assert_failed_refresh_then_another(
    phone: &HostLink,
    probe: &Path,
) -> Fallible {
    std::fs::write(
        probe.join("stdout.3.jsonl"),
        "{\"fake\":\"fail\",\"stderr\":\"current probe failed\",\"status\":1}\n",
    )?;
    let failed = send(
        phone,
        CommandBody::SetCredential {
            provider: Provider::Claude,
            credential: claude_credential("conductor-test-failing"),
        },
    )
    .await?;
    let reason = match failed {
        CommandResult::Instances(Instances(mut instances)) => {
            match instances
                .remove(&api(Provider::Claude))
                .map(|it| it.availability)
            {
                Some(Availability::Unavailable { reason }) => Some(reason),
                _ => None,
            }
        }
        _ => None,
    }
    .ok_or("expected a value")?;
    assert_ne!(reason, REFRESHING);
    assert_ne!(reason, "");
    std::fs::write(probe.join("stdout.4.jsonl"), format!("{PROBED}\n"))?;
    assert_eq!(
        send(
            phone,
            CommandBody::SetCredential {
                provider: Provider::Claude,
                credential: claude_credential("conductor-test-again"),
            }
        )
        .await?,
        CommandResult::Instances(with_api_claude(gateway(probed())))
    );
    Ok(())
}

/// The own instances, unavailable, and Claude's of the gateway's account
/// as `availability` says.
fn with_api_claude(availability: Availability) -> Instances {
    let unavailable = || Availability::Unavailable {
        reason: String::from(UNAVAILABLE),
    };
    and(
        statuses(BTreeMap::from([
            (Provider::Claude, unavailable()),
            (Provider::Codex, unavailable()),
        ])),
        &api(Provider::Claude),
        availability,
    )
}

#[tokio::test]
async fn provider_refresh_keeps_wire_commands_live_and_coalesces_stale_claude_results()
-> Fallible {
    let data = data_dir("refresh-claude")?;
    let source = scratch("refresh-claude", "claude-approve.jsonl")?;
    let probe = held_claude_probe(&data)?;
    let claude_api = api(Provider::Claude);
    let seeded = seed(&data, std::slice::from_ref(&claude_api))?;
    let mut launchers = launchers([
        (
            Provider::Claude,
            Launcher::Unavailable(String::from(UNAVAILABLE)),
        ),
        (
            Provider::Codex,
            Launcher::Unavailable(String::from(UNAVAILABLE)),
        ),
    ]);
    launchers.insert(
        claude_api.clone(),
        refresh_claude(
            Home::private(
                &data,
                &host::registry::dir(&claude_api),
                &claude::profile().config,
            )?,
            &data,
        ),
    );
    let fx = refresh_host(&data, &source, launchers.clone()).await?;
    let desktop = endpoint().await?;
    let conn = authenticated(&desktop, &fx.target).await?;
    let first_body = CommandBody::SetCredential {
        provider: Provider::Claude,
        credential: claude_credential("conductor-test-old"),
    };
    let first = queued_command(&conn, 2301, first_body.clone());
    marker(&probe.join("held")).await?;
    let (phone, _updates, offered) = live(&fx).await?;
    assert_eq!(
        offered,
        Some(with_api_claude(Availability::Unavailable {
            reason: String::from(REFRESHING)
        }))
    );
    let duplicate = queued_command(&conn, 2301, first_body.clone());
    assert_refreshing_claude_refuses(&phone, &source).await?;
    assert_log_still_live(&fx.log).await?;
    let middle_body = CommandBody::SetCredential {
        provider: Provider::Claude,
        credential: claude_credential("conductor-test-middle"),
    };
    let middle = queued_command(&conn, 2302, middle_body.clone());
    saved(&data, &seeded, middle_body).await?;
    let newest_body = CommandBody::SetCredential {
        provider: Provider::Claude,
        credential: claude_credential("conductor-test-newest"),
    };
    let newest = queued_command(&conn, 2303, newest_body.clone());
    saved(&data, &seeded, newest_body).await?;
    assert_eq!(read(&probe.join("runs.txt"))?, "1");
    std::fs::write(probe.join("go"), "")?;
    let result = CommandResult::Instances(with_api_claude(gateway(probed())));
    assert_eq!(finished_command(first).await?, result);
    assert_eq!(finished_command(duplicate).await?, result);
    assert_eq!(finished_command(middle).await?, result);
    assert_eq!(finished_command(newest).await?, result);
    assert_eq!(read(&probe.join("runs.txt"))?, "2");
    assert_ne!(
        read(&probe.join("env.txt"))?.find("conductor-test-newest"),
        None
    );
    assert_failed_refresh_then_another(&phone, &probe).await?;
    phone.stop().await?;
    desktop.close().await;
    fx.shutdown().await?;
    let log =
        Log::open(String::from("refresh-host"), &data.join("conductor.redb"))?;
    let reopened = serve_log(endpoint().await?, launchers, &data, log).await?;
    let client = endpoint().await?;
    let conn = authenticated(&client, &reopened.target).await?;
    assert_eq!(
        retry_command(
            &conn,
            &proto::Command {
                id: proto::CommandId(2301),
                body: first_body
            }
        )
        .await?,
        result
    );
    assert_eq!(read(&probe.join("runs.txt"))?, "4");
    client.close().await;
    reopened.shutdown().await?;
    Ok(())
}

/// Makes the recording in `server` wait for `go` after its first two lines,
/// and play whole the next time.
fn hold_codex_listing(server: &Path) -> Fallible {
    let recording = read(&server.join("stdout.jsonl"))?;
    std::fs::write(server.join("stdout.2.jsonl"), &recording)?;
    std::fs::write(
        server.join("stdout.jsonl"),
        format!(
            "{}\n{{\"fake\":\"files\",\"write\":{{\"held\":\"ready\"}}}}\n{{\"fake\":\"release\",\"file\":\"go\"}}\n{}\n",
            recording.lines().take(2).collect::<Vec<_>>().join("\n"),
            recording.lines().skip(2).collect::<Vec<_>>().join("\n"),
        ),
    )?;
    Ok(())
}

/// The Codex account's instance can't be removed while its session runs.
async fn assert_running_codex_is_not_removed(phone: &HostLink) -> Fallible {
    assert_eq!(
        send(
            phone,
            CommandBody::RemoveInstance {
                instance: api(Provider::Codex),
            },
        )
        .await?,
        CommandResult::Rejected(Rejection::InstanceInUse { sessions: 1 })
    );
    Ok(())
}

#[tokio::test]
async fn provider_refresh_orders_codex_configuration_without_blocking_claude()
-> Fallible {
    let data = data_dir("refresh-codex")?;
    let source = scratch("refresh-codex-claude", "claude-approve.jsonl")?;
    let server = scratch("refresh-codex", "codex-two-threads-listed.jsonl")?;
    hold_codex_listing(&server)?;
    let (claude_api, codex_api) = (api(Provider::Claude), api(Provider::Codex));
    let seeded = seed(&data, &[claude_api.clone(), codex_api.clone()])?;
    let private = |id: &InstanceId, config: agents::Config| {
        Home::private(&data, &host::registry::dir(id), &config)
    };
    let hub = Hub::spawn(ServerOptions {
        codex_path: fake_codex(),
        home: private(&codex_api, codex::profile().config)?,
        credential: None,
        cwd: server.clone(),
        stdout_log: None,
    });
    let mut running = launchers([
        (
            Provider::Claude,
            Launcher::Unavailable(String::from(UNAVAILABLE)),
        ),
        (
            Provider::Codex,
            Launcher::Unavailable(String::from(UNAVAILABLE)),
        ),
    ]);
    running.insert(
        claude_api.clone(),
        refresh_claude(private(&claude_api, claude::profile().config)?, &data),
    );
    running.insert(
        codex_api.clone(),
        Launcher::Ready {
            start: codex_cli::starter(hub),
            own: OwnSignIn::Missing,
            offer: offer(models()),
        },
    );
    let fx = refresh_host(&data, &source, running).await?;
    let desktop = endpoint().await?;
    let conn = authenticated(&desktop, &fx.target).await?;
    let first =
        queued_command(&conn, 2311, codex_provider("conductor-test-old"));
    marker(&server.join("held")).await?;
    let (phone, mut updates, offered) = live(&fx).await?;
    assert_eq!(
        offered.ok_or("expected a value")?.availability(&codex_api),
        Some(&Availability::Unavailable {
            reason: String::from(REFRESHING)
        })
    );
    assert_eq!(
        continue_refresh_session(&phone).await?,
        CommandResult::Accepted
    );
    events_until(&mut updates, Until::Ready).await?;
    let started = send(&phone, start_on(claude_api, &source)).await?;
    let _ = started_session(&started)?;
    events_until(&mut updates, Until::Ready).await?;
    let middle_body = codex_provider("conductor-test-middle");
    let middle = queued_command(&conn, 2312, middle_body.clone());
    saved(&data, &seeded, middle_body).await?;
    let newest_body = codex_provider("conductor-test-newest");
    let newest = queued_command(&conn, 2313, newest_body.clone());
    saved(&data, &seeded, newest_body).await?;
    assert_eq!(read(&server.join("runs.txt"))?, "1");
    std::fs::write(server.join("go"), "")?;
    for task in [first, middle, newest] {
        assert_eq!(
            codex_offer(finished_command(task).await?),
            Some(codex_gateway())
        );
    }
    assert_eq!(read(&server.join("runs.txt"))?, "2");
    assert_ne!(
        read(&server.join("env.txt"))?.find("conductor-test-newest"),
        None
    );
    let _ =
        started_session(&send(&phone, start_on(codex_api, &server)).await?)?;
    events_until(&mut updates, Until::Ready).await?;
    assert_running_codex_is_not_removed(&phone).await?;
    assert_eq!(read(&server.join("runs.txt"))?, "2");
    phone.stop().await?;
    desktop.close().await;
    fx.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn one_command_receipt_survives_new_transport_identities_and_a_host_restart()
-> Fallible {
    let dir = scratch("receipt-transport", "claude-approve.jsonl")?;
    let data = data_dir("receipt-transport")?;
    let first =
        durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let address = first.address()?;
    let ticket = first.target.clone();
    let command = proto::Command {
        id: proto::CommandId(71),
        body: start(Provider::Claude, &dir),
    };
    let desktop = endpoint().await?;
    let phone = endpoint().await?;
    assert_ne!(desktop.id(), phone.id());
    let desktop_conn = authenticated(&desktop, &ticket).await?;
    let phone_conn = authenticated(&phone, &ticket).await?;
    let receipt = retry_command(&desktop_conn, &command).await?;
    let session = started_session(&receipt)?;
    assert_eq!(retry_command(&phone_conn, &command).await?, receipt);
    desktop.close().await;
    phone.close().await;
    first.shutdown().await?;

    let second = durable_host(&data, address).await?;
    let restarted_phone = endpoint().await?;
    assert_ne!(restarted_phone.id(), desktop.id());
    assert_ne!(restarted_phone.id(), phone.id());
    let conn = authenticated(&restarted_phone, &ticket).await?;
    assert_eq!(retry_command(&conn, &command).await?, receipt);
    let history = second.log.subscribe(HostSeq(0)).await?.replay;
    assert_eq!(
        history
            .iter()
            .filter(|event| matches!(
                event.kind,
                EventKind::SessionCreated { .. }
            ))
            .count(),
        1
    );
    assert_eq!(history.iter().find(|event| event.session != session), None);
    restarted_phone.close().await;
    second.shutdown().await?;
    Ok(())
}

/// Restarts the durable host `first` on `address` and waits for `phone`
/// to go live on the new one.
async fn restart(
    first: Fixture,
    data: &Path,
    address: SocketAddr,
    phone: &HostLink,
    updates: &mut mpsc::Receiver<Update>,
) -> Fallible<Fixture> {
    first.shutdown().await?;
    let second = durable_host(data, address).await?;
    phone.reconnect().await;
    until_live(updates).await?;
    Ok(second)
}

/// A Claude session in a managed worktree of `source`, after two turns.
struct TwoTurns {
    session: SessionId,
    worktree: PathBuf,
    retained: proto::Checkpoint,
    selected: proto::Checkpoint,
}

async fn two_turns(
    phone: &HostLink,
    updates: &mut mpsc::Receiver<Update>,
    source: &Path,
) -> Fallible<TwoTurns> {
    let session =
        started_session(&send(phone, start(Provider::Claude, source)).await?)?;
    let managed = workspace(&events_until(updates, Until::Ready).await?)?;
    let _ = send(phone, message(session, "turn one")).await?;
    let retained = checkpoint(
        &events_until(updates, Until::TurnEnd).await?,
        proto::CheckpointId(1),
    )?;
    let _ = send(phone, message(session, "turn two")).await?;
    let selected = checkpoint(
        &events_until(updates, Until::TurnEnd).await?,
        proto::CheckpointId(2),
    )?;
    Ok(TwoTurns {
        session,
        worktree: PathBuf::from(&managed.worktree.0),
        retained,
        selected,
    })
}

/// Whether any of `events` is of a kind `wanted` picks.
fn any_event(events: &[Event], wanted: impl Fn(&EventKind) -> bool) -> bool {
    events.iter().any(|event| wanted(&event.kind))
}

/// How many of the session's items in `view` are user messages.
fn user_items(view: &HostView, session: SessionId) -> usize {
    items(view, session)
        .iter()
        .filter(|item| matches!(item, Item::User { .. }))
        .count()
}

fn assert_absent(path: &Path) {
    assert_eq!(
        std::fs::metadata(path).err().map(|error| error.kind()),
        Some(std::io::ErrorKind::NotFound)
    );
}

/// The command is refused while an undo is unfinished.
async fn assert_undo_blocks(phone: &HostLink, body: CommandBody) -> Fallible {
    assert_eq!(
        send(phone, body).await?,
        CommandResult::Rejected(Rejection::UndoInProgress)
    );
    Ok(())
}

const fn undo_turn(
    session: SessionId,
    checkpoint: proto::CheckpointId,
) -> CommandBody {
    CommandBody::UndoTurn {
        session,
        checkpoint,
    }
}

async fn get_diff(
    phone: &HostLink,
    session: SessionId,
    checkpoint: proto::CheckpointId,
) -> Fallible<proto::CheckpointDiff> {
    diff_result(
        send(
            phone,
            CommandBody::GetDiff {
                session,
                checkpoint,
            },
        )
        .await?,
    )
}

fn assistant_messages(session: SessionId, events: &[Event]) -> Vec<EventKind> {
    of(session, events)
        .into_iter()
        .filter(|kind| matches!(kind, EventKind::AssistantMessage { .. }))
        .collect()
}

/// The undo put the retained turn's files back, and dropped the executable
/// bit the discarded turn set.
fn assert_undone_files(worktree: &Path) -> Fallible {
    assert_restored_files(worktree)?;
    assert_absent(&worktree.join("later.txt"));
    assert_eq!(
        git(worktree, &["ls-files", "--stage", "exec.sh"])?
            .split_whitespace()
            .next(),
        Some("100644")
    );
    Ok(())
}

/// A turn in the `other` session, in `other_path`, changes only its own
/// worktree.
async fn assert_independent(
    phone: &HostLink,
    updates: &mut mpsc::Receiver<Update>,
    other: SessionId,
    other_path: &Path,
    worktree: &Path,
) -> Fallible {
    assert_absent(&other_path.join("changed.txt"));
    let _ =
        send(phone, message(other, "an independent second session")).await?;
    let other_turn = events_until(updates, Until::TurnEnd).await?;
    checkpoint(&other_turn, proto::CheckpointId(1))?;
    assert_eq!(
        std::fs::read_to_string(other_path.join("after-undo.txt"))?,
        "continued from retained prefix\n"
    );
    assert_absent(&worktree.join("after-undo.txt"));
    Ok(())
}

fn checkpoint(
    events: &[Event],
    id: proto::CheckpointId,
) -> Fallible<proto::Checkpoint> {
    Ok(events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::CheckpointCreated { checkpoint }
                if checkpoint.id == id =>
            {
                Some(checkpoint.clone())
            }
            _ => None,
        })
        .ok_or("the requested checkpoint was not published")?)
}

fn workspace(events: &[Event]) -> Fallible<proto::Workspace> {
    Ok(events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::WorkspaceCreated { workspace } => {
                Some(workspace.clone())
            }
            _ => None,
        })
        .ok_or("the managed worktree was not published")?)
}

fn diff_result(result: CommandResult) -> Fallible<proto::CheckpointDiff> {
    match result {
        CommandResult::Diff(diff) => Ok(diff),
        other => {
            Err(format!("expected a checkpoint diff, not {other:?}").into())
        }
    }
}

/// A source checkout with staged, unstaged and untracked changes, whose
/// recordings fork on the second and third runs.
struct SourceCheckout {
    index: Vec<u8>,
    staged: String,
    unstaged: String,
}

impl SourceCheckout {
    fn dirty(source: &Path) -> Fallible<Self> {
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/e2e/claude-worktrees-fork.jsonl"),
            source.join("stdout.2.jsonl"),
        )?;
        std::fs::copy(
            source.join("stdout.2.jsonl"),
            source.join("stdout.3.jsonl"),
        )?;
        std::fs::write(source.join("baseline.txt"), "source staged\n")?;
        git(source, &["add", "baseline.txt"])?;
        std::fs::write(source.join("baseline.txt"), "source unstaged\n")?;
        std::fs::write(
            source.join("source-only.txt"),
            "never enter a session\n",
        )?;
        Ok(Self {
            index: std::fs::read(source.join(".git/index"))?,
            staged: git(source, &["diff", "--cached"])?,
            unstaged: git(source, &["diff"])?,
        })
    }

    fn assert_unchanged(&self, source: &Path) -> Fallible {
        assert_eq!(std::fs::read(source.join(".git/index"))?, self.index);
        assert_eq!(git(source, &["diff", "--cached"])?, self.staged);
        assert_eq!(git(source, &["diff"])?, self.unstaged);
        Ok(())
    }
}

/// The session's worktree is its own branch off `source`'s commit, without
/// the checkout's uncommitted files.
fn assert_isolated(
    managed: &proto::Workspace,
    source: &Path,
    session: SessionId,
) -> Fallible<PathBuf> {
    assert_eq!(managed.original.0, source.display().to_string());
    assert_eq!(
        managed.branch,
        format!("conductor/{}", ulid::Ulid::from(session.0))
    );
    let worktree = PathBuf::from(&managed.worktree.0);
    assert_ne!(worktree, source);
    assert_eq!(
        std::fs::read_to_string(worktree.join("baseline.txt"))?,
        "committed baseline\n"
    );
    assert_absent(&worktree.join("source-only.txt"));
    Ok(worktree)
}

fn assert_turn_tasks(kept: &[Vec<Task>], discarded: &[Vec<Task>]) {
    assert_eq!(
        [kept, discarded].map(|lists| lists
            .iter()
            .map(|tasks| owned(steps(tasks)))
            .collect::<Vec<Vec<(String, TaskStatus)>>>()),
        [
            Vec::from([Vec::from([(
                String::from("turn one"),
                TaskStatus::Completed
            )])]),
            Vec::from([Vec::from([
                (String::from("turn one"), TaskStatus::Completed),
                (String::from("turn two"), TaskStatus::InProgress),
            ])]),
        ]
    );
}

/// The second turn's diff against the first: its files, its text, and the
/// executable bit it set.
fn assert_discarded_diff(
    diff: &proto::CheckpointDiff,
    retained: &proto::Checkpoint,
    discarded: &proto::Checkpoint,
    worktree: &Path,
) -> Fallible {
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
        git(worktree, &["ls-tree", &discarded.commit, "exec.sh"])?
            .split_whitespace()
            .next(),
        Some("100755")
    );
    Ok(())
}

/// The last run of the Claude stand-in forked the conversation at the
/// retained turn's message.
fn assert_forked_at_retained_prefix(source: &Path) -> Fallible {
    let runs = argv(source)?;
    let resumed = runs.last().ok_or("expected a value")?;
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
    Ok(())
}

/// File undo and the lazy native fork survive restart together. All Git
/// staging/checkpoint operations stay out of the source repository's index.
#[tokio::test]
async fn managed_worktrees_isolate_source_diff_and_undo_files_and_history_after_restart()
-> Fallible {
    let source = scratch("workspace-undo", "claude-worktrees.jsonl")?;
    let checkout = SourceCheckout::dirty(&source)?;
    let data = data_dir("workspace-undo")?;
    let first =
        durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let address = first.address()?;
    let ticket = first.target.clone();
    let (phone, mut updates) = link(ticket.clone(), None).await?;
    until_live(&mut updates).await?;
    let session = started_session(
        &send(&phone, start(Provider::Claude, &source)).await?,
    )?;
    let opening = events_until(&mut updates, Until::Ready).await?;
    let managed = workspace(&opening)?;
    assert_eq!(checkpoint(&opening, proto::CheckpointId(0))?.parent, None);
    let worktree = assert_isolated(&managed, &source, session)?;
    let other = started_session(
        &send(&phone, start(Provider::Claude, &source)).await?,
    )?;
    let other_opening = events_until(&mut updates, Until::Ready).await?;
    let other_workspace = workspace(&other_opening)?;
    let other_path = PathBuf::from(&other_workspace.worktree.0);
    assert_ne!(session, other);
    assert_ne!(managed.worktree, other_workspace.worktree);
    assert_ne!(managed.branch, other_workspace.branch);
    let _ = send(&phone, message(session, "turn one")).await?;
    let first_turn = events_until(&mut updates, Until::TurnEnd).await?;
    let retained = checkpoint(&first_turn, proto::CheckpointId(1))?;
    let _ = send(&phone, message(session, "turn two")).await?;
    let second_turn = events_until(&mut updates, Until::TurnEnd).await?;
    let discarded = checkpoint(&second_turn, proto::CheckpointId(2))?;
    let kept_tasks = task_lists(&first_turn);
    assert_turn_tasks(&kept_tasks, &task_lists(&second_turn));
    assert_eq!(discarded.parent, Some(retained.id));
    let diff = get_diff(&phone, session, discarded.id).await?;
    assert_discarded_diff(&diff, &retained, &discarded, &worktree)?;
    let _ = send(&phone, message(session, "turn three")).await?;
    let third_turn = events_until(&mut updates, Until::TurnEnd).await?;
    checkpoint(&third_turn, proto::CheckpointId(3))?;
    checkout.assert_unchanged(&source)?;
    assert_independent(&phone, &mut updates, other, &other_path, &worktree)
        .await?;
    let retry_endpoint = endpoint().await?;
    let retry = authenticated(&retry_endpoint, &ticket).await?;
    let undo = proto::Command {
        id: proto::CommandId(1301),
        body: CommandBody::UndoTurn {
            session,
            checkpoint: discarded.id,
        },
    };
    let receipt = retry_command(&retry, &undo).await?;
    assert_eq!(receipt, CommandResult::Accepted);
    let undone = events_until(&mut updates, Until::Undone).await?;
    assert_eq!(
        of(session, &undone).last(),
        Some(&EventKind::TurnsUndone {
            checkpoint: discarded.id,
            restored: retained.id
        })
    );
    assert_eq!(task_lists(&undone), kept_tasks);
    assert_undone_files(&worktree)?;
    retry_endpoint.close().await;
    let second = restart(first, &data, address, &phone, &mut updates).await?;
    let restarted_retry_endpoint = endpoint().await?;
    let restarted_retry =
        authenticated(&restarted_retry_endpoint, &ticket).await?;
    assert_eq!(retry_command(&restarted_retry, &undo).await?, receipt);
    assert_eq!(
        send(&phone, continued(session)).await?,
        CommandResult::Accepted
    );
    events_until(&mut updates, Until::Ready).await?;
    let _ = send(&phone, message(session, "remember retained prefix")).await?;
    let after = events_until(&mut updates, Until::TurnEnd).await?;
    assert_eq!(
        assistant_messages(session, &after),
        [reply("Retained turn one.")]
    );
    assert_forked_at_retained_prefix(&source)?;
    assert_eq!(std::fs::read(source.join(".git/index"))?, checkout.index);
    restarted_retry_endpoint.close().await;
    phone.stop().await?;
    second.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn new_sessions_require_an_existing_repository_with_a_commit() -> Fallible
{
    let host = host().await?;
    let (phone, mut updates) = link(host.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let plain = data_dir("not-a-repository")?;
    // Stop Git discovery at this fixture instead of finding the enclosing checkout.
    std::fs::write(plain.join(".git"), "gitdir: no-such-git-directory\n")?;
    assert_eq!(
        send(&phone, start(Provider::Claude, &plain)).await?,
        CommandResult::Rejected(Rejection::NotGitRepository)
    );
    std::fs::remove_file(plain.join(".git"))?;
    git(&plain, &["init", "--quiet"])?;
    assert_eq!(
        send(&phone, start(Provider::Claude, &plain)).await?,
        CommandResult::Rejected(Rejection::NotGitRepository)
    );
    phone.stop().await?;
    host.router.shutdown().await?;
    Ok(())
}

/// Old persisted sessions have no workspace metadata. Continue still starts
/// their CLI in the original directory, even when it is not a Git repository.
#[tokio::test]
async fn a_legacy_session_continues_in_its_original_directory_without_undo()
-> Fallible {
    let source = data_dir("legacy-session-source")?;
    std::fs::write(source.join(".git"), "gitdir: no-such-git-directory\n")?;
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/claude-worktrees-fork.jsonl"),
        source.join("stdout.jsonl"),
    )?;
    let data = data_dir("legacy-session-host")?;
    let path = data.join("conductor.redb");
    let log = Log::open(String::from("test-host"), &path)?;
    let session = SessionId(1310);
    log.append(
        session,
        EventKind::SessionCreated {
            instance: instance(Provider::Claude),
            provider: Provider::Claude,
            directory: HostPath(source.display().to_string()),
            by: test_client(),
            model: None,
            effort: None,
        },
    )
    .await?;
    log.append(
        session,
        EventKind::SessionClosed {
            reason: CloseReason::Stopped {
                reason: String::from("old host stopped"),
            },
        },
    )
    .await?;
    log.shutdown().await?;
    let host =
        durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let (phone, mut updates) = link(host.target.clone(), None).await?;
    until_live(&mut updates).await?;
    assert_eq!(
        send(
            &phone,
            CommandBody::UndoTurn {
                session,
                checkpoint: proto::CheckpointId(0)
            }
        )
        .await?,
        CommandResult::Rejected(Rejection::CheckpointUnavailable)
    );
    assert_eq!(
        send(&phone, continued(session)).await?,
        CommandResult::Accepted
    );
    let ready = events_until(&mut updates, Until::Ready).await?;
    assert_eq!(
        ready.iter().find(|event| matches!(
            event.kind,
            EventKind::WorkspaceCreated { .. }
        )),
        None
    );
    let _ = send(&phone, message(session, "legacy directory")).await?;
    events_until(&mut updates, Until::TurnEnd).await?;
    assert_eq!(
        std::fs::read_to_string(source.join("after-undo.txt"))?,
        "continued from retained prefix\n"
    );
    phone.stop().await?;
    host.shutdown().await?;
    Ok(())
}

/// Logs an undo of `discarded` back to `retained` that started and failed
/// before it restored any files.
async fn inject_failed_undo(
    log: &Log,
    session: SessionId,
    retained: &proto::Checkpoint,
    discarded: &proto::Checkpoint,
) -> Fallible {
    let history = match retained.history.clone() {
        proto::HistoryAnchor::Messages {
            conversation,
            message: Some(through),
        } => Some(proto::HistoryAnchor::Fork {
            source: conversation,
            through,
            target: String::from("00000000-0000-4000-8000-000000000300"),
        }),
        _ => None,
    }
    .ok_or("expected a value")?;
    log.append(
        session,
        EventKind::UndoStarted {
            checkpoint: discarded.id,
            restore: retained.id,
            history,
        },
    )
    .await?;
    log.append(
        session,
        EventKind::UndoFailed {
            checkpoint: discarded.id,
            reason: String::from("injected failure before files were restored"),
        },
    )
    .await?;
    Ok(())
}

/// A crash after durable undo intent/failure retains the original transcript
/// and blocks normal commands until the same undo completes on retry.
#[tokio::test]
async fn a_failed_pending_undo_replays_after_restart_and_requires_a_successful_retry()
-> Fallible {
    let source = scratch("pending-undo", "claude-worktrees.jsonl")?;
    let data = data_dir("pending-undo")?;
    let first =
        durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let address = first.address()?;
    let ticket = first.target.clone();
    let (phone, mut updates) = link(ticket.clone(), None).await?;
    until_live(&mut updates).await?;
    let TwoTurns {
        session,
        retained,
        selected: discarded,
        ..
    } = two_turns(&phone, &mut updates, &source).await?;
    inject_failed_undo(&first.log, session, &retained, &discarded).await?;
    let second = restart(first, &data, address, &phone, &mut updates).await?;
    let replay = second.log.subscribe(HostSeq(0)).await?.replay;
    let view = fold(replay);
    assert_eq!(
        view.sessions.get(&session).map(|found| found.undo.clone()),
        Some(client::host_view::UndoState::Failed {
            checkpoint: discarded.id,
            reason: String::from("injected failure before files were restored")
        })
    );
    assert_eq!(user_items(&view, session), 2);
    assert_undo_blocks(&phone, message(session, "blocked while recovering"))
        .await?;
    assert_undo_blocks(&phone, continued(session)).await?;
    assert_eq!(
        send(
            &phone,
            CommandBody::UndoTurn {
                session,
                checkpoint: discarded.id
            }
        )
        .await?,
        CommandResult::Accepted
    );
    events_until(&mut updates, Until::Undone).await?;
    let after = fold(second.log.subscribe(HostSeq(0)).await?.replay);
    assert_eq!(user_items(&after, session), 1);
    assert_eq!(
        after.sessions.get(&session).map(|found| found.undo.clone()),
        Some(client::host_view::UndoState::Undone)
    );
    phone.stop().await?;
    second.shutdown().await?;
    Ok(())
}

fn undo_failure(result: CommandResult) -> Fallible<String> {
    match result {
        CommandResult::Rejected(Rejection::UndoFailed { reason }) => Ok(reason),
        other => {
            Err(format!("expected an undo phase failure, not {other:?}").into())
        }
    }
}

#[tokio::test]
async fn native_refusal_then_files_failure_requires_retry_across_a_real_restart()
-> Fallible {
    let source = scratch("actual-undo-failures", "claude-worktrees.jsonl")?;
    let data = data_dir("actual-undo-failures")?;
    let first =
        durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let address = first.address()?;
    let ticket = first.target.clone();
    let (phone, mut updates) = link(ticket.clone(), None).await?;
    until_live(&mut updates).await?;
    let TwoTurns {
        session,
        worktree,
        retained,
        selected,
    } = two_turns(&phone, &mut updates, &source).await?;
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/claude-worktrees-fork-refused.jsonl"),
        source.join("stdout.2.jsonl"),
    )?;
    let undo = CommandBody::UndoTurn {
        session,
        checkpoint: selected.id,
    };
    assert_ne!(
        undo_failure(send(&phone, undo.clone()).await?)?
            .find("temporary fake native fork refusal"),
        None
    );
    let native_failure = events_until(&mut updates, Until::UndoFailure).await?;
    assert!(!any_event(&native_failure, |kind| matches!(
        kind,
        EventKind::UndoHistoryRestored { .. }
    )));
    assert_ne!(
        std::fs::read_to_string(worktree.join("changed.txt"))?.find("turn two"),
        None
    );
    assert_undo_blocks(
        &phone,
        message(session, "blocked after native refusal"),
    )
    .await?;
    let git_dir = PathBuf::from(
        git(&worktree, &["rev-parse", "--absolute-git-dir"])?.trim(),
    );
    let lock = git_dir.join("index.lock");
    std::fs::write(&lock, "owned test lock\n")?;
    assert_ne!(
        undo_failure(send(&phone, undo.clone()).await?)?.find("index.lock"),
        None
    );
    let files_failure = events_until(&mut updates, Until::UndoFailure).await?;
    assert_eq!(
        files_failure
            .iter()
            .filter(|event| matches!(
                event.kind,
                EventKind::UndoHistoryRestored { .. }
            ))
            .count(),
        1
    );
    assert!(!any_event(&files_failure, |kind| matches!(
        kind,
        EventKind::TurnsUndone { .. }
    )));
    assert_undo_blocks(&phone, continued(session)).await?;
    let second = restart(first, &data, address, &phone, &mut updates).await?;
    assert_undo_blocks(&phone, message(session, "blocked after restart"))
        .await?;
    std::fs::remove_file(&lock)?;
    assert_eq!(send(&phone, undo).await?, CommandResult::Accepted);
    let recovered = events_until(&mut updates, Until::Undone).await?;
    assert!(!any_event(&recovered, |kind| matches!(
        kind,
        EventKind::UndoHistoryRestored { .. }
    )));
    assert_restored_files(&worktree)?;
    let restored = fold(second.log.subscribe(HostSeq(0)).await?.replay);
    assert_eq!(user_items(&restored, session), 1);
    assert_eq!(
        send(&phone, message(session, "retained prefix after recovery"))
            .await?,
        CommandResult::Accepted
    );
    let after = events_until(&mut updates, Until::TurnEnd).await?;
    assert_ne!(checkpoint(&after, proto::CheckpointId(3))?.id, retained.id);
    assert_eq!(
        assistant_messages(session, &after),
        [reply("Retained turn one.")]
    );
    phone.stop().await?;
    second.shutdown().await?;
    Ok(())
}

async fn drive_materialization(path: &Path) -> std::io::Result<()> {
    loop {
        match tokio::fs::metadata(path).await {
            Ok(_) => break Ok(()),
            Err(error) => match error.kind() {
                std::io::ErrorKind::NotFound => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                _ => break Err(error),
            },
        }
    }
}

/// The fork an undo restored history to.
fn fork_target(undone: &[Event]) -> Fallible<String> {
    Ok(undone
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::UndoHistoryRestored {
                history: proto::HistoryAnchor::Fork { target, .. },
                ..
            } => Some(target.clone()),
            _ => None,
        })
        .ok_or("expected a value")?)
}

/// The checkpoint recovery took of a turn whose start the host never saw:
/// it holds the files, and can't be undone to.
fn unknown_boundary(
    recovery: &[Event],
    worktree: &Path,
) -> Fallible<proto::Checkpoint> {
    let unknown = checkpoint(recovery, proto::CheckpointId(3))?;
    assert_eq!(unknown.history, proto::HistoryAnchor::Unknown);
    assert_ne!(
        Some(&unknown.undo).filter(|undo| matches!(
            undo,
            proto::UndoAvailability::Unavailable { .. }
        )),
        None
    );
    assert_eq!(
        git(
            worktree,
            &["show", &format!("{}:unnotified.txt", unknown.commit)]
        )?,
        "provider wrote before notification\n"
    );
    Ok(unknown)
}

/// The provider persisted the first post-undo input before it notified the
/// daemon. After restart, resume that existing target instead of forking the
/// older prefix again and silently discarding the newly persisted input.
#[tokio::test]
async fn a_materialized_fork_target_is_resumed_when_its_notification_was_lost()
-> Fallible {
    let source = scratch("materialized-fork-crash", "claude-worktrees.jsonl")?;
    let data = data_dir("materialized-fork-crash")?;
    let first =
        durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let address = first.address()?;
    let ticket = first.target.clone();
    let (phone, mut updates) = link(ticket.clone(), None).await?;
    until_live(&mut updates).await?;
    let TwoTurns {
        session, worktree, ..
    } = two_turns(&phone, &mut updates, &source).await?;
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/claude-worktrees-fork-held.jsonl"),
        source.join("stdout.2.jsonl"),
    )?;
    assert_eq!(
        send(&phone, undo_turn(session, proto::CheckpointId(2))).await?,
        CommandResult::Accepted
    );
    let undone = events_until(&mut updates, Until::Undone).await?;
    let after_undo = undone.last().ok_or("expected a value")?.seq;
    let target = fork_target(&undone)?;
    assert_eq!(
        send(&phone, message(session, "first post undo input")).await?,
        CommandResult::Accepted
    );
    tokio::time::timeout(
        STEP,
        drive_materialization(&source.join("fork-materialized")),
    )
    .await??;
    let native_transcript =
        std::fs::read_to_string(source.join(".fake-sessions").join(&target))?;
    assert_ne!(native_transcript.find("first post undo input"), None);
    let unnotified = first.log.subscribe(after_undo).await?.replay;
    assert!(!any_event(&unnotified, |kind| matches!(
        kind,
        EventKind::NativeTurnStarted { .. }
            | EventKind::HistoryMaterialized { .. }
    )));
    let before_runs = argv(&source)?.len();
    let second = restart(first, &data, address, &phone, &mut updates).await?;
    let recovery = second.log.subscribe(HostSeq(0)).await?.replay;
    let unknown = unknown_boundary(&recovery, &worktree)?;
    assert_eq!(
        send(&phone, undo_turn(session, unknown.id)).await?,
        CommandResult::Rejected(Rejection::CheckpointUnavailable)
    );
    assert_eq!(
        send(&phone, continued(session)).await?,
        CommandResult::Accepted
    );
    events_until(&mut updates, Until::Ready).await?;
    let runs = argv(&source)?;
    assert_eq!(
        runs.len(),
        before_runs.checked_add(1).ok_or("expected a value")?
    );
    let resumed = runs.last().ok_or("expected a value")?;
    assert_eq!(
        resumed
            .iter()
            .find(|arg| arg.starts_with("--resume="))
            .cloned(),
        Some(format!("--resume={target}"))
    );
    assert!(!resumed.iter().any(|arg| arg.as_str() == "--fork-session"));
    std::fs::write(source.join("fork-release"), "")?;
    let _ = send(&phone, message(session, "continue existing target")).await?;
    let after = events_until(&mut updates, Until::TurnEnd).await?;
    assert_eq!(
        checkpoint(&after, proto::CheckpointId(4))?.history,
        proto::HistoryAnchor::Messages {
            conversation: target,
            message: Some(String::from("00000000-0000-4000-8000-000000000204"))
        }
    );
    phone.stop().await?;
    second.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn a_failed_snapshot_is_retried_before_accepting_the_next_user_message()
-> Fallible {
    let source = scratch("snapshot-retry", "claude-worktrees.jsonl")?;
    let host = host().await?;
    let (phone, mut updates) = link(host.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session = started_session(
        &send(&phone, start(Provider::Claude, &source)).await?,
    )?;
    let managed = workspace(&events_until(&mut updates, Until::Ready).await?)?;
    let worktree = PathBuf::from(&managed.worktree.0);
    let _ = send(&phone, message(session, "turn one")).await?;
    let retained = checkpoint(
        &events_until(&mut updates, Until::TurnEnd).await?,
        proto::CheckpointId(1),
    )?;
    git(&worktree, &["symbolic-ref", "HEAD", "refs/heads/unrelated"])?;
    let _ = send(&phone, message(session, "turn two")).await?;
    let failed = events_until(&mut updates, Until::TurnEnd).await?;
    assert!(!any_event(&failed, |kind| matches!(
        kind,
        EventKind::CheckpointCreated { .. }
    )));
    assert_eq!(
        failed
            .iter()
            .filter(|event| matches!(event.kind, EventKind::Error { .. }))
            .count(),
        1
    );
    let before = host.log.subscribe(HostSeq(0)).await?.replay;
    let users = before
        .iter()
        .filter(|event| matches!(event.kind, EventKind::UserMessage { .. }))
        .count();
    let rejected =
        send(&phone, message(session, "must not enter native history")).await?;
    assert_ne!(
        Some(rejected).filter(|result| matches!(
            result,
            CommandResult::Rejected(Rejection::WorkspaceFailure { .. })
        )),
        None
    );
    let still = host.log.subscribe(HostSeq(0)).await?.replay;
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
    )?;
    assert_eq!(
        send(&phone, message(session, "turn three")).await?,
        CommandResult::Accepted
    );
    let after = events_until(&mut updates, Until::TurnEnd).await?;
    let checkpoints = after
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::CheckpointCreated { checkpoint } => {
                Some(checkpoint.clone())
            }
            _ => None,
        })
        .collect::<Vec<proto::Checkpoint>>();
    assert_eq!(checkpoints.len(), 2);
    let recovered = checkpoints.first().ok_or("expected a value")?;
    let latest = checkpoints.last().ok_or("expected a value")?;
    assert_eq!(recovered.parent, Some(retained.id));
    assert_eq!(latest.parent, Some(recovered.id));
    assert_eq!(recovered.id.cmp(&proto::CheckpointId(2)), Ordering::Greater);
    let diff = get_diff(&phone, session, latest.id).await?;
    assert_eq!(
        diff.files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<String>>(),
        ["changed.txt", "later.txt"]
    );
    phone.stop().await?;
    host.router.shutdown().await?;
    Ok(())
}

fn assert_restored_files(worktree: &Path) -> Fallible {
    assert_eq!(
        std::fs::read_to_string(worktree.join("changed.txt"))?,
        "turn one\n"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("deleted.txt"))?,
        "restore me\n"
    );
    assert_eq!(
        std::fs::metadata(worktree.join("introduced.txt"))
            .err()
            .map(|error| error.kind()),
        Some(std::io::ErrorKind::NotFound)
    );
    Ok(())
}

const fn set_effort(session: SessionId, effort: Effort) -> CommandBody {
    CommandBody::SetEffort { session, effort }
}

/// The task lists in `events`, in order.
fn task_lists(events: &[Event]) -> Vec<Vec<Task>> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::TasksChanged { tasks } => Some(tasks.clone()),
            _ => None,
        })
        .collect()
}

/// Each task's text and status, the ids and holder aside.
fn steps(tasks: &[Task]) -> Vec<(&str, TaskStatus)> {
    tasks
        .iter()
        .map(|task| (task.text.as_str(), task.status))
        .collect()
}

/// The three lists the recorded Claude and Codex runs wrote: three steps,
/// the first pending, then in progress, then done.
fn written(a: &str, b: &str, c: &str) -> Vec<Vec<(String, TaskStatus)>> {
    [
        TaskStatus::Pending,
        TaskStatus::InProgress,
        TaskStatus::Completed,
    ]
    .into_iter()
    .map(|first| {
        Vec::from([
            (String::from(a), first),
            (String::from(b), TaskStatus::Pending),
            (String::from(c), TaskStatus::Pending),
        ])
    })
    .collect()
}

fn owned(steps: Vec<(&str, TaskStatus)>) -> Vec<(String, TaskStatus)> {
    steps
        .into_iter()
        .map(|(text, status)| (String::from(text), status))
        .collect()
}

/// Each task's id and holder, list by list.
fn owners(lists: &[Vec<Task>]) -> Vec<Vec<(TaskId, SessionId)>> {
    lists
        .iter()
        .map(|tasks| tasks.iter().map(|task| (task.id, task.holder)).collect())
        .collect()
}

/// The first list's ids, held by `session`, as many times as there are
/// lists: what `owners` gives when every list keeps them.
fn kept(
    session: SessionId,
    lists: &[Vec<Task>],
) -> Vec<Vec<(TaskId, SessionId)>> {
    let first = lists.first().map_or_else(Vec::new, |tasks| {
        tasks.iter().map(|task| (task.id, session)).collect()
    });
    lists.iter().map(|_| Vec::clone(&first)).collect()
}

/// What each line the session wrote to the Claude stand-in was: a control
/// request's subtype and settings, or a user message's text.
fn written_to_claude(dir: &Path) -> Fallible<Vec<String>> {
    let parsed = sent_to_agent(dir)?
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<serde_json::Value>, serde_json::Error>>()?;
    Ok(parsed.iter().map(written_line).collect())
}

/// A control request's subtype and settings, or a user message's text.
fn written_line(line: &serde_json::Value) -> String {
    line.pointer("/request/subtype").map_or_else(
        || {
            line.pointer("/message/content/0/text")
                .and_then(serde_json::Value::as_str)
                .map_or_else(String::new, String::from)
        },
        |subtype| {
            format!(
                "{} {}",
                subtype.as_str().unwrap_or_default(),
                line.pointer("/request/settings")
                    .map_or_else(String::new, serde_json::Value::to_string)
            )
        },
    )
}

/// Holds the recording's first turn until the test writes `release`.
fn hold_first_turn(dir: &Path) -> Fallible {
    let stdout = dir.join("stdout.jsonl");
    let user = r#"{"fake": "await", "match": {"type": "user"}}"#;
    let held = read(&stdout)?.replacen(
        user,
        &format!("{user}\n{{\"fake\": \"release\", \"file\": \"release\"}}"),
        1,
    );
    std::fs::write(stdout, held)?;
    Ok(())
}

/// A Claude turn's `TodoWrite` calls become the session's tasks: one
/// `TasksChanged` per call, each task keeping its id, and no call shown as
/// a tool call. An effort set while the turn runs is checked against the
/// model's efforts, logged at once, and reaches the CLI with the next
/// message (`apply_flag_settings`, then the message), not during the turn.
#[tokio::test]
async fn a_claude_checklist_becomes_tasks_and_a_new_effort_waits_for_the_next_turn()
-> Fallible {
    let fx = host().await?;
    let dir = scratch("checklist", "claude-checklist-effort.jsonl")?;
    hold_first_turn(&dir)?;
    let (phone, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&phone, start(Provider::Claude, &dir)).await?)?;
    let opened = legacy_events_until(&mut updates, Until::Ready).await?;
    let _ = send(&phone, message(session, "plan")).await?;
    let mid_turn = [
        send(&phone, set_effort(session, Effort::Max)).await?,
        send(&phone, set_effort(session, Effort::Low)).await?,
        send(&phone, set_effort(session, Effort::Low)).await?,
    ];
    let before = written_to_claude(&dir)?;
    std::fs::write(dir.join("release"), "")?;
    let turn = legacy_events_until(&mut updates, Until::TurnEnd).await?;
    let lists = task_lists(&turn);
    assert_eq!(
        (
            mid_turn,
            before,
            lists
                .iter()
                .map(|tasks| owned(steps(tasks)))
                .collect::<Vec<Vec<(String, TaskStatus)>>>(),
            owners(&lists),
            turn.iter()
                .filter(|event| matches!(
                    event.kind,
                    EventKind::ToolCall {
                        phase: ToolPhase::Started { .. },
                        ..
                    } | EventKind::RequestOpened { .. }
                ))
                .count(),
            turn.iter()
                .filter(|event| matches!(
                    event.kind,
                    EventKind::EffortChanged { .. }
                ))
                .map(|event| event.kind.clone())
                .collect::<Vec<EventKind>>(),
        ),
        (
            [
                CommandResult::Rejected(Rejection::NotOffered {
                    reason: String::from("Big doesn't take the effort max"),
                }),
                CommandResult::Accepted,
                CommandResult::Accepted,
            ],
            Vec::from([String::from("initialize "), String::from("plan")]),
            written("Write a.txt", "Write b.txt", "Write c.txt"),
            kept(session, &lists),
            0,
            Vec::from([EventKind::EffortChanged {
                effort: Effort::Low
            }]),
        )
    );
    let _ = send(&phone, message(session, "pong")).await?;
    let next = legacy_events_until(&mut updates, Until::TurnEnd).await?;
    assert_eq!(
        written_to_claude(&dir)?,
        [
            "initialize ",
            "plan",
            r#"apply_flag_settings {"effortLevel":"low"}"#,
            "pong"
        ]
    );
    let view = fold([opened, turn, next].concat());
    assert_eq!(
        view.sessions
            .get(&session)
            .map(|found| (found.effort, found.tasks.clone())),
        Some((Some(Effort::Low), lists.last().cloned().unwrap_or_default()))
    );
    phone.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// A Codex turn's plan updates become the session's tasks as Claude's
/// checklist does, and a new effort rides on the next `turn/start`.
#[tokio::test]
async fn a_codex_plan_becomes_tasks_and_a_new_effort_starts_the_next_turn()
-> Fallible {
    let (fx, dir) = codex_host("codex-plan-effort.jsonl").await?;
    let (phone, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&phone, start(Provider::Codex, &dir)).await?)?;
    let _ = legacy_events_until(&mut updates, Until::Ready).await?;
    let _ = send(&phone, message(session, "plan")).await?;
    let turn = legacy_events_until(&mut updates, Until::TurnEnd).await?;
    let lists = task_lists(&turn);
    let set = send(&phone, set_effort(session, Effort::High)).await?;
    let _ = send(&phone, message(session, "pong")).await?;
    let next = kinds_until(&mut updates, Until::TurnEnd).await?;
    assert_eq!(
        (
            lists
                .iter()
                .map(|tasks| owned(steps(tasks)))
                .collect::<Vec<Vec<(String, TaskStatus)>>>(),
            owners(&lists),
            set,
            next.into_iter()
                .filter(|kind| !matches!(
                    kind,
                    EventKind::AssistantDelta { .. }
                ))
                .collect::<Vec<EventKind>>(),
            requests(&dir, "stdin.jsonl", "turn/start")?
                .iter()
                .map(|params| params.get("effort").cloned())
                .collect::<Vec<Option<serde_json::Value>>>(),
        ),
        (
            written("write a.txt", "write b.txt", "write c.txt"),
            kept(session, &lists),
            CommandResult::Accepted,
            Vec::from([
                EventKind::EffortChanged {
                    effort: Effort::High
                },
                user("pong"),
                reply("pong"),
                turn_end(TurnOutcome::Completed),
            ]),
            Vec::from([None, Some(serde_json::Value::from("high"))]),
        )
    );
    phone.stop().await?;
    fx.router.shutdown().await?;
    Ok(())
}

/// A host restart keeps the session's tasks and its effort: `Continue`
/// starts the CLI at that effort, and the agent's next checklist keeps the
/// tasks' ids.
#[tokio::test]
async fn tasks_and_effort_survive_a_host_restart() -> Fallible {
    let dir = scratch("tasks-restart", "claude-checklist-effort.jsonl")?;
    let data = data_dir("tasks-restart")?;
    std::fs::copy(dir.join("stdout.jsonl"), dir.join("stdout.2.jsonl"))?;
    let first =
        durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let address = first.address()?;
    let (phone, mut updates) = link(first.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let session =
        started_session(&send(&phone, start(Provider::Claude, &dir)).await?)?;
    let _ = send(&phone, message(session, "plan")).await?;
    let before =
        task_lists(&legacy_events_until(&mut updates, Until::TurnEnd).await?);
    let set = send(&phone, set_effort(session, Effort::Low)).await?;
    first.shutdown().await?;

    let second = durable_host(&data, address).await?;
    phone.reconnect().await;
    let _ = until_live(&mut updates).await?;
    let continued = send(&phone, continued(session)).await?;
    let _ = kinds_until(&mut updates, Until::Ready).await?;
    let _ = send(&phone, message(session, "plan")).await?;
    let after =
        task_lists(&legacy_events_until(&mut updates, Until::TurnEnd).await?);
    let (late, mut late_updates) = link(second.target.clone(), None).await?;
    let view = fold(
        until_live(&mut late_updates)
            .await?
            .into_iter()
            .filter_map(|update| match update {
                Update::Event(event) => Some(event),
                Update::Status(_) | Update::Reset(_) => None,
            })
            .collect(),
    );
    assert_eq!(
        (
            set,
            continued,
            argv(&dir)?
                .iter()
                .map(|run| flag(run, "--effort"))
                .collect::<Vec<Option<String>>>(),
            owners(&[before.clone(), after.clone()].concat()),
            after.len(),
            view.sessions
                .get(&session)
                .map(|found| (found.effort, found.tasks.clone())),
        ),
        (
            CommandResult::Accepted,
            CommandResult::Accepted,
            Vec::from([None, Some(String::from("low"))]),
            kept(session, &[before, after.clone()].concat()),
            3,
            Some((
                Some(Effort::Low),
                after.last().cloned().unwrap_or_default()
            )),
        )
    );
    late.stop().await?;
    phone.stop().await?;
    second.shutdown().await?;
    Ok(())
}

/// The code the sign-in stand-in accepts (fixtures/fake-login.py).
const SIGN_IN_CODE: &str = "conductor-test-sign-in-code#fake-state";
/// One it refuses.
const WRONG_CODE: &str = "conductor-test-wrong-code";

/// The instance's sign-in steps the link shows, through the first that
/// `done` takes.
async fn sign_in_until(
    updates: &mut mpsc::Receiver<Update>,
    instance: &InstanceId,
    done: impl Fn(&proto::SignInStep) -> bool,
) -> Fallible<proto::SignInStep> {
    loop {
        if let Update::Status(status) = next(updates).await?
            && let Some(step) = status.sign_ins.get(instance)
            && done(step)
        {
            break Ok(step.clone());
        }
    }
}

/// Every file under `dir` that holds `text`.
fn holding(dir: &Path, text: &str) -> Fallible<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            found.extend(holding(&path, text)?);
        } else if count(&path, text)? > 0 {
            found.push(path);
        }
    }
    Ok(found)
}

/// A sign-in from the app on the stand-in: it prints a link and asks for
/// the code the page gives; a wrong code fails, the right one signs the
/// home in, and the account the CLI names becomes an instance, signed in,
/// with the home, without a restart. Neither code is in a log line, a
/// stored event or receipt, or any file the host keeps.
#[tokio::test]
async fn a_sign_in_from_the_app_makes_the_account_s_instance_and_stores_no_code()
-> Fallible {
    let data = data_dir("sign-in")?;
    let (log_path, logging) = capture_log("sign-in")?;
    probed_in(&data, "claude")?;
    let log =
        Log::open(String::from("test-host"), &data.join("conductor.redb"))?;
    let fx = plugin_host_logging(&data, log).await?;
    let (link, mut updates, _) = live(&fx).await?;
    let claude = instance(Provider::Claude);
    let account =
        InstanceId::named(Provider::Claude, "conductor-test@example.invalid");
    let start = || CommandBody::StartSignIn {
        instance: claude.clone(),
    };
    let submit = |code: &str| CommandBody::SubmitSignInCode {
        instance: claude.clone(),
        code: proto::SignInCode(String::from(code)),
    };
    let open = |step: &proto::SignInStep| {
        matches!(step, proto::SignInStep::Open { .. })
    };
    let ended = |step: &proto::SignInStep| {
        matches!(
            step,
            proto::SignInStep::Failed { .. }
                | proto::SignInStep::SignedIn { .. }
        )
    };

    let started = send(&link, start()).await?;
    let shown = sign_in_until(&mut updates, &claude, open).await?;
    let again = send(&link, start()).await?;
    let wrong = send(&link, submit(WRONG_CODE)).await?;
    let refused = sign_in_until(&mut updates, &claude, ended).await?;
    let restarted = send(&link, start()).await?;
    let _ = sign_in_until(&mut updates, &claude, open).await?;
    let right = send(&link, submit(SIGN_IN_CODE)).await?;
    let signed_in = sign_in_until(&mut updates, &claude, ended).await?;
    let now = Availability::Available {
        auth: Auth::SignedIn,
        models: probed(),
    };
    assert_eq!(
        (started, again, wrong, right),
        (
            CommandResult::Accepted,
            CommandResult::Rejected(Rejection::SignInRunning),
            CommandResult::Accepted,
            CommandResult::Accepted,
        )
    );
    assert_eq!(
        (shown, refused, restarted),
        (
            proto::SignInStep::Open {
                url: String::from(
                    "https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=org%3Acreate_api_key+user%3Aprofile+user%3Ainference+user%3Asessions%3Aclaude_code+user%3Amcp_servers+user%3Afile_upload+user%3Aplugins&code_challenge=CHALLENGE&code_challenge_method=S256&state=STATE"
                ),
                code: None,
                paste: true,
            },
            proto::SignInStep::Failed {
                reason: String::from(
                    "Claude didn't accept that code. Sign in again to get a new one."
                ),
            },
            CommandResult::Accepted,
        )
    );
    assert_eq!(
        (
            match signed_in {
                proto::SignInStep::SignedIn { instances } => {
                    status_of(&CommandResult::Instances(instances), &account)
                        .map(|it| it.availability)
                }
                _ => None,
            },
            link.status()
                .instances
                .and_then(|known| known.of(&account).cloned())
                .map(|it| it.availability),
            dir_of(&data, "home", &account)
                .join(".fake-signed-in")
                .exists(),
            dir_of(&data, "home", &claude)
                .join(".fake-signed-in")
                .exists(),
        ),
        (Some(now.clone()), Some(now), true, false),
        "the signed-in home moved to the account's instance, and Claude's own is fresh"
    );
    link.stop().await?;
    fx.shutdown().await?;
    drop(logging);
    assert_no_code_kept(&log_path, &data)
}

/// Neither sign-in code is in the log at `log_path` or a file under
/// `data`, and the log has both submits, without their text, and one
/// sign-in that took.
fn assert_no_code_kept(log_path: &Path, data: &Path) -> Fallible {
    let logged = captured(log_path)?;
    assert_eq!(
        (
            logged.contains(SIGN_IN_CODE) || logged.contains(WRONG_CODE),
            logged.matches("SignInCode(..)").count(),
            logged
                .matches("sign-in instance=claude step=signed in")
                .count(),
            holding(data, SIGN_IN_CODE)?,
            holding(data, WRONG_CODE)?,
        ),
        (false, 2, 1, Vec::new(), Vec::new()),
        "{logged}"
    );
    Ok(())
}

/// A bare copy of the repository at `source`, as `name.git` beside it, and
/// its `file://` URL.
fn bare(source: &Path, name: &str) -> Fallible<(PathBuf, String)> {
    let parent = source.parent().ok_or("a repository has a parent")?;
    let target = parent.join(format!("{name}-remote.git"));
    git(
        parent,
        &[
            "clone",
            "--bare",
            "--quiet",
            &source.display().to_string(),
            &target.display().to_string(),
        ],
    )?;
    let url = format!("file://{}", target.display());
    Ok((target, url))
}

/// A committed repository at `dir` and its root commit.
fn rooted(dir: &Path) -> Fallible<String> {
    std::fs::create_dir_all(dir)?;
    committed_repository(dir)?;
    Ok(git(dir, &["rev-list", "--max-parents=0", "HEAD"])?
        .trim()
        .to_owned())
}

/// A project's `project.toml` in the home at `home`.
fn listed_project(home: &Path, id: &str, fields: (&str, &str)) -> Fallible {
    let (remote, root) = fields;
    let folder = home.join("projects").join(id);
    std::fs::create_dir_all(&folder)?;
    std::fs::write(
        folder.join("project.toml"),
        format!(
            "name = \"{id}\"\ntitle = \"{id}\"\nremote = \"{remote}\"\n\
             root = \"{root}\"\nprofiles = [\"rust\"]\n"
        ),
    )?;
    Ok(())
}

/// The home as the link has it, once it is as `done` wants.
async fn home_until(
    link: &HostLink,
    updates: &mut mpsc::Receiver<Update>,
    done: impl Fn(&proto::Home) -> bool,
) -> Fallible<proto::Home> {
    loop {
        if let Some(home) = link.status().home.filter(&done) {
            break Ok(home);
        }
        let _ = next(updates).await?;
    }
}

fn checkouts_of(home: &proto::Home, id: &str) -> Vec<proto::Checkout> {
    home.projects
        .iter()
        .find(|project| project.id.0 == id)
        .map_or_else(Vec::new, |project| project.checkouts.clone())
}

fn found(path: &Path, origin: proto::Origin) -> Fallible<proto::Checkout> {
    Ok(proto::Checkout {
        path: HostPath(std::fs::canonicalize(path)?.display().to_string()),
        origin,
    })
}

/// A home listing alpha and beta, each with a bare remote, and a search
/// root holding the person's checkout of alpha one level down, which
/// matches; one with alpha's remote but other history, and one with alpha's
/// history but another remote, which don't.
struct Homeland {
    scratch: PathBuf,
    roots: PathBuf,
    branch: String,
    home_bare: PathBuf,
    home_url: String,
}

fn homeland(name: &str) -> Fallible<Homeland> {
    let scratch = target_tmp()?
        .join(format!("conductor-home-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let sources = scratch.join("sources");
    let roots = scratch.join("roots");
    std::fs::create_dir_all(&roots)?;
    let alpha_root = rooted(&sources.join("alpha"))?;
    let beta_root = rooted(&sources.join("beta"))?;
    let branch =
        git(&sources.join("alpha"), &["symbolic-ref", "--short", "HEAD"])?
            .trim()
            .to_owned();
    let (_, alpha) = bare(&sources.join("alpha"), "alpha")?;
    let (_, beta) = bare(&sources.join("beta"), "beta")?;
    git(&roots, &["clone", "--quiet", &alpha, "apps/alpha"])?;
    let impostor = roots.join("impostor");
    let _ = rooted(&impostor)?;
    std::fs::write(
        impostor.join("other.txt"),
        "other history
",
    )?;
    git(&impostor, &["add", "other.txt"])?;
    git(&impostor, &["commit", "--quiet", "--amend", "--no-edit"])?;
    git(&impostor, &["remote", "add", "origin", &alpha])?;
    let fork_source = sources.join("alpha").display().to_string();
    git(&roots, &["clone", "--quiet", &fork_source, "fork"])?;
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/claude-approve.jsonl"),
        roots.join("apps").join("alpha").join("stdout.jsonl"),
    )?;
    let home_source = sources.join("home");
    std::fs::create_dir_all(&home_source)?;
    git(&home_source, &["init", "--quiet"])?;
    listed_project(&home_source, "alpha", (&alpha, &alpha_root))?;
    listed_project(&home_source, "beta", (&beta, &beta_root))?;
    git(&home_source, &["add", "projects"])?;
    git(
        &home_source,
        &[
            "commit",
            "--quiet",
            "-m",
            "list alpha and beta

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>",
        ],
    )?;
    let (home_bare, home_url) = bare(&home_source, "home")?;
    Ok(Homeland {
        scratch,
        roots,
        branch,
        home_bare,
        home_url,
    })
}

fn clone_project(id: &str) -> CommandBody {
    CommandBody::CloneProject {
        project: ProjectId(String::from(id)),
    }
}

/// beta, with no checkout, is cloned into the data dir; an unknown project
/// is refused.
async fn clones_beta(
    link: &HostLink,
    updates: &mut mpsc::Receiver<Update>,
    data: &Path,
) -> Fallible<proto::Home> {
    assert_eq!(
        send(link, clone_project("beta")).await?,
        CommandResult::Accepted
    );
    let cloned = home_until(link, updates, |home| {
        !checkouts_of(home, "beta").is_empty()
    })
    .await?;
    assert_eq!(
        checkouts_of(&cloned, "beta"),
        [found(
            &data.join("projects").join("beta"),
            proto::Origin::Cloned
        )?]
    );
    assert_eq!(
        send(link, clone_project("gamma")).await?,
        CommandResult::Rejected(Rejection::UnknownProject)
    );
    Ok(cloned)
}

/// A session in alpha starts in the person's checkout, which stays as it
/// was; an unknown project is refused.
async fn starts_in_alpha(
    link: &HostLink,
    updates: &mut mpsc::Receiver<Update>,
    land: &Homeland,
) -> Fallible {
    let in_project = |id: &str| CommandBody::StartSession {
        instance: instance(Provider::Claude),
        at: StartAt::Project(ProjectId(String::from(id))),
        model: None,
        effort: None,
    };
    assert_eq!(
        send(link, in_project("gamma")).await?,
        CommandResult::Rejected(Rejection::UnknownProject)
    );
    let session = started_session(&send(link, in_project("alpha")).await?)?;
    let events = events_until(updates, Until::Ready).await?;
    let alpha = land.roots.join("apps").join("alpha");
    let checkout = std::fs::canonicalize(&alpha)?.display().to_string();
    let placed = events
        .iter()
        .filter(|event| event.session == session)
        .filter_map(|event| match &event.kind {
            EventKind::SessionCreated { directory, .. } => {
                Some(directory.0.clone())
            }
            EventKind::WorkspaceCreated { workspace } => {
                Some(workspace.repository.0.clone())
            }
            _ => None,
        })
        .collect::<Vec<String>>();
    assert_eq!(placed, [checkout.clone(), checkout]);
    // The fake CLI keeps its control files beside the recording, untracked.
    assert_eq!(
        (
            git(&alpha, &["status", "--porcelain", "--untracked-files=no"])?,
            git(&alpha, &["branch", "--show-current"])?,
        ),
        (
            String::new(),
            format!(
                "{}
",
                land.branch
            )
        ),
        "the person's checkout is untouched"
    );
    Ok(())
}

#[tokio::test]
async fn the_home_is_cloned_its_checkouts_found_and_a_project_cloned_and_started()
-> Fallible {
    let land = homeland("projects")?;
    let (fx, data) =
        host_at_home(land.home_url.clone(), Vec::from([land.roots.clone()]))
            .await?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let synced = home_until(&link, &mut updates, |home| {
        home.sync == proto::HomeSync::Synced && home.projects.len() == 2
    })
    .await?;
    assert_eq!(
        (
            synced.remote.as_str(),
            synced.synced.is_some(),
            data.join("home-repo/projects/alpha/project.toml").exists(),
        ),
        (land.home_url.as_str(), true, true)
    );
    assert_eq!(
        (
            checkouts_of(&synced, "alpha"),
            checkouts_of(&synced, "beta")
        ),
        (
            Vec::from([found(
                &land.roots.join("apps").join("alpha"),
                proto::Origin::Found
            )?]),
            Vec::new()
        )
    );
    let _ = clones_beta(&link, &mut updates, &data).await?;
    starts_in_alpha(&link, &mut updates, &land).await?;

    std::fs::rename(&land.home_bare, land.scratch.join("home.gone"))?;
    assert_eq!(
        send(&link, CommandBody::SyncHome).await?,
        CommandResult::Accepted
    );
    let behind = home_until(&link, &mut updates, |home| {
        matches!(home.sync, proto::HomeSync::Behind { .. })
    })
    .await?;
    // The clone stays as of its last sync, with its projects.
    assert_eq!((behind.synced.is_some(), behind.projects.len()), (true, 2));
    assert!(behind.synced >= synced.synced);
    link.stop().await?;
    fx.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

fn alpha_issues(home: &proto::Home) -> Vec<proto::Issue> {
    home.projects
        .iter()
        .find(|project| project.id.0 == "alpha")
        .map_or_else(Vec::new, |project| project.issues.clone())
}

/// The issue's file as the home's remote has it.
fn pushed_issue(land: &Homeland, issue: proto::IssueId) -> Fallible<String> {
    let path =
        format!("HEAD:projects/alpha/issues/{}.md", home::issue::name(issue));
    git(&land.home_bare, &["show", &path])
}

fn change(issue: proto::IssueId, change: IssueChange) -> CommandBody {
    CommandBody::ChangeIssue { issue, change }
}

/// A person edits the issue's file in a clone of their own and pushes it,
/// behind the host's back.
fn noted_by_hand(land: &Homeland, issue: proto::IssueId) -> Fallible {
    let person = land.scratch.join("person");
    git(
        &land.scratch,
        &[
            "clone",
            "--quiet",
            &land.home_url,
            &person.display().to_string(),
        ],
    )?;
    let file = person
        .join("projects/alpha/issues")
        .join(format!("{}.md", home::issue::name(issue)));
    let text = std::fs::read_to_string(&file)?;
    std::fs::write(&file, format!("{text}\n## Notes\n- by hand\n"))?;
    git(&person, &["commit", "--quiet", "-am", "a note by hand"])?;
    git(&person, &["push", "--quiet"])?;
    Ok(())
}

/// An issue without a title is refused, and one with one is opened in
/// alpha and pushed.
async fn opens_an_issue(
    link: &HostLink,
    updates: &mut mpsc::Receiver<Update>,
    land: &Homeland,
) -> Fallible<proto::IssueId> {
    let create = |title: &str| CommandBody::CreateIssue {
        project: ProjectId(String::from("alpha")),
        fields: IssueFields {
            title: String::from(title),
            body: String::from("Why."),
            priority: Priority::High,
            labels: Vec::from([String::from("app")]),
        },
    };
    assert_eq!(
        send(link, create(" ")).await?,
        CommandResult::Rejected(Rejection::IssueNotSaved {
            reason: String::from("an issue needs a title")
        })
    );
    let issue = match send(link, create("Sign in")).await? {
        CommandResult::IssueCreated { issue } => issue,
        other => return Err(format!("expected an issue, not {other:?}").into()),
    };
    let opened = home_until(link, updates, |home| {
        home.unpushed == 0 && !alpha_issues(home).is_empty()
    })
    .await?;
    assert_eq!(
        alpha_issues(&opened)
            .iter()
            .map(|shown| (shown.id, shown.title.clone(), shown.status))
            .collect::<Vec<_>>(),
        [(issue, String::from("Sign in"), IssueStatus::Open)]
    );
    assert!(pushed_issue(land, issue)?.contains("title = \"Sign in\""));
    Ok(issue)
}

/// A change made while the home's remote is away is kept, signed, in the
/// clone, and lands on the next sync.
async fn lands_an_offline_note(
    link: &HostLink,
    updates: &mut mpsc::Receiver<Update>,
    land: &Homeland,
    issue: proto::IssueId,
) -> Fallible {
    std::fs::rename(&land.home_bare, land.scratch.join("home.gone"))?;
    assert_eq!(
        send(
            link,
            change(issue, IssueChange::Note(String::from("offline")))
        )
        .await?,
        CommandResult::Accepted
    );
    let waiting = home_until(link, updates, |home| home.unpushed == 1).await?;
    let signed = format!("test-host · {}: offline", test_client().0);
    assert!(
        alpha_issues(&waiting)
            .iter()
            .flat_map(|shown| shown.notes.clone())
            .any(|note| note.ends_with(&signed)),
        "the clone has the note, signed by the host and the client, before the remote does"
    );
    std::fs::rename(land.scratch.join("home.gone"), &land.home_bare)?;
    assert_eq!(
        send(link, CommandBody::SyncHome).await?,
        CommandResult::Accepted
    );
    let _ = home_until(link, updates, |home| home.unpushed == 0).await?;
    let landed = pushed_issue(land, issue)?;
    assert!(
        landed.contains(&signed) && landed.contains("- by hand"),
        "{landed}"
    );
    Ok(())
}

#[tokio::test]
async fn issues_are_written_to_the_home_and_replayed_over_a_concurrent_push()
-> Fallible {
    let land = homeland("issues")?;
    let (fx, _) = host_at_home(land.home_url.clone(), Vec::new()).await?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let _ = home_until(&link, &mut updates, |home| home.projects.len() == 2)
        .await?;
    let issue = opens_an_issue(&link, &mut updates, &land).await?;
    noted_by_hand(&land, issue)?;
    assert_eq!(
        send(
            &link,
            change(issue, IssueChange::Status(IssueStatus::InProgress))
        )
        .await?,
        CommandResult::Accepted
    );
    let replayed = pushed_issue(&land, issue)?;
    assert!(
        replayed.contains("status = \"in-progress\"")
            && replayed.contains("- by hand"),
        "the host's change lands on top of the person's: {replayed}"
    );
    assert_eq!(
        send(
            &link,
            change(proto::IssueId(7), IssueChange::Note(String::from("x")))
        )
        .await?,
        CommandResult::Rejected(Rejection::UnknownIssue)
    );
    lands_an_offline_note(&link, &mut updates, &land, issue).await?;
    link.stop().await?;
    fx.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

fn on_issue(issue: proto::IssueId) -> CommandBody {
    CommandBody::StartSession {
        instance: instance(Provider::Claude),
        at: StartAt::Issue(issue),
        model: None,
        effort: None,
    }
}

#[tokio::test]
async fn a_session_on_an_issue_works_on_its_branch_from_the_issues_brief()
-> Fallible {
    let land = homeland("issue-sessions")?;
    let (fx, _) =
        host_at_home(land.home_url.clone(), Vec::from([land.roots.clone()]))
            .await?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let _ = home_until(&link, &mut updates, |home| {
        !checkouts_of(home, "alpha").is_empty()
    })
    .await?;
    let issue = opens_an_issue(&link, &mut updates, &land).await?;
    let session = started_session(&send(&link, on_issue(issue)).await?)?;
    let events = events_until(&mut updates, Until::Request).await?;
    let mine = events
        .iter()
        .filter(|event| event.session == session)
        .map(|event| event.kind.clone())
        .collect::<Vec<EventKind>>();
    assert!(mine.contains(&EventKind::OnIssue { issue }));
    let brief = mine.iter().find_map(|kind| match kind {
        EventKind::UserMessage { text, by } => Some((text.clone(), by.clone())),
        _ => None,
    });
    assert!(
        brief.as_ref().is_some_and(|(text, by)| {
            text.contains("\"Sign in\"")
                && text.contains("Why.")
                && *by == test_client()
        }),
        "the first message is the issue's brief: {brief:?}"
    );
    let branch = format!("issue/{}", home::issue::name(issue));
    let alpha = land.roots.join("apps").join("alpha");
    let session_branch = mine
        .iter()
        .find_map(|kind| match kind {
            EventKind::WorkspaceCreated { workspace } => {
                Some(workspace.branch.clone())
            }
            _ => None,
        })
        .ok_or("the session made no workspace")?;
    assert_eq!(
        git(&alpha, &["rev-parse", &session_branch])?,
        git(&alpha, &["rev-parse", &branch])?,
        "the session's branch is cut from the issue's"
    );
    assert_eq!(
        git(&alpha, &["rev-parse", &branch])?,
        git(&alpha, &["rev-parse", "HEAD"])?,
        "a new issue's branch starts at the checkout's HEAD"
    );
    let moved = home_until(&link, &mut updates, |home| {
        alpha_issues(home)
            .iter()
            .any(|shown| shown.status == IssueStatus::InProgress)
    })
    .await?;
    assert_eq!(moved.unpushed, 0);
    assert_eq!(
        send(&link, on_issue(issue)).await?,
        CommandResult::Rejected(Rejection::IssueTaken { session })
    );
    link.stop().await?;
    fx.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

#[tokio::test]
async fn finishing_lands_the_sessions_work_on_the_issue_notes_it_and_asks_review()
-> Fallible {
    let land = homeland("finish")?;
    let alpha = land.roots.join("apps").join("alpha");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e/claude-worktrees.jsonl"),
        alpha.join("stdout.jsonl"),
    )?;
    let (fx, _) =
        host_at_home(land.home_url.clone(), Vec::from([land.roots.clone()]))
            .await?;
    let (link, mut updates) = link(fx.target.clone(), None).await?;
    until_live(&mut updates).await?;
    let _ = home_until(&link, &mut updates, |home| {
        !checkouts_of(home, "alpha").is_empty()
    })
    .await?;
    let issue = opens_an_issue(&link, &mut updates, &land).await?;
    let session = started_session(&send(&link, on_issue(issue)).await?)?;
    let _ = events_until(&mut updates, Until::TurnEnd).await?;
    assert_eq!(
        send(&link, CommandBody::FinishSession { session }).await?,
        CommandResult::Accepted
    );
    let finishing =
        of(session, &events_until(&mut updates, Until::Finished).await?);
    let remote = git(&alpha, &["remote", "get-url", "origin"])?;
    assert_eq!(
        finishing.last(),
        Some(&EventKind::SessionFinished {
            finished: proto::Finished::Landed {
                commits: 1,
                kept: Kept::Pushed {
                    remote: String::from(remote.trim())
                },
            }
        })
    );
    assert!(finishing.contains(&EventKind::SessionClosed {
        reason: CloseReason::Stopped {
            reason: String::from(host::finish::FINISHED)
        }
    }));
    let branch = format!("issue/{}", home::issue::name(issue));
    let pushed = git(
        Path::new(remote.trim().trim_start_matches("file://")),
        &["show", &format!("{branch}:changed.txt")],
    )?;
    assert!(
        pushed.starts_with("turn two\n"),
        "what the worktree held when it finished landed and was pushed: {pushed}"
    );
    let reviewed = home_until(&link, &mut updates, |home| {
        alpha_issues(home)
            .iter()
            .any(|shown| shown.status == IssueStatus::Review)
    })
    .await?;
    let notes = alpha_issues(&reviewed)
        .into_iter()
        .flat_map(|shown| shown.notes)
        .collect::<Vec<String>>();
    assert!(
        notes.last().is_some_and(
            |note| note.ends_with("test-host · claude: Completed turn 2.")
        ),
        "the summary is the issue's last note: {notes:?}"
    );
    assert_eq!(
        send(
            &link,
            CommandBody::FinishSession {
                session: SessionId(1)
            }
        )
        .await?,
        CommandResult::Rejected(Rejection::UnknownSession)
    );
    link.stop().await?;
    fx.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}
