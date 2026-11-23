#![cfg(test)]

//! Host and client in one process over real iroh connections on loopback, with
//! relays off so the test needs no network.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use daemon::{HostOptions, Hosted, Plugins};
mod credential_sign_in;
mod disk;
mod raw;
use iroh::endpoint::presets;
use iroh::{Endpoint, RelayMode};
use net::Target;
use proto::{
    Answer, Auth, AuthToken, Availability, Change, ChecklistEntry,
    ChecklistStatus, ClientName, CloseReason, CommandBody, CommandResult,
    Credential, Decision, Effort, Event, EventKind, Feed, FileChange, HostPath,
    HostSeq, InstanceId, InstanceStatus, Instances, IssueChange, IssueId, Kept,
    ModelChoice, ModelId, Models, ProjectId, Provider, Question,
    QuestionOption, Rejection, RequestId, Resolution, Roster, Selection,
    SessionId, StartAt, SubagentActivity, SubagentEvent, SubagentId,
    SubagentOutcome, ToolCallId, ToolKind, ToolOutcome, ToolPhase, TurnOutcome,
};
use raw::{RawClient, Received, Recorder};
use runtime::Settings;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

const STEP: Duration = Duration::from_secs(10);
const REFUSING_CLI: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/e2e/refuse-version.sh"
);
const UNAVAILABLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/e2e/refuse-version.sh --version exited with Some(44)"
);

type Fallible<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn endpoint() -> Fallible<Endpoint> {
    Ok(Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?)
}

struct Fixture {
    host: Hosted,
    target: Target,
}

impl Fixture {
    fn address(&self) -> Fallible<SocketAddr> {
        self.target
            .addr
            .ip_addrs()
            .copied()
            .find(SocketAddr::is_ipv4)
            .map(|bound| SocketAddr::from((Ipv4Addr::LOCALHOST, bound.port())))
            .ok_or_else(|| "the host has no IPv4 address".into())
    }

    async fn shutdown(self) -> Fallible {
        Ok(self.host.shutdown().await?)
    }
}

fn fake_claude() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../claude-cli/tests/fake-claude.sh")
}

fn fake_codex() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../codex-cli/tests/fake-codex.sh")
}

fn recording_file(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/e2e")
        .join(name)
}

fn plugins(claude: Option<PathBuf>, codex: Option<PathBuf>) -> Plugins {
    [
        claude_cli::plugin(Settings {
            path: claude,
            model: None,
        }),
        codex_cli::plugin(Settings {
            path: codex,
            model: None,
        }),
    ]
    .into_iter()
    .collect()
}

fn claude_plugins() -> Plugins {
    plugins(Some(fake_claude()), Some(PathBuf::from(REFUSING_CLI)))
}

async fn host_with(claude_path: PathBuf) -> Fallible<Fixture> {
    let endpoint = endpoint().await?;
    let data = data_dir(&endpoint.id().to_string())?;
    standard_claude_probe(&data)?;
    serve(
        endpoint,
        plugins(Some(claude_path), Some(PathBuf::from(REFUSING_CLI))),
        &data,
    )
    .await
}

async fn host_at_home(
    remote: String,
    roots: Vec<PathBuf>,
) -> Fallible<(Fixture, PathBuf)> {
    host_at_home_for(remote, roots, None).await
}

/// A host at the home whose skills go to the user at `user`, if any.
async fn host_at_home_for(
    remote: String,
    roots: Vec<PathBuf>,
    user: Option<PathBuf>,
) -> Fallible<(Fixture, PathBuf)> {
    let endpoint = endpoint().await?;
    let data_dir = data_dir(&endpoint.id().to_string())?;
    standard_claude_probe(&data_dir)?;
    let fixture = assemble(
        endpoint,
        claude_plugins(),
        HostOptions {
            name: "test-host".into(),
            data_dir: data_dir.clone(),
            home: Some(remote),
            projects: roots,
            user,
        },
    )
    .await?;
    Ok((fixture, data_dir))
}

async fn codex_host(recording: &str) -> Fallible<(Fixture, PathBuf)> {
    codex_host_on(recording, instance(Provider::Codex)).await
}

async fn codex_host_on(
    recording: &str,
    id: InstanceId,
) -> Fallible<(Fixture, PathBuf)> {
    codex_host_from(recording, id, &recording_file(recording)).await
}

async fn codex_host_from(
    recording: &str,
    id: InstanceId,
    source: &Path,
) -> Fallible<(Fixture, PathBuf)> {
    let data = data_dir(recording)?;
    if id == api(Provider::Codex) {
        seed(&data, std::slice::from_ref(&id), &[Provider::Codex])?;
    }
    let server = codex_server_from(&data, &id, recording, source)?;
    let fx = serve(
        endpoint().await?,
        plugins(Some(PathBuf::from(REFUSING_CLI)), Some(fake_codex())),
        &data,
    )
    .await?;
    Ok((fx, server))
}

fn codex_server(
    data: &Path,
    instance: &InstanceId,
    name: &str,
) -> Fallible<PathBuf> {
    codex_server_from(data, instance, name, &recording_file(name))
}

fn codex_server_from(
    data: &Path,
    instance: &InstanceId,
    name: &str,
    source: &Path,
) -> Fallible<PathBuf> {
    let server = dir_of(data, "work", instance);
    std::fs::create_dir_all(&server)?;
    let wire = read(source)?;
    let played = if name.ends_with("-listed.jsonl") {
        wire
    } else {
        with_model_list(&wire)?
    };
    std::fs::write(server.join("stdout.jsonl"), played)?;
    committed_repository(&server)?;
    Ok(server)
}

fn with_model_list(wire: &str) -> Fallible<String> {
    let mut played = String::new();
    for line in wire.lines() {
        let mut frame: serde_json::Value = serde_json::from_str(line)?;
        let reply = frame["id"]
            .as_u64()
            .filter(|_| frame.get("method").is_none());
        if let Some(id @ 2..) = reply {
            frame["id"] = serde_json::json!(id.saturating_add(1));
        }
        if frame["fake"] == "hang_up" {
            let lines = frame["after"]
                .as_u64()
                .ok_or("missing stdin close threshold")?;
            frame["after"] = serde_json::json!(lines.saturating_add(1));
        }
        writeln!(played, "{frame}")?;
        if reply == Some(1) {
            writeln!(played, "{CODEX_MODELS}")?;
        }
    }
    Ok(played)
}

const CODEX_MODELS: &str = r#"{"id":2,"result":{"data":[{"id":"big","displayName":"Big","description":"thinks","isDefault":true,"supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"}]},{"id":"small","displayName":"Small","description":"quick","supportedReasoningEfforts":[{"reasoningEffort":"low"}]}]}}"#;

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
    std::fs::write(dir.join("store.key.file"), "")?;
    Ok(dir)
}

async fn serve(
    endpoint: Endpoint,
    plugins: Plugins,
    data: &Path,
) -> Fallible<Fixture> {
    assemble(
        endpoint,
        plugins,
        HostOptions {
            name: "test-host".into(),
            data_dir: data.to_path_buf(),
            home: None,
            projects: Vec::new(),
            user: None,
        },
    )
    .await
}

async fn assemble(
    endpoint: Endpoint,
    plugins: Plugins,
    options: HostOptions,
) -> Fallible<Fixture> {
    let host =
        tokio::time::timeout(STEP, daemon::start(endpoint, plugins, options))
            .await??;
    tokio::time::timeout(STEP, host.initialized()).await??;
    let target = host.target();
    Ok(Fixture { host, target })
}

async fn durable_host(data: &Path, address: SocketAddr) -> Fallible<Fixture> {
    let endpoint = durable_endpoint(data, address).await?;
    standard_claude_probe(data)?;
    serve(endpoint, claude_plugins(), data).await
}

async fn durable_endpoint(
    data: &Path,
    address: SocketAddr,
) -> Fallible<Endpoint> {
    let key_path = data.join("test-endpoint.key");
    let key = if key_path.exists() {
        iroh::SecretKey::from_bytes(
            &std::fs::read(&key_path)?
                .try_into()
                .map_err(|_| "invalid test endpoint key")?,
        )
    } else {
        let key = iroh::SecretKey::generate();
        std::fs::write(key_path, key.to_bytes())?;
        key
    };
    Ok(Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .secret_key(key)
        .bind_addr(address)?
        .bind()
        .await?)
}

/// The events up to and including `last`.
async fn events_through(
    client: &mut RawClient,
    last: HostSeq,
) -> Fallible<Vec<Event>> {
    client.events_until(|event| event.seq >= last).await
}

fn legacy_created(instance: InstanceId, directory: &Path) -> EventKind {
    EventKind::SessionCreated {
        instance,
        directory: HostPath(directory.display().to_string()),
        by: test_client(),
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

/// One command and the host's literal answer.
struct Case {
    body: CommandBody,
    result: CommandResult,
}

async fn host() -> Fallible<Fixture> {
    host_with(fake_claude()).await
}

fn start(provider: Provider, directory: &Path) -> CommandBody {
    start_on(instance(provider), directory)
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

/// The ALPN before the current one: the same name, one version lower.
fn previous_alpn() -> Fallible<Vec<u8>> {
    let (name, version) = std::str::from_utf8(proto::ALPN)?
        .rsplit_once('/')
        .ok_or("the ALPN has no version")?;
    let previous = version
        .parse::<u32>()?
        .checked_sub(1)
        .ok_or("the ALPN has no previous version")?;
    Ok(format!("{name}/{previous}").into_bytes())
}

#[tokio::test]
async fn a_client_offering_the_previous_alpn_fails_the_handshake() -> Fallible {
    let fx = host().await?;
    let endpoint = endpoint().await?;
    let previous = previous_alpn()?;
    assert_ne!(previous, proto::ALPN);
    assert!(
        endpoint
            .connect(fx.target.addr.clone(), &previous)
            .await
            .is_err()
    );
    let mut feed = Recorder::new()?;
    let current = RawClient::live(&fx.target, "test-client", &mut feed).await?;
    current.close().await;
    endpoint.close().await;
    feed.write()?;
    fx.shutdown().await
}

#[tokio::test]
async fn incomplete_hello_expires_while_established_clients_keep_working()
-> Fallible {
    let fx = host().await?;
    let mut feed = Recorder::new()?;
    let established =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let endpoint = endpoint().await?;
    let incomplete = endpoint
        .connect(fx.target.addr.clone(), proto::ALPN)
        .await?;
    let (mut hello, _reply) = incomplete.open_bi().await?;
    hello.write_all(&[0, 0]).await?;

    let command = || {
        start(
            Provider::Claude,
            Path::new("/no-such-conductor-test-directory"),
        )
    };
    assert_eq!(
        established.send(command()).await?.value,
        CommandResult::Rejected(Rejection::NoSuchDirectory)
    );
    let reason = tokio::time::timeout(STEP, incomplete.closed()).await?;
    assert!(matches!(
        reason,
        iroh::endpoint::ConnectionError::ApplicationClosed(_)
    ));
    assert_eq!(
        established.send(command()).await?.value,
        CommandResult::Rejected(Rejection::NoSuchDirectory)
    );
    established.close().await;
    endpoint.close().await;
    feed.write()?;
    fx.shutdown().await
}

#[tokio::test]
async fn a_client_goes_live_and_start_session_is_checked_at_the_boundary()
-> Fallible {
    let fx = host().await?;
    let mut feed = Recorder::new()?;
    let phone = RawClient::live(&fx.target, "test-client", &mut feed).await?;
    assert_eq!(phone.welcome.host, "test-host");
    assert_eq!(
        phone.instances,
        statuses(BTreeMap::from([
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
        ]))
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
    for case in cases {
        assert_eq!(phone.send(case.body).await?.value, case.result);
    }
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

async fn feed_session(
    client: &mut RawClient,
    name: &str,
) -> Fallible<SessionId> {
    let source = scratch(name, "claude-feed.jsonl")?;
    start_ready(client, instance(Provider::Claude), &source).await
}

async fn feed_turn(
    client: &mut RawClient,
    session: SessionId,
    text: &str,
) -> Fallible<Vec<Event>> {
    client.accepted(message(session, text)).await?;
    raw_events_until(client, Until::TurnEnd).await
}

#[tokio::test]
async fn reconnect_mid_feed_resumes_after_the_cursor_without_gaps_or_duplicates()
-> Fallible {
    let fx = host().await?;
    let mut feed = Recorder::new()?;
    let mut writer =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let mut reader = RawClient::live(&fx.target, "reader", &mut feed).await?;
    let session = feed_session(&mut writer, "reconnect-feed").await?;
    let first = feed_turn(&mut writer, session, "one").await?;
    let mut observed = events_through(
        &mut reader,
        first.last().ok_or("missing first turn")?.seq,
    )
    .await?;
    let applied = observed
        .iter()
        .map(|event| event.seq)
        .max()
        .ok_or("the reader applied nothing")?;
    writer.accepted(message(session, "two")).await?;
    reader.close().await;
    // The second turn ends while the reader is away, so it resumes from the
    // stored log alone and a rerun reads the same frames.
    raw_conversation_events_until(&mut writer, Until::TurnEnd).await?;
    let mut reader = RawClient::open(&fx.target, "reader", &mut feed).await?;
    reader.subscribe(applied).await?;
    observed.extend(raw_events(reader.caught_up().await?));
    let third = feed_turn(&mut writer, session, "three").await?;
    observed.extend(
        events_through(
            &mut reader,
            third.last().ok_or("missing third turn")?.seq,
        )
        .await?,
    );
    observed.retain(|event| {
        !matches!(event.kind, EventKind::AssistantDelta { .. })
    });
    assert_eq!(observed, raw_history(&fx.target, &mut feed).await?);
    assert_eq!(
        seqs(&observed),
        (1..=observed.len() as u64).map(HostSeq).collect::<Vec<_>>()
    );
    reader.close().await;
    writer.close().await;
    feed.write()?;
    fx.shutdown().await
}

#[tokio::test]
async fn a_new_client_resumes_from_a_dead_clients_cursor() -> Fallible {
    let fx = host().await?;
    let mut feed = Recorder::new()?;
    let mut writer =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let mut reader = RawClient::live(&fx.target, "reader", &mut feed).await?;
    let session = feed_session(&mut writer, "dead-client").await?;
    let first = feed_turn(&mut writer, session, "one").await?;
    let seq = first.last().ok_or("missing first turn")?.seq;
    events_through(&mut reader, seq).await?;
    let epoch = reader.welcome.epoch;
    reader.close().await;
    let second = feed_turn(&mut writer, session, "two").await?;
    let mut reader = RawClient::open(&fx.target, "reader", &mut feed).await?;
    assert_eq!(reader.welcome.epoch, epoch);
    reader.subscribe(seq).await?;
    let rest = raw_events(reader.caught_up().await?);
    assert_eq!(
        rest,
        second
            .into_iter()
            .filter(|event| !matches!(
                event.kind,
                EventKind::AssistantDelta { .. }
            ))
            .collect::<Vec<_>>()
    );
    reader.close().await;
    writer.close().await;
    feed.write()?;
    fx.shutdown().await
}

#[tokio::test]
async fn a_live_client_sees_the_deltas_and_a_resumed_one_only_the_message()
-> Fallible {
    let fx = host().await?;
    let mut feed = Recorder::new()?;
    let mut first =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = feed_session(&mut first, "live-deltas").await?;
    let seen = raw_history(&fx.target, &mut feed).await?;
    let live = feed_turn(&mut first, session, "say hello").await?;
    let assistant = |events: &[Event]| {
        of(session, events)
            .into_iter()
            .filter(|kind| {
                matches!(
                    kind,
                    EventKind::AssistantDelta { .. }
                        | EventKind::AssistantMessage { .. }
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        assistant(&live),
        [delta("Hel"), delta("lo"), reply("Hello")]
    );
    let window = live
        .windows(4)
        .find(|events| events[1].kind == delta("Hel"))
        .ok_or("no durable event before the first delta")?;
    let prior = window[0].seq;
    assert_eq!(seqs(&window[1..]), [prior, prior, prior.next()]);
    let epoch = first.welcome.epoch;
    first.close().await;
    let mut resumed =
        RawClient::open(&fx.target, "test-client", &mut feed).await?;
    assert_eq!(resumed.welcome.epoch, epoch);
    resumed
        .subscribe(seen.last().ok_or("missing opening events")?.seq)
        .await?;
    let rest = raw_events(resumed.caught_up().await?);
    assert_eq!(assistant(&rest), [reply("Hello")]);
    assert_eq!(
        rest,
        live.into_iter()
            .filter(|event| !matches!(
                event.kind,
                EventKind::AssistantDelta { .. }
            ))
            .collect::<Vec<_>>()
    );
    resumed.close().await;
    feed.write()?;
    fx.shutdown().await
}

#[tokio::test]
async fn a_cursor_from_another_epoch_starts_over_from_the_beginning() -> Fallible
{
    let data = data_dir("epoch-reset")?;
    let first =
        durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let address = first.address()?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&first.target, "test-client", &mut feed).await?;
    let earlier = scratch("epoch-earlier", "claude-feed.jsonl")?;
    let gone =
        start_ready(&mut phone, instance(Provider::Claude), &earlier).await?;
    let applied = feed_turn(&mut phone, gone, "say hello")
        .await?
        .last()
        .ok_or("missing turn")?
        .seq;
    let stale = phone.welcome.epoch;
    phone.close().await;
    first.shutdown().await?;
    // A host whose log is gone starts a new one, in a new epoch, under the
    // same ticket.
    std::fs::remove_file(data.join("conductor.redb"))?;
    let second = durable_host(&data, address).await?;
    let mut writer =
        RawClient::live(&second.target, "test-client", &mut feed).await?;
    let epoch = writer.welcome.epoch;
    assert_ne!(epoch, stale);
    let source = scratch("epoch-reset", "claude-feed.jsonl")?;
    let session =
        start_ready(&mut writer, instance(Provider::Claude), &source).await?;
    feed_turn(&mut writer, session, "say hello").await?;
    writer.close().await;

    let mut phone =
        RawClient::open(&second.target, "test-client", &mut feed).await?;
    assert_eq!(phone.welcome.epoch, epoch);
    phone.subscribe_in(stale, applied).await?;
    let refused = phone.next().await.err().map(|error| error.to_string());
    assert_eq!(refused.as_deref(), Some("the host finished the stream"));
    phone.subscribe(HostSeq(0)).await?;
    let events = raw_events(phone.caught_up().await?);
    assert_eq!(events.first().map(|event| event.seq), Some(HostSeq(1)));
    assert!(events.iter().all(|event| event.session == session));
    assert_eq!(
        of(session, &events),
        [
            opening(Provider::Claude, &source),
            Vec::from([
                user("say hello"),
                reply("Hello"),
                turn_end(TurnOutcome::Completed),
            ]),
        ]
        .concat()
    );
    assert_eq!(events, raw_history(&second.target, &mut feed).await?);
    phone.close().await;
    feed.write()?;
    second.shutdown().await
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
    std::fs::copy(recording_file(recording), dir.join("stdout.jsonl"))?;
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
    Removed,
    Finished,
}

const fn reached(until: &Until, kind: &EventKind) -> bool {
    matches!(
        (until, kind),
        (Until::Ready, EventKind::SessionReady)
            | (
                Until::Request,
                EventKind::RequestOpened { .. }
                    | EventKind::Subagent {
                        event: SubagentEvent::RequestOpened { .. },
                        ..
                    }
            )
            | (Until::Question, EventKind::QuestionAsked { .. })
            | (Until::Resolved, EventKind::RequestResolved { .. })
            | (Until::Delta, EventKind::AssistantDelta { .. })
            | (Until::TurnEnd, EventKind::TurnCompleted { .. })
            | (Until::Closed, EventKind::SessionClosed { .. })
            | (Until::Removed, EventKind::SessionRemoved { .. })
            | (Until::Finished, EventKind::SessionFinished { .. })
    )
}

/// The kinds of the conversation events the raw client receives, through
/// the first that reaches `until`, for tests that start one session.
async fn raw_kinds_until(
    client: &mut RawClient,
    until: Until,
) -> Fallible<Vec<EventKind>> {
    Ok(client
        .events_until(|event| reached(&until, &event.kind))
        .await?
        .into_iter()
        .map(|event| event.kind)
        .filter(conversation_event)
        .collect())
}

/// The events a client subscribing from the start of the host's log
/// replays.
async fn raw_history(
    target: &Target,
    feed: &mut Recorder,
) -> Fallible<Vec<Event>> {
    let mut late = RawClient::open(target, "test-client", feed).await?;
    late.subscribe(HostSeq(0)).await?;
    let events = late
        .caught_up()
        .await?
        .into_iter()
        .filter_map(|frame| match frame.value {
            Feed::Event(event) => Some(event),
            _ => None,
        })
        .collect();
    late.close().await;
    Ok(events)
}

async fn raw_ping_turn(client: &mut RawClient, session: SessionId) -> Fallible {
    client.accepted(message(session, "ping")).await?;
    assert_eq!(
        raw_kinds_until(client, Until::TurnEnd).await?,
        [
            user("ping"),
            delta("pong"),
            reply("pong"),
            turn_end(TurnOutcome::Completed)
        ]
    );
    Ok(())
}

async fn raw_resume_to_ready(
    client: &mut RawClient,
    session: SessionId,
) -> Fallible {
    client.accepted(continued(session)).await?;
    assert_eq!(
        raw_kinds_until(client, Until::Ready).await?,
        [EventKind::SessionReady]
    );
    Ok(())
}

/// The events the raw client receives, through the first that reaches
/// `until`.
async fn raw_events_until(
    client: &mut RawClient,
    until: Until,
) -> Fallible<Vec<Event>> {
    client
        .events_until(|event| reached(&until, &event.kind))
        .await
}

/// The events through the first that reaches `until`, without the workspace
/// and snapshot metadata, which have tests of their own.
async fn raw_conversation_events_until(
    client: &mut RawClient,
    until: Until,
) -> Fallible<Vec<Event>> {
    let mut events = raw_events_until(client, until).await?;
    events.retain(|event| conversation_event(&event.kind));
    Ok(events)
}

/// The events among `frames`.
fn raw_events(frames: Vec<Received<Feed>>) -> Vec<Event> {
    frames
        .into_iter()
        .filter_map(|frame| match frame.value {
            Feed::Event(event) => Some(event),
            _ => None,
        })
        .collect()
}

const fn conversation_event(kind: &EventKind) -> bool {
    !matches!(
        kind,
        EventKind::WorkspaceCreated { .. }
            | EventKind::SnapshotRecorded { .. }
            | EventKind::NativeTurnStarted { .. }
    )
}

/// The kinds of `session`'s events.
fn of(session: SessionId, events: &[Event]) -> Vec<EventKind> {
    events
        .iter()
        .filter(|event| event.session == session)
        .filter(|event| conversation_event(&event.kind))
        .map(|event| event.kind.clone())
        .collect()
}

/// An issue command is answered with the issue it wrote and no warning.
async fn wrote(client: &RawClient, body: CommandBody) -> Fallible {
    assert!(matches!(
        client.send(body).await?.value,
        CommandResult::IssueWritten { warnings, .. } if warnings.is_empty()
    ));
    Ok(())
}

async fn configured(
    client: &RawClient,
    body: CommandBody,
    target: &Target,
) -> Fallible<Instances> {
    client.accepted(body).await?;
    current_offers(target).await
}

async fn current_offers(target: &Target) -> Fallible<Instances> {
    let observer = endpoint().await?;
    let (conn, welcome) = hello(&observer, target).await?;
    let offers = subscribed_offers(&conn, &welcome).await;
    observer.close().await;
    offers
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

/// A Codex command the user decided on, as the client shows it.
fn codex_tool(
    call: &str,
    command: &str,
    decision: Decision,
    outcome: ToolOutcome,
) -> Vec<EventKind> {
    let id = ToolCallId(String::from(call));
    let tool = ToolKind::Command {
        command: String::from(command),
    };
    Vec::from([
        EventKind::ToolCall {
            call: id.clone(),
            phase: ToolPhase::Started { tool: tool.clone() },
        },
        EventKind::RequestOpened {
            request: RequestId(1),
            call: id.clone(),
            tool,
            reason: None,
            for_session: Some(String::from(command)),
        },
        EventKind::RequestResolved {
            request: RequestId(1),
            resolution: Resolution::Decided {
                decision,
                by: test_client(),
            },
        },
        EventKind::ToolCall {
            call: id,
            phase: ToolPhase::Finished { outcome },
        },
    ])
}

const APPROVED_CALL: &str = "toolu_bdrk_011tLLSAecKzNjYWMVcNwMfD";
const DENIED_CALL: &str = "toolu_bdrk_01WAz64VUtaiJZHcy71oavyx";

#[tokio::test]
async fn a_claude_turn_streams_asks_and_finishes() -> Fallible {
    let fx = host().await?;
    let dir = scratch("approve", "claude-approve.jsonl")?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = started_session(
        &phone.send(start(Provider::Claude, &dir)).await?.value,
    )?;
    let sent = phone.send(message(session, "create a.txt")).await?;
    assert_eq!(sent.raw, br#""Accepted""#);
    assert_eq!(
        phone.send(message(session, "and b.txt")).await?.value,
        CommandResult::Rejected(Rejection::TurnInProgress)
    );
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Request).await?,
        [
            opening(Provider::Claude, &dir),
            Vec::from([user("create a.txt")]),
            write_asked(APPROVED_CALL, "/tmp/s1-scratch/a.txt"),
        ]
        .concat()
    );
    let desk = RawClient::live(&fx.target, "desk", &mut feed).await?;
    assert_eq!(
        [
            phone
                .send(resolve(session, 1, Decision::Approve))
                .await?
                .value,
            desk.send(resolve(session, 1, Decision::Deny)).await?.value,
            phone
                .send(resolve(session, 2, Decision::Approve))
                .await?
                .value,
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
        raw_kinds_until(&mut phone, Until::TurnEnd).await?,
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
    phone.close().await;
    desk.close().await;
    feed.write()?;
    fx.shutdown().await
}

#[tokio::test]
async fn removing_a_session_stops_it_and_removes_its_workspace() -> Fallible {
    let fx = host().await?;
    let dir = scratch("remove", "claude-approve.jsonl")?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &dir)).await?;
    let opened = raw_events_until(&mut phone, Until::Ready).await?;
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
    phone
        .accepted(CommandBody::RemoveSession { session })
        .await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Removed).await?,
        [
            EventKind::SessionClosed {
                reason: CloseReason::Stopped {
                    reason: String::from("the session was removed"),
                },
            },
            EventKind::SessionRemoved {
                kept: Some(Kept::Local),
            },
        ]
    );
    assert!(!worktree.exists());
    assert_eq!(
        phone.send(CommandBody::Continue { session }).await?.value,
        CommandResult::Rejected(Rejection::SessionRemoved)
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

#[tokio::test]
async fn a_denied_request_continues_and_an_interrupt_ends_the_turn() -> Fallible
{
    let fx = host().await?;
    let dir = scratch("deny", "claude-deny-interrupt.jsonl")?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &dir)).await?;
    assert_eq!(
        phone.send(interrupt(session)).await?.value,
        CommandResult::Rejected(Rejection::NoTurnRunning)
    );
    let _ = phone.send(message(session, "create b.txt")).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Request).await?,
        [
            opening(Provider::Claude, &dir),
            Vec::from([user("create b.txt")]),
            write_asked(DENIED_CALL, "/tmp/s1-scratch/b.txt"),
        ]
        .concat()
    );
    phone.accepted(resolve(session, 1, Decision::Deny)).await?;
    let text = "The write to `b.txt` was denied (\"The user does not want b.txt.\"), so I did not retry and no file was created.";
    assert_eq!(
        raw_kinds_until(&mut phone, Until::TurnEnd).await?,
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

    let _ = phone.send(message(session, "count to 100")).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Delta).await?,
        [user("count to 100"), delta("1")]
    );
    phone.accepted(interrupt(session)).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::TurnEnd).await?,
        [reply("1"), turn_end(TurnOutcome::Interrupted)]
    );

    raw_ping_turn(&mut phone, session).await?;
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

/// The recording starts the turn only once `release` exists, and the fake
/// answers only an interrupt sent after that (Codex's of that turn's id), so
/// this passes only if the session holds the interrupt until the turn starts.
async fn held_interrupt(
    fx: Fixture,
    dir: PathBuf,
    provider: Provider,
) -> Fallible {
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(provider, &dir)).await?;
    let _ = phone.send(message(session, "count to 300")).await?;
    phone.accepted(interrupt(session)).await?;
    std::fs::write(dir.join("release"), "")?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::TurnEnd).await?,
        [
            opening(provider, &dir),
            Vec::from([
                user("count to 300"),
                turn_end(TurnOutcome::Interrupted)
            ]),
        ]
        .concat()
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

/// The host's log stays in `target/tmp` when this fails, to show which step
/// the turn stalled on.
#[tokio::test]
async fn an_interrupt_before_the_turn_starts_is_held_until_it_does() -> Fallible
{
    let (log, _logging) = capture_log("held")?;
    let dir = scratch("held", "claude-held-interrupt.jsonl")?;
    held_interrupt(host().await?, dir, Provider::Claude).await?;
    let _ = captured(&log)?;
    Ok(())
}

/// The CLI withdraws the open request when the turn is interrupted, and the
/// tool call it asked about fails.
#[tokio::test]
async fn an_interrupt_withdraws_the_open_request() -> Fallible {
    let fx = host().await?;
    let dir = scratch("withdraw", "claude-withdraw.jsonl")?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &dir)).await?;
    let _ = phone.send(message(session, "create d.txt")).await?;
    let call = "toolu_bdrk_01KEnaUr83k9BPcLoGpArHUq";
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Request).await?,
        [
            opening(Provider::Claude, &dir),
            Vec::from([user("create d.txt")]),
            write_asked(call, "/tmp/s1-scratch/d.txt"),
        ]
        .concat()
    );
    phone.accepted(interrupt(session)).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::TurnEnd).await?,
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
        phone
            .send(resolve(session, 1, Decision::Approve))
            .await?
            .value,
        CommandResult::Rejected(Rejection::UnknownRequest)
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
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
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &dir)).await?;
    let _ = phone.send(message(session, "ask me")).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Question).await?,
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
    let desk = RawClient::live(&fx.target, "desk", &mut feed).await?;
    assert_eq!(
        [
            phone
                .send(resolve(session, 1, Decision::Approve))
                .await?
                .value,
            phone
                .send(CommandBody::Answer {
                    session,
                    request: RequestId(1),
                    answers: Vec::new(),
                })
                .await?
                .value,
            phone.send(answer(session, 1, &["Blue"])).await?.value,
            desk.send(answer(session, 1, &["Red"])).await?.value,
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
        raw_kinds_until(&mut phone, Until::TurnEnd).await?,
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
    phone.close().await;
    desk.close().await;
    feed.write()?;
    fx.shutdown().await
}

/// Codex asks with `requestUserInput`, answered as an RPC result keyed by
/// the question's id, which the stand-in waits for.
#[tokio::test]
async fn a_codex_question_is_answered_by_its_question_id() -> Fallible {
    let (fx, dir) = codex_host("codex-question.jsonl").await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Codex, &dir)).await?;
    let _ = phone.send(message(session, "ask me")).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Question).await?,
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
    phone.accepted(answer(session, 1, &["Blue"])).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::TurnEnd).await?,
        [
            answered_blue(),
            delta("Blue"),
            reply("Blue"),
            turn_end(TurnOutcome::Completed),
        ]
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

/// Approving for the session adds the CLI's suggested rule for the session
/// only, so the same command runs again without a request.
#[tokio::test]
async fn a_claude_call_approved_for_the_session_runs_again_without_asking()
-> Fallible {
    let fx = host().await?;
    let dir = scratch("for-session", "claude-for-session.jsonl")?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &dir)).await?;
    let _ = phone.send(message(session, "touch t1.txt")).await?;
    let (first, again) = (
        "toolu_bdrk_0194hqw8EXmFKbEqNMeNMkWf",
        "toolu_bdrk_015276Epur1cWgLdAb2EXFPF",
    );
    let touch = ToolKind::Command {
        command: String::from("touch t1.txt"),
    };
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Request).await?,
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
    phone
        .accepted(resolve(session, 1, Decision::ApproveForSession))
        .await?;
    let first_turn = raw_kinds_until(&mut phone, Until::TurnEnd).await?;
    let _ = phone.send(message(session, "again")).await?;
    let second_turn = raw_kinds_until(&mut phone, Until::TurnEnd).await?;
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
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

/// Codex takes `acceptForSession` for a command, which the stand-in waits
/// for, and runs the same command again without a request.
#[tokio::test]
async fn a_codex_command_accepted_for_the_session_runs_again_without_asking()
-> Fallible {
    let (fx, dir) = codex_host("codex-for-session.jsonl").await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Codex, &dir)).await?;
    let _ = phone.send(message(session, "touch t1.txt")).await?;
    let command = "/usr/bin/zsh -c 'touch t1.txt'";
    let (first, again) = (
        "exec-6a70037b-22b5-482b-955f-e442841e7e21",
        "exec-0e77cebc-6940-4892-b90c-558be7ccb2b4",
    );
    let touch = ToolKind::Command {
        command: String::from(command),
    };
    let asked =
        calls_and_requests(raw_kinds_until(&mut phone, Until::Request).await?);
    phone
        .accepted(resolve(session, 1, Decision::ApproveForSession))
        .await?;
    let first_turn = raw_kinds_until(&mut phone, Until::TurnEnd).await?;
    let _ = phone.send(message(session, "again")).await?;
    let second_turn = raw_kinds_until(&mut phone, Until::TurnEnd).await?;
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
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

/// An interrupt withdraws an open question, as it does an approval: Claude
/// cancels it, Codex withdraws it after the turn ends, and either way it is
/// withdrawn and a late answer is refused.
#[tokio::test]
async fn an_interrupt_withdraws_an_open_question() -> Fallible {
    let fx = host().await?;
    let dir = scratch("question-withdraw", "claude-question-withdraw.jsonl")?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &dir)).await?;
    let _ = phone.send(message(session, "ask me")).await?;
    let mut events =
        raw_conversation_events_until(&mut phone, Until::Question).await?;
    phone.accepted(interrupt(session)).await?;
    events.extend(
        raw_conversation_events_until(&mut phone, Until::TurnEnd).await?,
    );
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
        phone.send(answer(session, 1, &["Pear"])).await?.value,
        CommandResult::Rejected(Rejection::UnknownRequest)
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

#[tokio::test]
async fn a_codex_interrupt_withdraws_the_open_question_at_the_turn_end()
-> Fallible {
    let (fx, dir) = codex_host("codex-question-withdraw.jsonl").await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Codex, &dir)).await?;
    let _ = phone.send(message(session, "ask me")).await?;
    let question = raw_kinds_until(&mut phone, Until::Question).await?;
    phone.accepted(interrupt(session)).await?;
    assert_eq!(
        (
            question.last().cloned(),
            raw_kinds_until(&mut phone, Until::Resolved).await?
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
        phone.send(answer(session, 1, &["Pear"])).await?.value,
        CommandResult::Rejected(Rejection::UnknownRequest)
    );
    std::fs::write(dir.join("release"), "")?;
    assert_eq!(sent_to_agent(&dir)?.matches("\"answers\"").count(), 0);
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

/// The stand-in stops reading after `initialize`, so the message can't
/// reach it. The turn must end, or every client shows it running forever.
#[tokio::test]
async fn a_message_the_agent_never_gets_fails_its_turn() -> Fallible {
    let fx = host().await?;
    let dir = scratch("hang-up", "claude-hang-up.jsonl")?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &dir)).await?;
    phone.accepted(message(session, "hi")).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::TurnEnd).await?,
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
        phone.send(interrupt(session)).await?.value,
        CommandResult::Rejected(Rejection::NoTurnRunning)
    );
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Closed).await?,
        [EventKind::SessionClosed {
            reason: CloseReason::Exited {
                status: String::from("exit status: 0"),
            },
        }]
    );
    assert_eq!(
        phone.send(message(session, "hi")).await?.value,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

#[tokio::test]
async fn a_missing_claude_binary_closes_the_session() -> Fallible {
    let tools = data_dir("missing-binary-tools")?;
    let script = tools.join("claude");
    wrapper(&script, &fake_claude())?;
    let fx = host_with(script.clone()).await?;
    std::fs::remove_file(script)?;
    let dir = scratch("missing-binary", "claude-approve.jsonl")?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &dir)).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Closed).await?,
        [
            EventKind::SessionCreated {
                instance: instance(Provider::Claude),
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
        phone.send(message(session, "hi")).await?.value,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

/// Two sessions' turns run at once on one app-server: each session's events
/// are its own thread's, one approves and the other declines.
#[tokio::test]
async fn two_codex_sessions_share_one_server() -> Fallible {
    let (fx, dir) = codex_host("codex-two-threads.jsonl").await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let first_session = phone.started(start(Provider::Codex, &dir)).await?;
    // The second session starts once the first has its thread, so the first
    // gets the recording's first thread.
    raw_conversation_events_until(&mut phone, Until::Ready).await?;
    let second_session = phone.started(start(Provider::Codex, &dir)).await?;
    assert_ne!(first_session, second_session);
    raw_conversation_events_until(&mut phone, Until::Ready).await?;
    phone
        .accepted(message(first_session, "create a.txt"))
        .await?;
    phone
        .accepted(message(second_session, "create b.txt"))
        .await?;
    let asked = [
        raw_conversation_events_until(&mut phone, Until::Request).await?,
        raw_conversation_events_until(&mut phone, Until::Request).await?,
    ]
    .concat();
    phone
        .accepted(resolve(first_session, 1, Decision::Approve))
        .await?;
    phone
        .accepted(resolve(second_session, 1, Decision::Deny))
        .await?;
    let ended = [
        asked,
        raw_conversation_events_until(&mut phone, Until::TurnEnd).await?,
        raw_conversation_events_until(&mut phone, Until::TurnEnd).await?,
    ]
    .concat();
    let turn = |session| {
        of(session, &ended)
            .into_iter()
            .filter(|kind| !matches!(kind, EventKind::AssistantDelta { .. }))
            .collect::<Vec<EventKind>>()
    };
    assert_eq!(
        turn(first_session),
        [
            Vec::from([
                user("create a.txt"),
                reply("I’ll create `a.txt` with the exact requested contents."),
            ]),
            codex_tool(
                "exec-f4c7c6d4-8255-4bcc-972d-9f106cda7991",
                "/usr/bin/zsh -lc \"printf %s 'hello from s2' > a.txt\"",
                Decision::Approve,
                ToolOutcome::Succeeded
            ),
            Vec::from([
                reply("Created [a.txt](/tmp/s2-scratch/a.txt)."),
                turn_end(TurnOutcome::Completed),
            ]),
        ]
        .concat()
    );
    assert_eq!(
        turn(second_session),
        [
            Vec::from([user("create b.txt")]),
            codex_tool(
                "exec-5f0ba5d1-395f-4d84-b098-a443a0873a5a",
                "/usr/bin/zsh -lc 'printf %s nope > b.txt'",
                Decision::Deny,
                ToolOutcome::Declined
            ),
            Vec::from([
                reply("The request was declined."),
                turn_end(TurnOutcome::Completed),
            ]),
        ]
        .concat()
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
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
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Codex, &dir)).await?;
    let _ = phone.send(message(session, "count to 300")).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Delta).await?,
        [
            opening(Provider::Codex, &dir),
            Vec::from([user("count to 300"), delta("1")]),
        ]
        .concat()
    );
    phone.accepted(interrupt(session)).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::TurnEnd).await?,
        [delta("\n"), turn_end(TurnOutcome::Interrupted)]
    );
    raw_ping_turn(&mut phone, session).await?;
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

/// Codex withdraws the open request only after the interrupted turn ends,
/// and the recording holds that withdrawal until `release` exists. The
/// session withdraws the request at the turn's end, so an answer in between
/// is refused and never reaches Codex.
#[tokio::test]
async fn a_codex_interrupt_withdraws_the_open_request_at_the_turn_end()
-> Fallible {
    let (fx, dir) = codex_host("codex-withdraw.jsonl").await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Codex, &dir)).await?;
    let _ = phone.send(message(session, "create d.txt")).await?;
    let call =
        ToolCallId(String::from("exec-c1e73e4d-1cac-431e-baf8-8bf5b779606b"));
    let tool = ToolKind::Command {
        command: String::from("/usr/bin/zsh -lc 'printf %s d > d.txt'"),
    };
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Request).await?,
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
    phone.accepted(interrupt(session)).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Resolved).await?,
        [
            turn_end(TurnOutcome::Interrupted),
            EventKind::RequestResolved {
                request: RequestId(1),
                resolution: Resolution::Withdrawn,
            },
        ]
    );
    assert_eq!(
        phone
            .send(resolve(session, 1, Decision::Approve))
            .await?
            .value,
        CommandResult::Rejected(Rejection::UnknownRequest)
    );
    std::fs::write(dir.join("release"), "")?;
    // Codex's withdrawal plays before the next turn and is ignored.
    raw_ping_turn(&mut phone, session).await?;
    assert_eq!(
        std::fs::read_to_string(dir.join("stdin.jsonl"))?
            .matches("\"decision\"")
            .count(),
        0
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

/// Both thread starts must have answered before the app-server exits.
#[tokio::test]
async fn the_app_server_exiting_closes_every_codex_session() -> Fallible {
    let (fx, dir) = codex_host("codex-exit.jsonl").await?;
    std::fs::copy(
        recording_file("codex-exit.jsonl"),
        dir.join("stdout.2.jsonl"),
    )?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let first_session = phone.started(start(Provider::Codex, &dir)).await?;
    let first = raw_conversation_events_until(&mut phone, Until::Ready).await?;
    let second_session = phone.started(start(Provider::Codex, &dir)).await?;
    assert_ne!(first_session, second_session);
    let seen = [
        first,
        raw_conversation_events_until(&mut phone, Until::Closed).await?,
        raw_conversation_events_until(&mut phone, Until::Closed).await?,
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
        phone.send(message(first_session, "hi")).await?.value,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    let next_session = phone.started(start(Provider::Codex, &dir)).await?;
    assert_ne!(next_session, first_session);
    assert_ne!(next_session, second_session);
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Ready).await?,
        opening(Provider::Codex, &dir)
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

#[tokio::test]
async fn a_wrong_token_is_refused_and_never_goes_live() -> Fallible {
    let fx = host().await?;
    let target = Target {
        token: AuthToken::from_bytes([0; 16]),
        ..fx.target.clone()
    };
    let mut feed = Recorder::new()?;
    let refused = match RawClient::open(&target, "test-client", &mut feed).await
    {
        Ok(client) => {
            client.close().await;
            None
        }
        Err(error) => Some(error.to_string()),
    };
    assert_eq!(refused.as_deref(), Some("wrong token"));
    feed.write()?;
    fx.shutdown().await
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
const UNPROTECTED: &str = "the CLI's commands can read the provider credential; install bubblewrap and socat to sandbox them provider=Claude";

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

async fn claude_host_in(data: &Path) -> Fallible<Fixture> {
    install_fake(data, Provider::Claude, &fake_claude())?;
    probe(data, &instance(Provider::Claude), PROBED)?;
    probe(data, &api(Provider::Claude), PROBED)?;
    serve(
        endpoint().await?,
        plugins(None, Some(PathBuf::from(REFUSING_CLI))),
        data,
    )
    .await
}

fn install_fake(data: &Path, cli: Provider, fake: &Path) -> Fallible {
    let pin = agents::pin::pin(
        cli,
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::consts::EXE_SUFFIX,
        cfg!(target_env = "musl"),
    )
    .ok_or("no CLI pin on this platform")?;
    let path = data
        .join("cli")
        .join(cli.name())
        .join(pin.version)
        .join(pin.binary);
    wrapper(&path, fake)
}

fn wrapper(path: &Path, program: &Path) -> Fallible {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().ok_or("missing script directory")?)?;
    let quoted = program
        .to_str()
        .ok_or("non-UTF-8 fake CLI path")?
        .replace('\'', "'\"'\"'");
    std::fs::write(path, format!("#!/bin/sh\nexec '{quoted}' \"$@\"\n"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
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

#[tokio::test]
async fn a_claude_gateway_reaches_the_cli_survives_a_restart_and_is_removed()
-> Fallible {
    let data = data_dir("claude-gateway")?;
    let (log, logging) = capture_log("claude-gateway")?;
    let dir = scratch("claude-gateway", "claude-approve.jsonl")?;
    let fx = claude_host_in(&data).await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let claude_api = api(Provider::Claude);
    let with_api =
        and(with_claude(no_credential()), &claude_api, gateway(probed()));
    assert_eq!(phone.instances, with_claude(no_credential()));
    assert_eq!(
        phone.send(start(Provider::Claude, &dir)).await?.value,
        refused_for_no_credential()
    );
    phone.accepted(claude_gateway()).await?;
    wait_for_offer(&mut phone, &claude_api).await?;
    assert_eq!(phone.instances, with_api);
    let probe_env = read(&dir_of(&data, "work", &claude_api).join("env.txt"))?;
    let home = dir_of(&data, "home", &claude_api);
    assert_gateway_env(&probe_env, CLAUDE_TOKEN, GATEWAY, &home);
    let expected_warnings = usize::from(!scrubbed(&probe_env));
    assert_eq!(
        count(&log, UNPROTECTED)?,
        expected_warnings,
        "after gateway configuration"
    );

    phone.started(start_on(claude_api.clone(), &dir)).await?;
    let events =
        raw_conversation_events_until(&mut phone, Until::Ready).await?;
    let env = read(&dir.join("env.txt"))?;
    assert_gateway_env(&env, CLAUDE_TOKEN, GATEWAY, &home);
    assert_eq!(scrubbed(&env), scrubbed(&probe_env));
    assert_eq!(count(&data.join("credentials"), CLAUDE_TOKEN)?, 0);
    phone.close().await;
    fx.shutdown().await?;

    let fx = claude_host_in(&data).await?;
    let phone = RawClient::live(&fx.target, "test-client", &mut feed).await?;
    assert_eq!(phone.instances, with_api);
    assert_eq!(
        count(&log, UNPROTECTED)?,
        expected_warnings * 2,
        "after restarting with the gateway"
    );
    phone
        .accepted(CommandBody::RemoveInstance {
            instance: claude_api.clone(),
        })
        .await?;
    assert_eq!(
        phone.send(start_on(claude_api, &dir)).await?.value,
        CommandResult::Rejected(Rejection::UnknownInstance)
    );
    phone.close().await;
    fx.shutdown().await?;

    let fx = claude_host_in(&data).await?;
    let phone = RawClient::live(&fx.target, "test-client", &mut feed).await?;
    assert_eq!(phone.instances, with_claude(no_credential()));
    phone.close().await;
    fx.shutdown().await?;
    assert_eq!(
        count(&log, UNPROTECTED)?,
        expected_warnings * 2,
        "after restarting without the gateway"
    );
    drop(logging);
    let logged = captured(&log)?;
    assert_eq!(
        (
            logged.matches("added an instance").count(),
            logged.matches(CLAUDE_TOKEN).count(),
            format!("{events:?}").matches(CLAUDE_TOKEN).count(),
        ),
        (1, 0, 0)
    );
    feed.write()
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
/// first, from the instance feed.
fn codex_offer(
    Instances(mut shown): Instances,
) -> Option<(Auth, Option<String>, Vec<String>)> {
    let Availability::Available { auth, models } =
        shown.remove(&api(Provider::Codex))?.availability
    else {
        return None;
    };
    let choices = models
        .choices
        .into_iter()
        .map(|choice| choice.id.0)
        .collect();
    Some((auth, models.default.map(|model| model.0), choices))
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
/// model as the default.
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
    let codex_api = api(Provider::Codex);
    seed(&data, std::slice::from_ref(&codex_api), &[Provider::Codex])?;
    install_fake(&data, Provider::Codex, &fake_codex())?;
    let server =
        codex_server(&data, &codex_api, "codex-two-threads-listed.jsonl")?;
    let fx = serve(
        endpoint().await?,
        plugins(Some(PathBuf::from(REFUSING_CLI)), None),
        &data,
    )
    .await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    assert_eq!(
        phone.instances,
        and(
            with_codex(Availability::Unavailable {
                reason: String::from(UNAVAILABLE),
            }),
            &codex_api,
            no_credential()
        )
    );
    assert_eq!(
        codex_offer(
            configured(&phone, codex_provider(FIRST_KEY), &fx.target).await?
        ),
        Some(codex_gateway())
    );
    phone.started(start_on(codex_api.clone(), &server)).await?;
    let first = raw_conversation_events_until(&mut phone, Until::Ready).await?;
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
        codex_offer(
            configured(&phone, codex_provider(SECOND_KEY), &fx.target).await?
        ),
        Some(codex_gateway())
    );
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Closed).await?,
        [EventKind::SessionClosed {
            reason: CloseReason::Stopped {
                reason: String::from("new Codex credential"),
            },
        }]
    );
    phone.started(start_on(codex_api, &server)).await?;
    let second =
        raw_conversation_events_until(&mut phone, Until::Ready).await?;
    assert_eq!(
        read(&server.join("env.txt"))?,
        format!(
            "CODEX_HOME={}\nCONDUCTOR_CODEX_KEY={SECOND_KEY}\n",
            home.display()
        )
    );
    assert_eq!(count(&home.join("config.toml"), FIRST_KEY)?, 0);
    phone.close().await;
    fx.shutdown().await?;
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
        (2, 0, 0, 0)
    );
    feed.write()
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
    let mut feed = Recorder::new()?;
    let phone = RawClient::live(&fx.target, "test-client", &mut feed).await?;
    assert_eq!(
        (phone.instances.clone(), data.join("credentials").exists()),
        (with_claude(no_credential()), false)
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

#[tokio::test]
async fn an_unreadable_store_keeps_the_host_up_refuses_a_set_and_moves_aside()
-> Fallible {
    let data = data_dir("unreadable")?;
    std::fs::write(data.join("store.key"), "00".repeat(32))?;
    std::fs::write(data.join("credentials"), b"not a sealed store")?;
    let fx = claude_host_in(&data).await?;
    let mut feed = Recorder::new()?;
    let phone = RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let unreadable = Availability::Unavailable {
        reason: format!(
            "{} does not open with the store key; clearing a credential moves it aside",
            data.join("credentials").display()
        ),
    };
    assert_eq!(
        phone.instances,
        with_claude(unreadable.clone()),
        "a stored credential may be in it, so the home's sign-in is not used"
    );
    let refused = CommandResult::Rejected(Rejection::CredentialNotSaved {
        reason: format!(
            "{} does not open with the store key; clearing a credential moves it aside",
            data.join("credentials").display()
        ),
    });
    assert_eq!(phone.send(claude_gateway()).await?.value, refused);
    assert_eq!(
        read(&data.join("credentials"))?,
        "not a sealed store",
        "a refused set leaves the file as it was"
    );
    assert_eq!(
        configured(
            &phone,
            CommandBody::RemoveInstance {
                instance: instance(Provider::Codex),
            },
            &fx.target,
        )
        .await?,
        with_claude(no_credential())
    );
    assert_eq!(
        read(&data.join("credentials.unreadable"))?,
        "not a sealed store"
    );
    assert_eq!(
        configured(&phone, claude_gateway(), &fx.target).await?,
        and(
            with_claude(no_credential()),
            &api(Provider::Claude),
            gateway(probed())
        )
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

#[tokio::test]
async fn a_base_url_that_could_hide_a_secret_is_refused() -> Fallible {
    let data = data_dir("base-url")?;
    let fx = claude_host_in(&data).await?;
    let mut feed = Recorder::new()?;
    let phone = RawClient::live(&fx.target, "test-client", &mut feed).await?;
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
        (
            phone.send(body).await?.value,
            data.join("credentials").exists()
        ),
        (
            CommandResult::Rejected(Rejection::InvalidCredential {
                reason: String::from(
                    "Gateway URL may not carry a user, a password, a query or a fragment"
                ),
            }),
            false
        )
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

fn probe(data: &Path, id: &InstanceId, response: &str) -> Fallible {
    let work = dir_of(data, "work", id);
    std::fs::create_dir_all(&work)?;
    std::fs::write(work.join("stdout.jsonl"), format!("{response}\n"))?;
    Ok(())
}

const CLAUDE_MODELS: &str = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_1","response":{"claude_code_version":"2.1.316","models":[{"value":"default","resolvedModel":"big","displayName":"Default","description":""},{"value":"big","resolvedModel":"big","displayName":"Big","description":"thinks","supportedEffortLevels":["low","high"]},{"value":"small","displayName":"Small","description":"quick","supportedEffortLevels":["low"]}]}}}"#;

fn standard_claude_probe(data: &Path) -> Fallible {
    probe(data, &instance(Provider::Claude), CLAUDE_MODELS)
}

async fn plugin_host(data: &Path) -> Fallible<Fixture> {
    serve(endpoint().await?, claude_plugins(), data).await
}

fn status_of(listed: &Instances, id: &InstanceId) -> Option<InstanceStatus> {
    listed.of(id).cloned()
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

fn scrubbed(env: &str) -> bool {
    env.lines()
        .any(|line| line == "CLAUDE_CODE_SUBPROCESS_ENV_SCRUB=1")
}

fn assert_gateway_env(env: &str, token: &str, base_url: &str, home: &Path) {
    let fields = env
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(fields.get("ANTHROPIC_AUTH_TOKEN"), Some(&token));
    assert_eq!(fields.get("ANTHROPIC_BASE_URL"), Some(&base_url));
    assert_eq!(fields.get("ANTHROPIC_API_KEY"), None);
    assert_eq!(fields.get("CLAUDE_CODE_OAUTH_TOKEN"), None);
    assert_eq!(fields.get("CLAUDE_CONFIG_DIR").map(Path::new), Some(home));
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
    probe(&data, &claude_api, PROBED)?;
    probe(&data, &other, PROBED)?;
    let fx = plugin_host(&data).await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let set = |base_url: &str, token: &str| CommandBody::SetCredential {
        provider: Provider::Claude,
        credential: credential(
            "gateway",
            &[("base_url", base_url), ("token", token)],
        ),
    };
    let after_api =
        configured(&phone, set(GATEWAY, CLAUDE_TOKEN), &fx.target).await?;
    let after_other =
        configured(&phone, set(OTHER, OTHER_TOKEN), &fx.target).await?;
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
        (Some(gateway(probed())), None, Some(other_gateway.clone()))
    );
    probe(&data, &claude_api, CLAUDE_MODELS)?;
    let changed =
        configured(&phone, set(GATEWAY, API_TOKEN), &fx.target).await?;
    assert_eq!(
        (
            status_of(&changed, &claude_api).map(|it| it.availability),
            status_of(&changed, &other).map(|it| it.availability),
        ),
        (Some(gateway(models())), Some(other_gateway))
    );
    let first = phone.started(start_on(claude_api.clone(), &on_api)).await?;
    let api_events = raw_events_until(&mut phone, Until::Ready).await?;
    let second = phone.started(start_on(other.clone(), &on_other)).await?;
    let other_events = raw_events_until(&mut phone, Until::Ready).await?;
    assert_eq!(
        (created_on(&api_events), created_on(&other_events)),
        (
            Vec::from([(first, claude_api.clone())]),
            Vec::from([(second, other.clone())]),
        )
    );
    assert_gateway_env(
        &read(&on_api.join("env.txt"))?,
        API_TOKEN,
        GATEWAY,
        &dir_of(&data, "home", &claude_api),
    );
    assert_gateway_env(
        &read(&on_other.join("env.txt"))?,
        OTHER_TOKEN,
        OTHER,
        &dir_of(&data, "home", &other),
    );
    assert_eq!(
        (
            phone
                .send(CommandBody::RemoveInstance {
                    instance: claude_api.clone()
                })
                .await?
                .value,
            dir_of(&data, "home", &claude_api)
                .join("settings.json")
                .exists(),
        ),
        (
            CommandResult::Rejected(Rejection::InstanceInUse { sessions: 1 }),
            true,
        )
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
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
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    assert_eq!(
        [
            phone
                .send(start_with(Provider::Claude, &dir, "huge", Effort::Low))
                .await?
                .value,
            phone
                .send(start_with(Provider::Claude, &dir, "small", Effort::High))
                .await?
                .value,
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
    let _ = phone
        .started(start_with(Provider::Claude, &dir, "small", Effort::Low))
        .await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Ready).await?,
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
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
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
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &dir)).await?;
    let _ = phone.send(message(session, "sheep?")).await?;
    let live = raw_kinds_until(&mut phone, Until::TurnEnd).await?;
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
    let caught_up = raw_history(&fx.target, &mut feed).await?;
    assert_eq!(
        caught_up
            .into_iter()
            .map(|event| event.kind)
            .filter(conversation_event)
            .collect::<Vec<EventKind>>(),
        stored
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

/// `probe-subagent-todo.jsonl`: the main agent's `Agent` call runs a
/// sub-agent, which calls `TodoWrite` twice and reports "ok". Its events are
/// the session's, and its checklist is not the session's checklist.
#[tokio::test]
async fn a_claude_subagent_runs_in_its_session_under_the_call_that_started_it()
-> Fallible {
    let fx = host().await?;
    let dir = scratch("subagent", "claude-subagent.jsonl")?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &dir)).await?;
    let _ = phone.send(message(session, "delegate")).await?;
    let events =
        raw_conversation_events_until(&mut phone, Until::TurnEnd).await?;
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
            .filter(|kind| !matches!(
                kind,
                EventKind::AssistantDelta { .. }
                    | EventKind::Subagent {
                        event: SubagentEvent::ChecklistChanged { .. },
                        ..
                    }
            ))
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
                EventKind::Subagent {
                    agent: agent(),
                    event: SubagentEvent::Kind {
                        kind: String::from("general-purpose"),
                    },
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
    assert_child_checklists(&events, &agent());
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

fn assert_child_checklists(events: &[Event], child: &SubagentId) {
    let checklists: Vec<&[ChecklistEntry]> = events
        .iter()
        .filter_map(|entry| match &entry.kind {
            EventKind::Subagent {
                agent: id,
                event: SubagentEvent::ChecklistChanged { checklist },
            } if id == child => Some(checklist.as_slice()),
            _ => None,
        })
        .collect();
    assert_eq!(
        checklists
            .iter()
            .map(|checklist| steps(checklist))
            .collect::<Vec<_>>(),
        [
            vec![
                ("alpha", ChecklistStatus::Pending),
                ("beta", ChecklistStatus::Pending)
            ],
            vec![
                ("alpha", ChecklistStatus::Completed),
                ("beta", ChecklistStatus::Completed)
            ],
        ],
    );
}

/// Synthetic against the pinned 0.166 app-server schema.
#[tokio::test]
async fn codex_children_route_activity_and_approvals_to_the_session_that_spawned_them()
-> Fallible {
    let recording = "codex-subagents.jsonl";
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(recording);
    let (fx, dir) =
        codex_host_from(recording, instance(Provider::Codex), &source).await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Codex, &dir)).await?;
    let _ = phone.send(message(session, "delegate")).await?;
    let events =
        raw_conversation_events_until(&mut phone, Until::Request).await?;
    let request = events
        .last()
        .and_then(|event| match &event.kind {
            EventKind::Subagent {
                agent,
                event: SubagentEvent::RequestOpened { request, .. },
            } if agent.0 == "child-alpha" => Some(*request),
            _ => None,
        })
        .ok_or("expected the child approval")?;
    phone
        .accepted(resolve(session, request.0, Decision::Approve))
        .await?;
    let _ = raw_conversation_events_until(&mut phone, Until::TurnEnd).await?;
    let history = of(session, &raw_history(&fx.target, &mut feed).await?);
    let alpha = agent_events(&history, "child-alpha");
    let beta = agent_events(&history, "child-beta");
    assert!(alpha.iter().any(|event| matches!(
        event,
        SubagentEvent::Started { description, .. } if description == "Inspect alpha"
    )));
    assert!(alpha.contains(&SubagentEvent::Kind {
        kind: String::from("explorer")
    }));
    assert_eq!(
        alpha.iter().rev().find_map(|event| match event {
            SubagentEvent::ChecklistChanged { checklist } => {
                Some(steps(checklist))
            }
            _ => None,
        }),
        Some(Vec::from([("Read alpha", ChecklistStatus::Completed)]))
    );
    assert!(alpha.iter().any(|event| matches!(
        event,
        SubagentEvent::Ended {
            outcome: SubagentOutcome::Finished,
            ..
        }
    )));
    assert!(beta.iter().any(|event| matches!(
        event,
        SubagentEvent::Ended {
            outcome: SubagentOutcome::Failed,
            ..
        }
    )));
    assert!(reports(&alpha).contains(&"Alpha checked."));
    assert!(reports(&beta).contains(&"Beta failed."));
    assert_approved_and_ran(&history, &alpha, request)?;
    assert!(history.contains(&reply("Delegation done.")));
    assert!(!history.contains(&reply("Alpha checked.")));
    assert!(
        !history
            .iter()
            .any(|kind| matches!(kind, EventKind::ChecklistChanged { .. }))
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

/// The child's call that asked `request` was approved and then succeeded.
fn assert_approved_and_ran(
    history: &[EventKind],
    alpha: &[SubagentEvent],
    request: RequestId,
) -> Fallible {
    let call = alpha
        .iter()
        .find_map(|event| match event {
            SubagentEvent::RequestOpened {
                request: asked,
                call,
                ..
            } if *asked == request => Some(call.clone()),
            _ => None,
        })
        .ok_or("the child asked no approval")?;
    assert!(history.iter().any(|kind| matches!(
        kind,
        EventKind::RequestResolved {
            request: resolved,
            resolution: Resolution::Decided {
                decision: Decision::Approve,
                ..
            },
        } if *resolved == request
    )));
    assert!(alpha.contains(&SubagentEvent::Activity(
        SubagentActivity::ToolCall {
            call,
            phase: ToolPhase::Finished {
                outcome: ToolOutcome::Succeeded,
            },
        }
    )));
    Ok(())
}

/// What the sub-agent `agent` did, among a session's events.
fn agent_events(kinds: &[EventKind], agent: &str) -> Vec<SubagentEvent> {
    kinds
        .iter()
        .filter_map(|kind| match kind {
            EventKind::Subagent { agent: id, event } if id.0 == agent => {
                Some(event.clone())
            }
            _ => None,
        })
        .collect()
}

/// A sub-agent's messages and final report.
fn reports(events: &[SubagentEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            SubagentEvent::Activity(SubagentActivity::Message { text })
            | SubagentEvent::Ended {
                summary: Some(text),
                ..
            } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// Through the owner's gateway, with betas off, the CLI streams an empty
/// thinking block (`claude-killed.jsonl`'s first turn), which shows nothing.
#[tokio::test]
async fn empty_gateway_thinking_logs_no_reasoning_and_folds_to_no_disclosure()
-> Fallible {
    let fx = host().await?;
    let dir = scratch("betas-off", "claude-killed.jsonl")?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &dir)).await?;
    let _ = phone.send(message(session, "sheep?")).await?;
    let turn = raw_kinds_until(&mut phone, Until::TurnEnd).await?;
    assert_eq!(
        turn.into_iter()
            .filter(|kind| matches!(
                kind,
                EventKind::Reasoning { .. }
                    | EventKind::ReasoningDelta { .. }
                    | EventKind::AssistantMessage { .. }
            ))
            .collect::<Vec<EventKind>>(),
        [reply("18")]
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

/// A Claude CLI killed mid-turn closes the session; `Continue` starts it
/// again with `--resume` on the same conversation, model and effort, and
/// the next turn runs.
#[tokio::test]
async fn a_killed_claude_session_continues_its_conversation() -> Fallible {
    let fx = host().await?;
    let dir = scratch("killed", "claude-killed.jsonl")?;
    std::fs::copy(
        recording_file("claude-resumed.jsonl"),
        dir.join("stdout.2.jsonl"),
    )?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    assert_eq!(
        phone.send(continued(SessionId(1))).await?.value,
        CommandResult::Rejected(Rejection::UnknownSession)
    );
    let session = phone
        .started(start_with(Provider::Claude, &dir, "small", Effort::Low))
        .await?;
    let _ = phone.send(message(session, "sheep?")).await?;
    let _ = raw_conversation_events_until(&mut phone, Until::TurnEnd).await?;
    phone.accepted(continued(session)).await?;
    let _ = phone.send(message(session, "count")).await?;
    let killed =
        raw_conversation_events_until(&mut phone, Until::Closed).await?;
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
        phone.send(message(session, "hello?")).await?.value,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    raw_resume_to_ready(&mut phone, session).await?;
    let _ = phone.send(message(session, "how many sheep?")).await?;
    let after =
        raw_conversation_events_until(&mut phone, Until::TurnEnd).await?;
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
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

/// A `Continue` whose transcript is gone leaves the session closed, with
/// the CLI's own reason (the recorded `result` of the pinned CLI).
#[tokio::test]
async fn a_continue_with_no_transcript_stays_closed_with_the_clis_reason()
-> Fallible {
    let fx = host().await?;
    let dir = scratch("resume-missing", "claude-killed.jsonl")?;
    std::fs::copy(
        recording_file("claude-resume-missing.jsonl"),
        dir.join("stdout.2.jsonl"),
    )?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &dir)).await?;
    let _ = phone.send(message(session, "sheep?")).await?;
    let _ = raw_conversation_events_until(&mut phone, Until::TurnEnd).await?;
    let _ = phone.send(message(session, "count")).await?;
    let _ = raw_conversation_events_until(&mut phone, Until::Closed).await?;
    phone.accepted(continued(session)).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Closed).await?,
        [EventKind::SessionClosed {
            reason: CloseReason::FailedToStart {
                message: String::from(
                    "claude exited: No conversation found with session ID: 5b0b7a52-1c9e-4d1e-9a4e-3f2f8c1d6a10"
                ),
            },
        }]
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

const COUNTING: &str = "**Compiling numbers**\n\nI'm tasked with creating a list of 300 numbers, one per line. It sounds massive but straightforward—I'll ensure each number is on its own line. I need to focus on accuracy while keeping the format neat. There are around 600 tokens to work with overall. Luckily, it seems I won’t need any tools for this task; I just need to produce the exact output as required. Let's get started on this!";

/// The app-server killed mid-turn, as live: after `turn/started`, with
/// deltas streamed. The session closes with no failed turn, and `Continue`
/// resumes the thread.
#[tokio::test]
async fn a_codex_server_killed_mid_turn_closes_and_continues() -> Fallible {
    let (fx, server) = codex_host("codex-killed-mid-turn.jsonl").await?;
    std::fs::copy(
        recording_file("codex-resumed.jsonl"),
        server.join("stdout.2.jsonl"),
    )?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone
        .started(start_with(Provider::Codex, &server, "small", Effort::Low))
        .await?;
    let _ = phone.send(message(session, "prime?")).await?;
    let _ = raw_kinds_until(&mut phone, Until::TurnEnd).await?;
    let _ = phone.send(message(session, COUNT)).await?;
    let killed = raw_kinds_until(&mut phone, Until::Closed).await?;
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
    raw_resume_to_ready(&mut phone, session).await?;
    let _ = phone.send(message(session, "codeword?")).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::TurnEnd)
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
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

/// A new credential that doesn't offer a Codex session's model: `Continue`
/// starts it on the new default and logs why. (It closed before its first
/// turn, so it starts a thread afresh; a resume takes the same choice.)
#[tokio::test]
async fn a_codex_session_continues_on_the_default_when_a_new_credential_drops_its_model()
-> Fallible {
    let (fx, server) =
        codex_host_on("codex-resumed.jsonl", api(Provider::Codex)).await?;
    std::fs::copy(
        recording_file("codex-resumed-listed.jsonl"),
        server.join("stdout.2.jsonl"),
    )?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone
        .started(CommandBody::StartSession {
            instance: api(Provider::Codex),
            at: StartAt::Directory(HostPath(server.display().to_string())),
            model: Some(ModelId(String::from("small"))),
            effort: Some(Effort::Low),
        })
        .await?;
    let _ = raw_kinds_until(&mut phone, Until::Ready).await?;
    assert_eq!(
        codex_offer(
            configured(
                &phone,
                codex_provider("conductor-test-key"),
                &fx.target
            )
            .await?
        ),
        Some(codex_gateway())
    );
    let _ = raw_kinds_until(&mut phone, Until::Closed).await?;
    phone.accepted(continued(session)).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Ready).await?,
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
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
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
        recording_file("codex-resumed.jsonl"),
        server.join("stdout.2.jsonl"),
    )?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone
        .started(start_with(Provider::Codex, &server, "small", Effort::Low))
        .await?;
    let _ = phone.send(message(session, "prime?")).await?;
    let first = raw_kinds_until(&mut phone, Until::TurnEnd).await?;
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
    let _ = phone.send(message(session, COUNT)).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::Closed).await?,
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
    raw_resume_to_ready(&mut phone, session).await?;
    let _ = phone.send(message(session, "codeword?")).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::TurnEnd)
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
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
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
        recording_file("claude-resumed.jsonl"),
        dir.join("stdout.2.jsonl"),
    )?;
    let home = data.join("home-repo");
    std::fs::create_dir(&home)?;
    std::fs::write(home.join("AGENTS.md"), "Instructions before restart.")?;
    let first =
        durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let address = first.address()?;
    let ticket = first.target.clone();
    let mut feed = Recorder::new()?;
    let mut phone = RawClient::live(&ticket, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &dir)).await?;
    phone.accepted(message(session, "create a.txt")).await?;
    let before =
        raw_conversation_events_until(&mut phone, Until::Request).await?;
    let cursor = before.last().ok_or("expected a value")?.seq;
    let epoch = phone.welcome.epoch;
    let conversation = argv(&dir)?
        .first()
        .and_then(|run| flag(run, "--session-id"))
        .ok_or("expected a value")?;
    phone.close().await;
    first.shutdown().await?;

    std::fs::write(home.join("AGENTS.md"), "Instructions after restart.")?;
    let second = durable_host(&data, address).await?;
    assert_eq!(second.target.addr.id, ticket.addr.id);
    assert_eq!(second.target.token, ticket.token);
    let mut phone = RawClient::open(&ticket, "test-client", &mut feed).await?;
    assert_eq!(phone.welcome.epoch, epoch, "nothing is reset");
    phone.subscribe(cursor).await?;
    assert_restart_cleanup(
        &raw_events(phone.caught_up().await?),
        cursor,
        session,
    )?;
    assert_eq!(
        phone
            .send(resolve(session, 1, Decision::Approve))
            .await?
            .value,
        CommandResult::Rejected(Rejection::SessionClosed)
    );
    raw_resume_to_ready(&mut phone, session).await?;
    assert_eq!(
        phone
            .send(resolve(session, 1, Decision::Approve))
            .await?
            .value,
        CommandResult::Rejected(Rejection::UnknownRequest)
    );
    phone.accepted(message(session, "how many sheep?")).await?;
    assert_eq!(
        raw_kinds_until(&mut phone, Until::TurnEnd)
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
    let runs = argv(&dir)?;
    let resumed = runs.last().ok_or("no resumed CLI invocation")?;
    assert!(resumed.contains(&format!("--resume={conversation}")));
    assert!(
        resumed
            .iter()
            .any(|line| line.starts_with("--append-system-prompt="))
    );
    assert!(
        resumed
            .iter()
            .any(|line| line == "Instructions after restart.")
    );
    phone.close().await;
    feed.write()?;
    second.shutdown().await
}

/// After a restart, the replay from `cursor` holds only the cleanup: the
/// open request withdrawn, the turn interrupted with a snapshot, and the
/// session closed. Nothing is reset.
fn assert_restart_cleanup(
    cleanup: &[Event],
    cursor: HostSeq,
    session: SessionId,
) -> Fallible {
    let end = cursor.0.checked_add(4).ok_or("restart cursor overflow")?;
    assert_eq!(
        seqs(cleanup),
        (cursor.next().0..=end).map(HostSeq).collect::<Vec<_>>()
    );
    assert!(snapshot(cleanup)?.1.turn_start.is_some());
    assert_eq!(
        of(session, cleanup),
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
    hello(endpoint, target).await.map(|(conn, _)| conn)
}

async fn hello(
    endpoint: &Endpoint,
    target: &Target,
) -> Fallible<(iroh::endpoint::Connection, proto::Welcome)> {
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
    let reply = net::wire::read::<proto::HelloReply>(&mut recv)
        .await?
        .ok_or("the host closed the stream before replying")?;
    match reply {
        proto::HelloReply::Welcome(welcome) => Ok((conn, welcome)),
        proto::HelloReply::Refused { reason } => Err(reason.into()),
    }
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

fn seed(
    data: &Path,
    accounts: &[InstanceId],
    refused: &[Provider],
) -> Fallible<Roster> {
    let mut roster = Roster(
        Provider::ALL
            .into_iter()
            .map(|provider| (instance(provider), proto::Instance::default()))
            .collect(),
    );
    roster.0.extend(
        accounts
            .iter()
            .cloned()
            .map(|id| (id, proto::Instance::default())),
    );
    for &provider in refused {
        let own = roster
            .0
            .get_mut(&instance(provider))
            .ok_or("own instance missing")?;
        own.cli = Some(HostPath(REFUSING_CLI.into()));
    }
    disk::seed_roster(data, &roster)?;
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

async fn saved(
    data: &Path,
    mut expected: Roster,
    body: CommandBody,
) -> Fallible {
    let CommandBody::SetCredential {
        provider,
        credential,
    } = body
    else {
        return Err(format!("expected a credential, not {body:?}").into());
    };
    let changed = expected
        .0
        .get_mut(&api(provider))
        .ok_or("named instance missing")?;
    changed.credential = Some(credential);
    tokio::time::timeout(STEP, async {
        while disk::roster(data)? != expected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    })
    .await??;
    Ok(())
}

async fn marker(path: &Path) -> Fallible {
    tokio::time::timeout(STEP, async {
        loop {
            match tokio::fs::metadata(path).await {
                Ok(_) => break Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => break Err(error),
            }
        }
    })
    .await
    .map_err(|_| "the marker did not appear in time")??;
    Ok(())
}

/// After boot, the first refresh waits for `go` and fails.
fn held_claude_probe(data: &Path) -> Fallible<PathBuf> {
    let id = api(Provider::Claude);
    probe(data, &id, PROBED)?;
    let probe = dir_of(data, "work", &id);
    std::fs::write(
        probe.join("stdout.2.jsonl"),
        "{\"fake\":\"files\",\"write\":{\"held\":\"ready\"}}\n{\"fake\":\"release\",\"file\":\"go\"}\n{\"fake\":\"fail\",\"stderr\":\"superseded probe failed\",\"status\":1}\n",
    )?;
    Ok(probe)
}

/// While Claude refreshes, starting or continuing a Claude session is
/// refused at once, and other commands still answer.
async fn assert_refreshing_claude_refuses(
    phone: &RawClient,
    source: &Path,
    session: SessionId,
) -> Fallible {
    let refused = CommandResult::Rejected(Rejection::InstanceUnavailable {
        reason: String::from(REFRESHING),
    });
    assert_eq!(
        phone
            .send(start_on(api(Provider::Claude), source))
            .await?
            .value,
        refused
    );
    assert_eq!(phone.send(continued(session)).await?.value, refused);
    assert_eq!(
        phone.send(message(SessionId(999), "lookup")).await?.value,
        CommandResult::Rejected(Rejection::UnknownSession)
    );
    Ok(())
}

async fn start_ready(
    client: &mut RawClient,
    instance: InstanceId,
    source: &Path,
) -> Fallible<SessionId> {
    let session = client.started(start_on(instance, source)).await?;
    let ready = raw_events_until(client, Until::Ready).await?;
    assert_eq!(ready.last().map(|event| event.session), Some(session));
    Ok(session)
}

/// A credential whose probe fails leaves the account's instance
/// unavailable with the probe's reason; the next lists its models again.
async fn assert_failed_refresh_then_another(
    phone: &RawClient,
    probe: &Path,
    target: &Target,
) -> Fallible {
    std::fs::write(
        probe.join("stdout.4.jsonl"),
        "{\"fake\":\"fail\",\"stderr\":\"current probe failed\",\"status\":1}\n",
    )?;
    let failed = configured(
        phone,
        CommandBody::SetCredential {
            provider: Provider::Claude,
            credential: claude_credential("conductor-test-failing"),
        },
        target,
    )
    .await?;
    let Some(Availability::Unavailable { reason }) =
        status_of(&failed, &api(Provider::Claude))
            .map(|status| status.availability)
    else {
        return Err(
            "expected the failed refresh to make the instance unavailable"
                .into(),
        );
    };
    assert_ne!(reason, REFRESHING);
    assert_ne!(reason, "");
    assert_eq!(
        configured(
            phone,
            CommandBody::SetCredential {
                provider: Provider::Claude,
                credential: claude_credential("conductor-test-again"),
            },
            target,
        )
        .await?,
        with_api_claude(gateway(probed()))
    );
    Ok(())
}

fn with_api_claude(availability: Availability) -> Instances {
    and(
        with_claude(Availability::Available {
            auth: Auth::Unmanaged,
            models: models(),
        }),
        &api(Provider::Claude),
        availability,
    )
}

async fn wait_for_offer(client: &mut RawClient, id: &InstanceId) -> Fallible {
    while !matches!(
        client.instances.availability(id),
        Some(Availability::Available { .. })
    ) {
        client.next().await?;
    }
    Ok(())
}

async fn subscribed_offers(
    conn: &iroh::endpoint::Connection,
    welcome: &proto::Welcome,
) -> Fallible<Instances> {
    let (mut send, mut recv) = conn.open_bi().await?;
    net::wire::write(
        &mut send,
        &proto::Open::Subscribe(proto::Subscribe {
            epoch: welcome.epoch,
            after_seq: welcome.head_seq,
        }),
    )
    .await?;
    send.finish()?;
    loop {
        let frame =
            tokio::time::timeout(STEP, net::wire::read::<Feed>(&mut recv))
                .await??;
        if let Feed::Instances(offers) =
            frame.ok_or("subscription closed before the roster")?
        {
            return Ok(offers);
        }
    }
}

#[tokio::test]
async fn a_held_boot_probe_allows_connections_and_sessions_on_a_ready_account()
-> Fallible {
    let data = data_dir("held-boot-probe")?;
    let held = api(Provider::Claude);
    let healthy = instance(Provider::Claude);
    seed(&data, std::slice::from_ref(&held), &[])?;
    probe(&data, &held, PROBED)?;
    let first =
        durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let source = scratch("held-boot-probe", "claude-approve.jsonl")?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&first.target, "test-client", &mut feed).await?;
    let saved = start_ready(&mut phone, healthy.clone(), &source).await?;
    phone.close().await;
    let address = first.address()?;
    let ticket = first.target.clone();
    first.shutdown().await?;
    let work = dir_of(&data, "work", &held);
    probe(
        &data,
        &held,
        &format!("{}\n{PROBED}", r#"{"fake":"release","file":"go"}"#),
    )?;
    let rebound = durable_endpoint(&data, address).await?;
    // The host takes connections once started, before its probes finish.
    let second = tokio::time::timeout(
        STEP,
        daemon::start(
            rebound,
            claude_plugins(),
            HostOptions {
                name: "test-host".into(),
                data_dir: data.clone(),
                home: None,
                projects: Vec::new(),
                user: None,
            },
        ),
    )
    .await??;
    let mut phone = RawClient::live(&ticket, "test-client", &mut feed).await?;
    wait_for_offer(&mut phone, &healthy).await?;
    let starting = Availability::Unavailable {
        reason: String::from("starting the instance's CLI"),
    };
    assert_eq!(phone.instances.availability(&held), Some(&starting));
    phone.accepted(continued(saved)).await?;
    raw_events_until(&mut phone, Until::Ready).await?;
    let other = scratch("held-boot-other", "claude-approve.jsonl")?;
    start_ready(&mut phone, healthy, &other).await?;
    let desktop = endpoint().await?;
    let (conn, welcome) = hello(&desktop, &ticket).await?;
    std::fs::write(work.join("go"), "")?;
    wait_for_offer(&mut phone, &held).await?;
    phone.accepted(claude_gateway()).await?;
    assert_eq!(
        subscribed_offers(&conn, &welcome).await?,
        with_api_claude(gateway(probed()))
    );
    desktop.close().await;
    phone.close().await;
    tokio::time::timeout(STEP, second.initialized()).await??;
    feed.write()?;
    Ok(second.shutdown().await?)
}

#[tokio::test]
async fn retrying_the_stored_credential_during_boot_receives_the_live_refresh_result()
-> Fallible {
    let data = data_dir("retry-at-boot")?;
    let id = api(Provider::Claude);
    let current = claude_credential("conductor-test-original");
    let mut roster = seed(&data, std::slice::from_ref(&id), &[])?;
    roster
        .0
        .get_mut(&id)
        .ok_or("missing seeded instance")?
        .credential = Some(current.clone());
    disk::seed_roster(&data, &roster)?;
    standard_claude_probe(&data)?;
    let failed = r#"{"fake":"fail","stderr":"discovery failed","status":1}"#;
    probe(
        &data,
        &id,
        &format!("{}\n{failed}", r#"{"fake":"release","file":"go"}"#),
    )?;
    let work = dir_of(&data, "work", &id);
    std::fs::write(work.join("stdout.2.jsonl"), failed)?;
    let host = daemon::start(
        endpoint().await?,
        claude_plugins(),
        HostOptions {
            name: String::from("retry-at-boot"),
            data_dir: data,
            home: None,
            projects: Vec::new(),
            user: None,
        },
    )
    .await?;
    let target = host.target();
    let mut feed = Recorder::new()?;
    let mut phone = RawClient::live(&target, "test-client", &mut feed).await?;
    let desktop = endpoint().await?;
    let conn = authenticated(&desktop, &target).await?;
    let body = CommandBody::SetCredential {
        provider: Provider::Claude,
        credential: current,
    };
    let retry = queued_command(&conn, 2363, body.clone());
    let refreshing = Availability::Unavailable {
        reason: String::from(REFRESHING),
    };
    while phone.instances.availability(&id) != Some(&refreshing) {
        phone.next().await?;
    }
    std::fs::write(work.join("go"), "")?;
    let receipt = finished_command(retry).await?;
    assert_eq!(receipt, CommandResult::Accepted);
    assert!(
        matches!(status_of(&current_offers(&target).await?, &id).map(|status| status.availability), Some(Availability::Unavailable { reason }) if reason != REFRESHING && !reason.is_empty())
    );
    std::fs::write(work.join("stdout.3.jsonl"), PROBED)?;
    assert_eq!(
        configured(&phone, body.clone(), &target).await?,
        with_api_claude(gateway(probed()))
    );
    assert_eq!(
        finished_command(queued_command(&conn, 2363, body)).await?,
        receipt
    );
    desktop.close().await;
    phone.close().await;
    host.shutdown().await?;
    feed.write()
}

#[tokio::test]
async fn unsupported_stored_credentials_are_refused_by_both_instances_and_can_be_replaced()
-> Fallible {
    let data = data_dir("unsupported-stored-credentials")?;
    let accounts = Provider::ALL.map(api);
    let roster = Roster(
        accounts
            .iter()
            .map(|account| {
                (
                    account.clone(),
                    proto::Instance {
                        credential: Some(credential(
                            "obsolete",
                            &[("key", "private-value")],
                        )),
                        ..proto::Instance::default()
                    },
                )
            })
            .collect(),
    );
    disk::seed_roster(&data, &roster)?;
    let claude = api(Provider::Claude);
    probe(&data, &claude, PROBED)?;
    codex_server(
        &data,
        &api(Provider::Codex),
        "codex-two-threads-listed.jsonl",
    )?;
    let fx = serve(
        endpoint().await?,
        plugins(Some(fake_claude()), Some(fake_codex())),
        &data,
    )
    .await?;
    let mut feed = Recorder::new()?;
    let phone = RawClient::live(&fx.target, "test-client", &mut feed).await?;
    for account in accounts {
        let Some(Availability::Unavailable { reason }) =
            phone.instances.availability(&account)
        else {
            return Err(format!(
                "the stored credential for {account} was accepted"
            )
            .into());
        };
        assert!(reason.contains("refused the stored credential"));
        assert!(!reason.contains("private-value"));
    }
    assert_eq!(
        status_of(
            &configured(&phone, claude_gateway(), &fx.target).await?,
            &claude
        )
        .map(|status| status.availability),
        Some(gateway(probed()))
    );
    assert_eq!(
        codex_offer(
            configured(&phone, codex_provider("replacement-key"), &fx.target)
                .await?
        ),
        Some(codex_gateway())
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

#[tokio::test]
async fn a_new_account_with_failed_discovery_stays_unavailable_until_a_successful_change()
-> Fallible {
    let data = data_dir("new-account-discovery")?;
    standard_claude_probe(&data)?;
    let account = api(Provider::Claude);
    probe(
        &data,
        &account,
        r#"{"fake":"fail","stderr":"gateway offline","status":1}"#,
    )?;
    let fx = plugin_host(&data).await?;
    let mut feed = Recorder::new()?;
    let phone = RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let failed = configured(&phone, claude_gateway(), &fx.target).await?;
    let Some(Availability::Unavailable { reason }) =
        status_of(&failed, &account).map(|status| status.availability)
    else {
        return Err("the newly added account advertised an offer after discovery failed".into());
    };
    assert_ne!(reason, REFRESHING);
    probe(&data, &account, PROBED)?;
    assert_eq!(
        configured(&phone, claude_gateway(), &fx.target).await?,
        with_api_claude(gateway(probed()))
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

#[tokio::test]
async fn provider_refresh_keeps_wire_commands_live_and_coalesces_stale_claude_results()
-> Fallible {
    let data = data_dir("refresh-claude")?;
    let source = scratch("refresh-claude", "claude-approve.jsonl")?;
    let probe = held_claude_probe(&data)?;
    let claude_api = api(Provider::Claude);
    let seeded = seed(&data, std::slice::from_ref(&claude_api), &[])?;
    standard_claude_probe(&data)?;
    let fx = plugin_host(&data).await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = start_ready(&mut phone, claude_api, &source).await?;
    let desktop = endpoint().await?;
    let conn = authenticated(&desktop, &fx.target).await?;
    let first_body = CommandBody::SetCredential {
        provider: Provider::Claude,
        credential: claude_credential("conductor-test-old"),
    };
    let first = queued_command(&conn, 2301, first_body.clone());
    marker(&probe.join("held")).await?;
    let mut watcher =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    assert_eq!(
        watcher.instances,
        with_api_claude(Availability::Unavailable {
            reason: String::from(REFRESHING)
        })
    );
    let duplicate = queued_command(&conn, 2301, first_body.clone());
    assert_refreshing_claude_refuses(&phone, &source, session).await?;
    start_ready(&mut watcher, instance(Provider::Claude), &source).await?;
    watcher.close().await;
    let middle_body = CommandBody::SetCredential {
        provider: Provider::Claude,
        credential: claude_credential("conductor-test-middle"),
    };
    let middle = queued_command(&conn, 2302, middle_body.clone());
    saved(&data, seeded.clone(), middle_body).await?;
    let newest_body = CommandBody::SetCredential {
        provider: Provider::Claude,
        credential: claude_credential("conductor-test-newest"),
    };
    let newest = queued_command(&conn, 2303, newest_body.clone());
    saved(&data, seeded, newest_body).await?;
    assert_eq!(read(&probe.join("runs.txt"))?, "2");
    std::fs::write(probe.join("go"), "")?;
    let result = CommandResult::Accepted;
    assert_eq!(finished_command(first).await?, result);
    assert_eq!(finished_command(duplicate).await?, result);
    assert_eq!(finished_command(middle).await?, result);
    assert_eq!(finished_command(newest).await?, result);
    assert_eq!(
        current_offers(&fx.target).await?,
        with_api_claude(gateway(probed()))
    );
    assert_eq!(read(&probe.join("runs.txt"))?, "3");
    assert_ne!(
        read(&probe.join("env.txt"))?.find("conductor-test-newest"),
        None
    );
    assert_failed_refresh_then_another(&phone, &probe, &fx.target).await?;
    phone.close().await;
    desktop.close().await;
    fx.shutdown().await?;
    let reopened = plugin_host(&data).await?;
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
    assert_eq!(read(&probe.join("runs.txt"))?, "6");
    client.close().await;
    reopened.shutdown().await?;
    feed.write()
}

/// Holds the first listing after boot until `go` appears.
fn hold_codex_listing(server: &Path) -> Fallible {
    let recording = read(&server.join("stdout.jsonl"))?;
    let (prefix, body) = recording.split_at(
        recording
            .match_indices('\n')
            .nth(1)
            .ok_or("missing initialization")?
            .0
            .saturating_add(1),
    );
    std::fs::write(
        server.join("stdout.2.jsonl"),
        format!(
            "{prefix}{{\"fake\":\"await\",\"match\":{{\"method\":\"model/list\"}}}}\n{{\"fake\":\"files\",\"write\":{{\"held\":\"ready\"}}}}\n{{\"fake\":\"release\",\"file\":\"go\"}}\n{body}"
        ),
    )?;
    Ok(())
}

/// The Codex account's instance can't be removed while its session runs.
async fn assert_running_codex_is_not_removed(phone: &RawClient) -> Fallible {
    assert_eq!(
        phone
            .send(CommandBody::RemoveInstance {
                instance: api(Provider::Codex),
            })
            .await?
            .value,
        CommandResult::Rejected(Rejection::InstanceInUse { sessions: 1 })
    );
    Ok(())
}

#[tokio::test]
async fn provider_refresh_orders_codex_configuration_without_blocking_claude()
-> Fallible {
    let data = data_dir("refresh-codex")?;
    let source = scratch("refresh-codex-claude", "claude-refresh-exit.jsonl")?;
    std::fs::copy(
        recording_file("claude-approve.jsonl"),
        source.join("stdout.2.jsonl"),
    )?;
    let (claude_api, codex_api) = (api(Provider::Claude), api(Provider::Codex));
    let seeded = seed(
        &data,
        &[claude_api.clone(), codex_api.clone()],
        &[Provider::Claude, Provider::Codex],
    )?;
    let server =
        codex_server(&data, &codex_api, "codex-two-threads-listed.jsonl")?;
    hold_codex_listing(&server)?;
    probe(&data, &claude_api, CLAUDE_MODELS)?;
    let fx = serve(
        endpoint().await?,
        plugins(Some(fake_claude()), Some(fake_codex())),
        &data,
    )
    .await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = start_ready(&mut phone, claude_api.clone(), &source).await?;
    std::fs::write(source.join("exit-now"), "")?;
    raw_events_until(&mut phone, Until::Closed).await?;
    phone.close().await;
    let desktop = endpoint().await?;
    let conn = authenticated(&desktop, &fx.target).await?;
    let first =
        queued_command(&conn, 2311, codex_provider("conductor-test-old"));
    marker(&server.join("held")).await?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    assert_eq!(
        phone.instances.availability(&codex_api),
        Some(&Availability::Unavailable {
            reason: String::from(REFRESHING)
        })
    );
    phone.accepted(continued(session)).await?;
    raw_events_until(&mut phone, Until::Ready).await?;
    phone.started(start_on(claude_api, &source)).await?;
    raw_events_until(&mut phone, Until::Ready).await?;
    let middle_body = codex_provider("conductor-test-middle");
    let middle = queued_command(&conn, 2312, middle_body.clone());
    saved(&data, seeded.clone(), middle_body).await?;
    let newest_body = codex_provider("conductor-test-newest");
    let newest = queued_command(&conn, 2313, newest_body.clone());
    saved(&data, seeded, newest_body).await?;
    assert_eq!(read(&server.join("runs.txt"))?, "2");
    std::fs::write(server.join("go"), "")?;
    for task in [first, middle, newest] {
        assert_eq!(finished_command(task).await?, CommandResult::Accepted);
    }
    assert_eq!(
        codex_offer(current_offers(&fx.target).await?),
        Some(codex_gateway())
    );
    assert_eq!(read(&server.join("runs.txt"))?, "3");
    assert_ne!(
        read(&server.join("env.txt"))?.find("conductor-test-newest"),
        None
    );
    phone.started(start_on(codex_api, &server)).await?;
    raw_events_until(&mut phone, Until::Ready).await?;
    assert_running_codex_is_not_removed(&phone).await?;
    assert_eq!(read(&server.join("runs.txt"))?, "3");
    phone.close().await;
    desktop.close().await;
    feed.write()?;
    fx.shutdown().await
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
    let mut feed = Recorder::new()?;
    let history = raw_history(&second.target, &mut feed).await?;
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
    feed.write()?;
    second.shutdown().await
}

#[tokio::test]
async fn removal_rebuilds_its_missing_command_receipt_after_restart() -> Fallible
{
    let data = data_dir("removal-receipt")?;
    let instance = InstanceId::named(Provider::Claude, "removed account");
    let mut roster = proto::Roster::default();
    roster
        .0
        .insert(instance.clone(), proto::Instance::default());
    disk::seed_roster(&data, &roster)?;

    let first =
        durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let address = first.address()?;
    let target = first.target.clone();
    let endpoint = endpoint().await?;
    let conn = authenticated(&endpoint, &target).await?;
    let removal = proto::Command {
        id: proto::CommandId(72),
        body: CommandBody::RemoveInstance {
            instance: instance.clone(),
        },
    };
    let receipt = retry_command(&conn, &removal).await?;
    assert_eq!(receipt, CommandResult::Accepted);
    assert!(current_offers(&target).await?.of(&instance).is_none());
    first.shutdown().await?;

    // The roster is durable. Dropping the separate receipt restores the
    // on-disk state of an interruption between those two commits.
    assert!(disk::roster(&data)?.of(&instance).is_none());
    assert_eq!(disk::remove_receipt(&data, removal.id)?, receipt);

    let second = durable_host(&data, address).await?;
    let restarted = authenticated(&endpoint, &target).await?;
    assert_eq!(retry_command(&restarted, &removal).await?, receipt);
    let later = proto::Command {
        id: proto::CommandId(73),
        body: removal.body.clone(),
    };
    assert_eq!(
        retry_command(&restarted, &later).await?,
        CommandResult::Rejected(Rejection::UnknownInstance)
    );
    second.shutdown().await?;
    assert_eq!(disk::remove_receipt(&data, removal.id)?, receipt);
    endpoint.close().await;
    Ok(())
}

/// Restarts the durable host `first` on `address` and opens `phone` again
/// on the new one, resubscribed after the `applied` seq and caught up.
async fn restart(
    first: Fixture,
    data: &Path,
    address: SocketAddr,
    phone: RawClient,
    feed: &mut Recorder,
    applied: HostSeq,
) -> Fallible<(Fixture, RawClient)> {
    let epoch = phone.welcome.epoch;
    phone.close().await;
    first.shutdown().await?;
    let second = durable_host(data, address).await?;
    let mut phone =
        RawClient::open(&second.target, "test-client", feed).await?;
    assert_eq!(phone.welcome.epoch, epoch);
    phone.subscribe(applied).await?;
    phone.caught_up().await?;
    Ok((second, phone))
}

/// Whether any of `events` is of a kind `wanted` picks.
fn any_event(events: &[Event], wanted: impl Fn(&EventKind) -> bool) -> bool {
    events.iter().any(|event| wanted(&event.kind))
}

fn assert_absent(path: &Path) {
    assert_eq!(
        std::fs::metadata(path).err().map(|error| error.kind()),
        Some(std::io::ErrorKind::NotFound)
    );
}

async fn get_diff(
    phone: &RawClient,
    session: SessionId,
    snapshot: HostSeq,
) -> Fallible<proto::SnapshotDiff> {
    diff_result(
        phone
            .send(CommandBody::GetDiff { session, snapshot })
            .await?
            .value,
    )
}

fn assistant_messages(session: SessionId, events: &[Event]) -> Vec<EventKind> {
    of(session, events)
        .into_iter()
        .filter(|kind| matches!(kind, EventKind::AssistantMessage { .. }))
        .collect()
}

/// A turn in the `other` session, in `other_path`, changes only its own
/// worktree.
async fn assert_independent(
    phone: &mut RawClient,
    other: SessionId,
    other_path: &Path,
    worktree: &Path,
) -> Fallible<HostSeq> {
    assert_absent(&other_path.join("changed.txt"));
    phone
        .accepted(message(other, "an independent second session"))
        .await?;
    let other_turn = raw_events_until(phone, Until::TurnEnd).await?;
    snapshot(&other_turn)?;
    assert_eq!(
        std::fs::read_to_string(other_path.join("continued.txt"))?,
        "continued session\n"
    );
    assert_absent(&worktree.join("continued.txt"));
    Ok(other_turn.last().ok_or("missing turn")?.seq)
}

fn snapshot(events: &[Event]) -> Fallible<(HostSeq, proto::Snapshot)> {
    Ok(events
        .iter()
        .rev()
        .find_map(|event| match &event.kind {
            EventKind::SnapshotRecorded { snapshot } => {
                Some((event.seq, snapshot.clone()))
            }
            _ => None,
        })
        .ok_or("the snapshot was not published")?)
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

fn diff_result(result: CommandResult) -> Fallible<proto::SnapshotDiff> {
    match result {
        CommandResult::Diff(diff) => Ok(diff),
        other => Err(format!("expected a snapshot diff, not {other:?}").into()),
    }
}

/// A source checkout with staged, unstaged and untracked changes, whose
/// recordings continue on the second and third runs.
struct SourceCheckout {
    index: Vec<u8>,
    staged: String,
    unstaged: String,
}

impl SourceCheckout {
    fn dirty(source: &Path) -> Fallible<Self> {
        std::fs::copy(
            recording_file("claude-worktrees-continued.jsonl"),
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

fn assert_turn_tasks(
    kept: &[Vec<ChecklistEntry>],
    discarded: &[Vec<ChecklistEntry>],
) {
    assert_eq!(
        [kept, discarded].map(|lists| lists
            .iter()
            .map(|checklist| owned(steps(checklist)))
            .collect::<Vec<Vec<(String, ChecklistStatus)>>>()),
        [
            Vec::from([Vec::from([(
                String::from("turn one"),
                ChecklistStatus::Completed
            )])]),
            Vec::from([Vec::from([
                (String::from("turn one"), ChecklistStatus::Completed),
                (String::from("turn two"), ChecklistStatus::InProgress),
            ])]),
        ]
    );
}

/// The second turn's diff against the first: its files, its text, and the
/// executable bit it set.
fn assert_turn_diff(
    diff: &proto::SnapshotDiff,
    selected: &proto::Snapshot,
    worktree: &Path,
) -> Fallible {
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
        git(worktree, &["ls-tree", &selected.commit, "exec.sh"])?
            .split_whitespace()
            .next(),
        Some("100755")
    );
    Ok(())
}

#[tokio::test]
async fn managed_worktrees_isolate_source_and_keep_diffs_after_restart()
-> Fallible {
    let source = scratch("workspace-snapshots", "claude-worktrees.jsonl")?;
    let checkout = SourceCheckout::dirty(&source)?;
    let data = data_dir("workspace-snapshots")?;
    let first =
        durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let address = first.address()?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&first.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &source)).await?;
    let opening = raw_events_until(&mut phone, Until::Ready).await?;
    let managed = workspace(&opening)?;
    assert_eq!(snapshot(&opening)?.1.turn_start, None);
    let worktree = assert_isolated(&managed, &source, session)?;
    let other = phone.started(start(Provider::Claude, &source)).await?;
    let other_opening = raw_events_until(&mut phone, Until::Ready).await?;
    let other_workspace = workspace(&other_opening)?;
    let other_path = PathBuf::from(&other_workspace.worktree.0);
    assert_ne!(session, other);
    assert_ne!(managed.worktree, other_workspace.worktree);
    assert_ne!(managed.branch, other_workspace.branch);
    phone.accepted(message(session, "turn one")).await?;
    let first_turn = raw_events_until(&mut phone, Until::TurnEnd).await?;
    let (_, first_snapshot) = snapshot(&first_turn)?;
    phone.accepted(message(session, "turn two")).await?;
    let second_turn = raw_events_until(&mut phone, Until::TurnEnd).await?;
    let (second_seq, second_snapshot) = snapshot(&second_turn)?;
    let kept_tasks = task_lists(&first_turn);
    assert_turn_tasks(&kept_tasks, &task_lists(&second_turn));
    assert_ne!(first_snapshot.commit, second_snapshot.commit);
    let diff = get_diff(&phone, session, second_seq).await?;
    assert_turn_diff(&diff, &second_snapshot, &worktree)?;
    phone.accepted(message(session, "turn three")).await?;
    let third_turn = raw_events_until(&mut phone, Until::TurnEnd).await?;
    snapshot(&third_turn)?;
    checkout.assert_unchanged(&source)?;
    let applied =
        assert_independent(&mut phone, other, &other_path, &worktree).await?;
    let (second, mut restarted) =
        restart(first, &data, address, phone, &mut feed, applied).await?;
    assert_eq!(get_diff(&restarted, session, second_seq).await?, diff);
    restarted.accepted(continued(session)).await?;
    raw_events_until(&mut restarted, Until::Ready).await?;
    restarted
        .accepted(message(session, "continue after restart"))
        .await?;
    let after = raw_events_until(&mut restarted, Until::TurnEnd).await?;
    assert_eq!(
        assistant_messages(session, &after),
        [reply("Continued session.")]
    );
    let runs = argv(&source)?;
    let resumed = runs.last().ok_or("missing resumed invocation")?;
    assert!(resumed.iter().any(|arg| arg.starts_with("--resume=")));
    assert_eq!(get_diff(&restarted, session, second_seq).await?, diff);
    checkout.assert_unchanged(&source)?;
    restarted.close().await;
    feed.write()?;
    second.shutdown().await
}

#[tokio::test]
async fn new_sessions_require_an_existing_repository_with_a_commit() -> Fallible
{
    let host = host().await?;
    let mut feed = Recorder::new()?;
    let phone = RawClient::live(&host.target, "test-client", &mut feed).await?;
    let plain = data_dir("not-a-repository")?;
    // Stop Git discovery at this fixture instead of finding the enclosing checkout.
    std::fs::write(plain.join(".git"), "gitdir: no-such-git-directory\n")?;
    assert_eq!(
        phone.send(start(Provider::Claude, &plain)).await?.value,
        CommandResult::Rejected(Rejection::NotGitRepository)
    );
    std::fs::remove_file(plain.join(".git"))?;
    git(&plain, &["init", "--quiet"])?;
    assert_eq!(
        phone.send(start(Provider::Claude, &plain)).await?.value,
        CommandResult::Rejected(Rejection::NotGitRepository)
    );
    phone.close().await;
    feed.write()?;
    host.shutdown().await
}

/// Old persisted sessions have no workspace metadata. Continue still starts
/// their CLI in the original directory, even when it is not a Git repository.
#[tokio::test]
async fn a_session_without_a_worktree_continues_in_its_original_directory()
-> Fallible {
    let source = data_dir("legacy-session-source")?;
    std::fs::write(source.join(".git"), "gitdir: no-such-git-directory\n")?;
    std::fs::copy(
        recording_file("claude-worktrees-continued.jsonl"),
        source.join("stdout.jsonl"),
    )?;
    let data = data_dir("legacy-session-host")?;
    let session = SessionId(1310);
    disk::append(
        &data,
        session,
        [
            legacy_created(instance(Provider::Claude), &source),
            EventKind::SessionClosed {
                reason: CloseReason::Stopped {
                    reason: "old host stopped".into(),
                },
            },
        ],
    )?;
    let host =
        durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&host.target, "test-client", &mut feed).await?;
    phone.accepted(continued(session)).await?;
    let ready = raw_events_until(&mut phone, Until::Ready).await?;
    assert_eq!(
        ready.iter().find(|event| matches!(
            event.kind,
            EventKind::WorkspaceCreated { .. }
        )),
        None
    );
    phone.accepted(message(session, "legacy directory")).await?;
    raw_events_until(&mut phone, Until::TurnEnd).await?;
    assert_eq!(
        std::fs::read_to_string(source.join("continued.txt"))?,
        "continued session\n"
    );
    phone.close().await;
    feed.write()?;
    host.shutdown().await
}

#[tokio::test]
async fn an_idle_session_retries_a_failed_snapshot_on_continue_before_the_next_message()
-> Fallible {
    let source = scratch("snapshot-retry", "claude-worktrees.jsonl")?;
    let host = host().await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&host.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &source)).await?;
    let managed =
        workspace(&raw_events_until(&mut phone, Until::Ready).await?)?;
    let worktree = PathBuf::from(&managed.worktree.0);
    phone.accepted(message(session, "turn one")).await?;
    let (_, retained) =
        snapshot(&raw_events_until(&mut phone, Until::TurnEnd).await?)?;
    git(&worktree, &["symbolic-ref", "HEAD", "refs/heads/unrelated"])?;
    phone.accepted(message(session, "turn two")).await?;
    let failed = raw_events_until(&mut phone, Until::TurnEnd).await?;
    assert!(!any_event(&failed, |kind| matches!(
        kind,
        EventKind::SnapshotRecorded { .. }
    )));
    assert_eq!(
        failed
            .iter()
            .filter(|event| matches!(event.kind, EventKind::Error { .. }))
            .count(),
        1
    );
    let before = raw_history(&host.target, &mut feed).await?;
    let users = before
        .iter()
        .filter(|event| matches!(event.kind, EventKind::UserMessage { .. }))
        .count();
    assert!(matches!(
        phone
            .send(message(session, "must not enter native history"))
            .await?
            .value,
        CommandResult::Rejected(Rejection::WorkspaceFailure { .. })
    ));
    assert!(matches!(
        phone.send(continued(session)).await?.value,
        CommandResult::Rejected(Rejection::WorkspaceFailure { .. })
    ));
    let still = raw_history(&host.target, &mut feed).await?;
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
    phone.accepted(continued(session)).await?;
    phone.accepted(continued(session)).await?;
    let (recovered_seq, recovered) =
        snapshot(&raw_history(&host.target, &mut feed).await?)?;
    phone.accepted(message(session, "turn three")).await?;
    let after = raw_events_until(&mut phone, Until::TurnEnd).await?;
    let snapshots = after
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::SnapshotRecorded { snapshot } => Some(snapshot.clone()),
            _ => None,
        })
        .collect::<Vec<proto::Snapshot>>();
    assert!(!any_event(&after, |kind| matches!(
        kind,
        EventKind::SessionReady | EventKind::SessionClosed { .. }
    )));
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots.first(), Some(&recovered));
    let latest = snapshots.last().ok_or("expected a value")?;
    assert_ne!(recovered.commit, retained.commit);
    let failed_turn = failed
        .iter()
        .find(|event| matches!(event.kind, EventKind::UserMessage { .. }))
        .ok_or("missing failed turn")?
        .seq;
    assert_eq!(recovered.turn_start, Some(failed_turn));
    assert_ne!(latest.turn_start, recovered.turn_start);
    let (latest_seq, _) = snapshot(&after)?;
    assert!(latest_seq > recovered_seq);
    let diff = get_diff(&phone, session, latest_seq).await?;
    assert_eq!(
        diff.files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<String>>(),
        ["changed.txt", "later.txt"]
    );
    phone.close().await;
    feed.write()?;
    host.shutdown().await
}

const fn set_effort(session: SessionId, effort: Effort) -> CommandBody {
    CommandBody::SetEffort { session, effort }
}

/// The task lists in `events`, in order.
fn task_lists(events: &[Event]) -> Vec<Vec<ChecklistEntry>> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::ChecklistChanged { checklist } => {
                Some(checklist.clone())
            }
            _ => None,
        })
        .collect()
}

fn steps(checklist: &[ChecklistEntry]) -> Vec<(&str, ChecklistStatus)> {
    checklist
        .iter()
        .map(|task| (task.text.as_str(), task.status))
        .collect()
}

/// The three lists the recorded Claude and Codex runs wrote: three steps,
/// the first pending, then in progress, then done.
fn written(a: &str, b: &str, c: &str) -> Vec<Vec<(String, ChecklistStatus)>> {
    [
        ChecklistStatus::Pending,
        ChecklistStatus::InProgress,
        ChecklistStatus::Completed,
    ]
    .into_iter()
    .map(|first| {
        Vec::from([
            (String::from(a), first),
            (String::from(b), ChecklistStatus::Pending),
            (String::from(c), ChecklistStatus::Pending),
        ])
    })
    .collect()
}

fn owned(
    steps: Vec<(&str, ChecklistStatus)>,
) -> Vec<(String, ChecklistStatus)> {
    steps
        .into_iter()
        .map(|(text, status)| (String::from(text), status))
        .collect()
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

/// A Claude turn's `TodoWrite` calls become the session's checklist: one
/// `ChecklistChanged` per call, and no call shown as
/// a tool call. An effort set while the turn runs is checked against the
/// model's efforts, logged at once, and reaches the CLI with the next
/// message (`apply_flag_settings`, then the message), not during the turn.
#[tokio::test]
async fn a_claude_checklist_becomes_tasks_and_a_new_effort_waits_for_the_next_turn()
-> Fallible {
    let fx = host().await?;
    let dir = scratch("checklist", "claude-checklist-effort.jsonl")?;
    hold_first_turn(&dir)?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &dir)).await?;
    let opened =
        raw_conversation_events_until(&mut phone, Until::Ready).await?;
    let _ = phone.send(message(session, "plan")).await?;
    let mid_turn = [
        phone.send(set_effort(session, Effort::Max)).await?.value,
        phone.send(set_effort(session, Effort::Low)).await?.value,
        phone.send(set_effort(session, Effort::Low)).await?.value,
    ];
    let before = written_to_claude(&dir)?;
    std::fs::write(dir.join("release"), "")?;
    let turn =
        raw_conversation_events_until(&mut phone, Until::TurnEnd).await?;
    let lists = task_lists(&turn);
    assert_eq!(
        (
            mid_turn,
            before,
            lists
                .iter()
                .map(|checklist| owned(steps(checklist)))
                .collect::<Vec<Vec<(String, ChecklistStatus)>>>(),
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
            0,
            Vec::from([EventKind::EffortChanged {
                effort: Effort::Low
            }]),
        )
    );
    let _ = phone.send(message(session, "pong")).await?;
    let next =
        raw_conversation_events_until(&mut phone, Until::TurnEnd).await?;
    assert_eq!(
        written_to_claude(&dir)?,
        [
            "initialize ",
            "plan",
            r#"apply_flag_settings {"effortLevel":"low"}"#,
            "pong"
        ]
    );
    assert_eq!(
        latest_effort_and_checklist(&[opened, turn, next].concat()),
        (Some(Effort::Low), lists.last().cloned())
    );
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

/// A Codex turn's plan updates become the session's checklist as Claude's
/// checklist does, and a new effort rides on the next `turn/start`.
#[tokio::test]
async fn a_codex_plan_becomes_tasks_and_a_new_effort_starts_the_next_turn()
-> Fallible {
    let (fx, dir) = codex_host("codex-plan-effort.jsonl").await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Codex, &dir)).await?;
    let _ = raw_conversation_events_until(&mut phone, Until::Ready).await?;
    let _ = phone.send(message(session, "plan")).await?;
    let turn =
        raw_conversation_events_until(&mut phone, Until::TurnEnd).await?;
    let lists = task_lists(&turn);
    let set = phone.send(set_effort(session, Effort::High)).await?.value;
    let _ = phone.send(message(session, "pong")).await?;
    let next = raw_kinds_until(&mut phone, Until::TurnEnd).await?;
    assert_eq!(
        (
            lists
                .iter()
                .map(|checklist| owned(steps(checklist)))
                .collect::<Vec<Vec<(String, ChecklistStatus)>>>(),
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
    phone.close().await;
    feed.write()?;
    fx.shutdown().await
}

#[tokio::test]
async fn tasks_and_effort_survive_a_host_restart() -> Fallible {
    let dir = scratch("checklist-restart", "claude-checklist-effort.jsonl")?;
    let data = data_dir("checklist-restart")?;
    std::fs::copy(dir.join("stdout.jsonl"), dir.join("stdout.2.jsonl"))?;
    let first =
        durable_host(&data, SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
    let address = first.address()?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&first.target, "test-client", &mut feed).await?;
    let session = phone.started(start(Provider::Claude, &dir)).await?;
    let _ = phone.send(message(session, "plan")).await?;
    let turn = raw_events_until(&mut phone, Until::TurnEnd).await?;
    let before = task_lists(&turn);
    let cursor = (
        phone.welcome.epoch,
        turn.last().ok_or("the turn had no events")?.seq,
    );
    let set = phone.send(set_effort(session, Effort::Low)).await?.value;
    phone.close().await;
    first.shutdown().await?;

    let second = durable_host(&data, address).await?;
    let replayed = raw_history(&second.target, &mut feed).await?;
    assert_eq!(task_lists(&replayed).last(), before.last());
    let mut phone =
        RawClient::open(&second.target, "test-client", &mut feed).await?;
    phone.subscribe_in(cursor.0, cursor.1).await?;
    let _ = phone.caught_up().await?;
    let continued = phone.send(continued(session)).await?.value;
    let _ = raw_kinds_until(&mut phone, Until::Ready).await?;
    let _ = phone.send(message(session, "plan")).await?;
    let after = task_lists(
        &raw_conversation_events_until(&mut phone, Until::TurnEnd).await?,
    );
    let history = raw_history(&second.target, &mut feed).await?;
    assert_eq!(
        (
            set,
            continued,
            argv(&dir)?
                .iter()
                .map(|run| flag(run, "--effort"))
                .collect::<Vec<Option<String>>>(),
            after.len(),
            latest_effort_and_checklist(&history),
        ),
        (
            CommandResult::Accepted,
            CommandResult::Accepted,
            Vec::from([None, Some(String::from("low"))]),
            3,
            (Some(Effort::Low), after.last().cloned()),
        )
    );
    phone.close().await;
    feed.write()?;
    second.shutdown().await
}

/// The effort and the checklist that the last `EffortChanged` and the last
/// `ChecklistChanged` in `events` set.
fn latest_effort_and_checklist(
    events: &[Event],
) -> (Option<Effort>, Option<Vec<ChecklistEntry>>) {
    let effort = events.iter().rev().find_map(|event| match event.kind {
        EventKind::EffortChanged { effort } => Some(effort),
        _ => None,
    });
    (effort, task_lists(events).pop())
}

/// The code the sign-in stand-in accepts (fixtures/fake-login.py).
const SIGN_IN_CODE: &str = "conductor-test-sign-in-code#fake-state";
/// One it refuses.
const WRONG_CODE: &str = "conductor-test-wrong-code";

/// The instance's next sign-in step the client reads that `done` takes.
async fn sign_in_until(
    client: &mut RawClient,
    instance: &InstanceId,
    done: impl Fn(&proto::SignInStep) -> bool,
) -> Fallible<proto::SignInStep> {
    loop {
        if let Feed::SignIn(proto::SignIn {
            instance: signing_in,
            step,
        }) = client.next().await?.value
            && signing_in == *instance
            && done(&step)
        {
            break Ok(step);
        }
    }
}

async fn app_sign_in(
    client: &mut RawClient,
    id: &InstanceId,
) -> Fallible<proto::SignInStep> {
    client
        .accepted(CommandBody::StartSignIn {
            instance: id.clone(),
        })
        .await?;
    let _ = sign_in_until(client, id, |step| {
        matches!(step, proto::SignInStep::Open { .. })
    })
    .await?;
    client
        .accepted(CommandBody::SubmitSignInCode {
            instance: id.clone(),
            code: proto::SignInCode(String::from(SIGN_IN_CODE)),
        })
        .await?;
    sign_in_until(client, id, |step| {
        matches!(
            step,
            proto::SignInStep::SignedIn | proto::SignInStep::Failed { .. }
        )
    })
    .await
}

fn sign_in_home(data: &Path, id: &InstanceId) -> Fallible<PathBuf> {
    install_fake(data, Provider::Claude, &fake_claude())?;
    let home = dir_of(data, "home", id);
    std::fs::create_dir_all(&home)?;
    probe(data, id, PROBED)?;
    Ok(home)
}

fn sign_in_plugins() -> Plugins {
    plugins(None, Some(PathBuf::from(REFUSING_CLI)))
}

async fn terminal_sign_in(data: &Path, id: &InstanceId) -> Fallible {
    let home = dir_of(data, "home", id);
    let approval = home.join(".fake-approve");
    std::fs::write(approval, "")?;
    let name = match &id.account {
        proto::Account::Own => None,
        proto::Account::Named(name) => Some(name.clone()),
    };
    Ok(daemon::login(
        Some(data.to_path_buf()),
        sign_in_plugins(),
        id.provider,
        name,
    )
    .await?)
}

#[tokio::test]
async fn app_and_terminal_sign_in_keep_ownership_when_the_status_probe_fails()
-> Fallible {
    let own = instance(Provider::Claude);
    let mut feed = Recorder::new()?;
    for status in ["nonzero", "signal", "timeout"] {
        let app = data_dir(&format!("app-sign-in-{status}"))?;
        let terminal = data_dir(&format!("terminal-sign-in-{status}"))?;
        for data in [&app, &terminal] {
            let home = sign_in_home(data, &own)?;
            std::fs::write(home.join(".fake-status"), status)?;
        }
        let fx = serve(endpoint().await?, sign_in_plugins(), &app).await?;
        let mut phone =
            RawClient::live(&fx.target, "test-client", &mut feed).await?;
        let (app_result, terminal_result) = tokio::join!(
            app_sign_in(&mut phone, &own),
            terminal_sign_in(&terminal, &own),
        );

        let Err(error) = terminal_result else {
            return Err(format!(
                "the terminal probe should fail with {status}"
            )
            .into());
        };
        assert_eq!(
            app_result?,
            proto::SignInStep::Failed {
                reason: error.to_string(),
            },
            "{status}"
        );
        for data in [&app, &terminal] {
            let named = InstanceId::named(
                Provider::Claude,
                "conductor-test@example.invalid",
            );
            assert_eq!(
                (
                    data.join("credentials").exists(),
                    dir_of(data, "home", &own).join(".fake-signed-in").exists(),
                    dir_of(data, "home", &named).exists(),
                    dir_of(data, "work", &own).join("stdout.jsonl").exists(),
                    dir_of(data, "work", &named).exists(),
                ),
                (false, true, false, true, false),
                "{status} at {}",
                data.display(),
            );
        }
        phone.close().await;
        fx.shutdown().await?;
    }
    feed.write()
}

#[tokio::test]
async fn app_refresh_and_terminal_restart_show_the_same_signed_in_instance()
-> Fallible {
    let own = instance(Provider::Claude);
    let named =
        InstanceId::named(Provider::Claude, "conductor-test@example.invalid");
    let mut feed = Recorder::new()?;
    for (case, source, target) in [
        ("no-account", &own, &own),
        ("same-account", &named, &named),
        ("new-account", &own, &named),
    ] {
        let app = data_dir(&format!("app-sign-in-{case}"))?;
        let terminal = data_dir(&format!("terminal-sign-in-{case}"))?;
        for data in [&app, &terminal] {
            sign_in_home(data, &own)?;
            if source == &named {
                terminal_sign_in(data, &own).await?;
                let home = dir_of(data, "home", source);
                std::fs::remove_file(home.join(".fake-approve"))?;
                std::fs::remove_file(home.join(".fake-signed-in"))?;
            }
            let home = dir_of(data, "home", source);
            if target == &own {
                std::fs::write(home.join(".fake-account"), "")?;
            }
            std::fs::write(home.join("sign-in-home"), case)?;
            std::fs::write(
                dir_of(data, "work", source).join("sign-in-work"),
                case,
            )?;
        }
        let fx = serve(endpoint().await?, sign_in_plugins(), &app).await?;
        let mut phone =
            RawClient::live(&fx.target, "test-client", &mut feed).await?;
        assert_eq!(
            app_sign_in(&mut phone, source).await?,
            proto::SignInStep::SignedIn
        );
        wait_for_offer(&mut phone, target).await?;
        terminal_sign_in(&terminal, source).await?;
        let reopened =
            serve(endpoint().await?, sign_in_plugins(), &terminal).await?;
        let mut desk =
            RawClient::live(&reopened.target, "test-client", &mut feed).await?;
        wait_for_offer(&mut desk, target).await?;

        for (data, instances) in
            [(&app, &phone.instances), (&terminal, &desk.instances)]
        {
            assert_eq!(
                (
                    instances.of(&own).is_some(),
                    instances.of(&named).is_some(),
                    instances.availability(target),
                    std::fs::read_to_string(
                        dir_of(data, "home", target).join("sign-in-home")
                    )?,
                    std::fs::read_to_string(
                        dir_of(data, "work", target).join("sign-in-work")
                    )?,
                ),
                (
                    true,
                    target == &named,
                    Some(&Availability::Available {
                        auth: Auth::SignedIn,
                        models: probed(),
                    }),
                    String::from(case),
                    String::from(case)
                ),
                "{case} at {}",
                data.display(),
            );
            if source != target {
                assert!(
                    !dir_of(data, "home", source).join("sign-in-home").exists()
                        && !dir_of(data, "work", source)
                            .join("sign-in-work")
                            .exists()
                );
            }
        }
        phone.close().await;
        fx.shutdown().await?;
        desk.close().await;
        reopened.shutdown().await?;
    }
    feed.write()
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
    probe(&data, &instance(Provider::Claude), PROBED)?;
    let fx = plugin_host(&data).await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
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
            proto::SignInStep::Failed { .. } | proto::SignInStep::SignedIn
        )
    };

    let started = phone.send(start()).await?.value;
    let shown = sign_in_until(&mut phone, &claude, open).await?;
    let again = phone.send(start()).await?.value;
    let wrong = phone.send(submit(WRONG_CODE)).await?.value;
    let refused = sign_in_until(&mut phone, &claude, ended).await?;
    let restarted = phone.send(start()).await?.value;
    let _ = sign_in_until(&mut phone, &claude, open).await?;
    let right = phone.send(submit(SIGN_IN_CODE)).await?.value;
    let signed_in = sign_in_until(&mut phone, &claude, ended).await?;
    wait_for_offer(&mut phone, &account).await?;
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
            signed_in,
            phone
                .instances
                .of(&account)
                .cloned()
                .map(|it| it.availability),
            dir_of(&data, "home", &account)
                .join(".fake-signed-in")
                .exists(),
            dir_of(&data, "home", &claude)
                .join(".fake-signed-in")
                .exists(),
        ),
        (proto::SignInStep::SignedIn, Some(now), true, false),
        "the signed-in home moved to the account's instance, and Claude's own is fresh"
    );
    phone.close().await;
    fx.shutdown().await?;
    drop(logging);
    assert_no_code_kept(&log_path, &data)?;
    feed.write()
}

#[tokio::test]
async fn app_retry_finishes_adoption_interrupted_before_the_roster_save()
-> Fallible {
    let data = data_dir("sign-in-adoption-retry")?;
    let own = instance(Provider::Claude);
    let account =
        InstanceId::named(Provider::Claude, "conductor-test@example.invalid");
    probe(&data, &own, PROBED)?;
    let fx = plugin_host(&data).await?;
    let mut feed = Recorder::new()?;
    let mut phone =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let start = || CommandBody::StartSignIn {
        instance: own.clone(),
    };
    let blocked = data.join("credentials.new");
    std::fs::create_dir(&blocked)?;

    phone.accepted(start()).await?;
    sign_in_until(&mut phone, &own, |step| {
        matches!(step, proto::SignInStep::Open { .. })
    })
    .await?;
    phone
        .accepted(CommandBody::SubmitSignInCode {
            instance: own.clone(),
            code: proto::SignInCode(String::from(SIGN_IN_CODE)),
        })
        .await?;
    sign_in_until(&mut phone, &own, |step| {
        matches!(step, proto::SignInStep::Failed { .. })
    })
    .await?;
    std::fs::remove_dir(blocked)?;
    assert_eq!(
        phone
            .send(codex_provider("unrelated-credential"))
            .await?
            .value,
        CommandResult::Rejected(Rejection::CredentialNotSaved {
            reason: String::from(
                "an earlier account adoption still needs recovery"
            )
        })
    );
    phone.accepted(start()).await?;
    let done = sign_in_until(&mut phone, &own, |step| {
        matches!(step, proto::SignInStep::SignedIn)
    })
    .await?;
    assert_eq!(done, proto::SignInStep::SignedIn);
    wait_for_offer(&mut phone, &account).await?;
    assert_eq!(
        phone.instances.availability(&account),
        Some(&Availability::Available {
            auth: Auth::SignedIn,
            models: probed(),
        })
    );
    assert!(phone.instances.of(&own).is_some());
    assert!(
        dir_of(&data, "home", &account)
            .join(".fake-signed-in")
            .try_exists()?
    );
    phone.close().await;
    fx.shutdown().await?;
    feed.write()
}

/// Neither sign-in code is in the log at `log_path` or a file under
/// `data`, and the log has both submits, without their text, and one
/// sign-in that took.
fn assert_no_code_kept(log_path: &Path, data: &Path) -> Fallible {
    let logged = captured(log_path)?;
    assert_eq!(
        (
            logged.contains(SIGN_IN_CODE) || logged.contains(WRONG_CODE),
            logged.matches(r#"operation="submit_sign_in_code""#).count(),
            logged
                .matches("sign-in instance=claude step=signed in")
                .count(),
            holding(data, SIGN_IN_CODE)?,
            holding(data, WRONG_CODE)?,
        ),
        (false, 2, 1, Vec::new(), Vec::new())
    );
    for live_only in ["Opening browser to sign in", "code_challenge=CHALLENGE"]
    {
        assert!(!logged.contains(live_only));
        assert_eq!(holding(data, live_only)?, Vec::<PathBuf>::new());
    }
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

/// The flow Conductor's own home declares, which a test home's issues
/// follow.
const FLOW: &str = include_str!("../../home/src/flow/test-flow.toml");

/// A project's `project.toml` in the home at `home`.
/// Alpha's issues, from the home's root.
const ALPHA_ISSUES: &str = "projects/a1pha0/issues";

/// The id of the project the tests call `name`; gamma is listed nowhere.
fn project_id(name: &str) -> ProjectId {
    let id = match name {
        "alpha" => "a1pha0",
        "beta" => "beta00",
        _ => "gamma0",
    };
    ProjectId::parse(id).unwrap_or(ProjectId::new(0))
}

fn listed_project(home: &Path, name: &str, fields: (&str, &str)) -> Fallible {
    let (remote, root) = fields;
    let folder = home.join("projects").join(project_id(name).to_string());
    std::fs::create_dir_all(&folder)?;
    std::fs::write(
        folder.join("project.toml"),
        format!(
            "name = \"{name}\"\ntitle = \"{name}\"\nremote = \"{remote}\"\n\
             root = \"{root}\"\nprofiles = [\"rust\"]\n"
        ),
    )?;
    Ok(())
}

/// The home as the client has it, once it is as `done` wants.
async fn home_until(
    client: &mut RawClient,
    done: impl Fn(&proto::Home) -> bool,
) -> Fallible<proto::Home> {
    loop {
        if let Some(home) = client.home.as_ref().filter(|home| done(home)) {
            break Ok(home.clone());
        }
        let _ = client.next().await?;
    }
}

fn checkouts_of(home: &proto::Home, name: &str) -> Vec<proto::Checkout> {
    home.projects
        .iter()
        .find(|project| project.id == project_id(name))
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

impl Homeland {
    fn go_offline(&self) -> std::io::Result<()> {
        std::fs::rename(&self.home_bare, self.scratch.join("home.gone"))
    }

    fn go_online(&self) -> std::io::Result<()> {
        std::fs::rename(self.scratch.join("home.gone"), &self.home_bare)
    }
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
        recording_file("claude-approve.jsonl"),
        roots.join("apps").join("alpha").join("stdout.jsonl"),
    )?;
    let home_source = sources.join("home");
    std::fs::create_dir_all(&home_source)?;
    git(&home_source, &["init", "--quiet"])?;
    listed_project(&home_source, "alpha", (&alpha, &alpha_root))?;
    listed_project(&home_source, "beta", (&beta, &beta_root))?;
    let agents = home_source.join("docs").join("agents");
    std::fs::create_dir_all(&agents)?;
    std::fs::write(agents.join("flow.toml"), FLOW)?;
    git(&home_source, &["add", "projects", "docs"])?;
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
        project: project_id(id),
    }
}

/// beta, with no checkout, is cloned into the data dir; an unknown project
/// is refused.
async fn clones_beta(
    link: &mut RawClient,
    data: &Path,
) -> Fallible<proto::Home> {
    link.accepted(clone_project("beta")).await?;
    let cloned =
        home_until(link, |home| !checkouts_of(home, "beta").is_empty()).await?;
    assert_eq!(
        checkouts_of(&cloned, "beta"),
        [found(
            &data.join("projects").join(project_id("beta").to_string()),
            proto::Origin::Cloned
        )?]
    );
    assert_eq!(
        link.send(clone_project("gamma")).await?.value,
        CommandResult::Rejected(Rejection::UnknownProject)
    );
    Ok(cloned)
}

/// A session in alpha starts in the person's checkout, which stays as it
/// was; an unknown project is refused.
async fn starts_in_alpha(link: &mut RawClient, land: &Homeland) -> Fallible {
    let in_project = |id: &str| CommandBody::StartSession {
        instance: instance(Provider::Claude),
        at: StartAt::Project(project_id(id)),
        model: None,
        effort: None,
    };
    assert_eq!(
        link.send(in_project("gamma")).await?.value,
        CommandResult::Rejected(Rejection::UnknownProject)
    );
    let session = link.started(in_project("alpha")).await?;
    let events = raw_events_until(link, Until::Ready).await?;
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
    let mut feed = Recorder::new()?;
    let land = homeland("projects")?;
    let (fx, data) =
        host_at_home(land.home_url.clone(), Vec::from([land.roots.clone()]))
            .await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let synced = home_until(&mut link, |home| {
        home.sync == proto::HomeSync::Synced && home.projects.len() == 2
    })
    .await?;
    assert_eq!(
        (
            synced.remote.as_str(),
            synced.synced.is_some(),
            data.join("home-repo/projects")
                .join(project_id("alpha").to_string())
                .join("project.toml")
                .exists(),
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
    let _ = clones_beta(&mut link, &data).await?;
    starts_in_alpha(&mut link, &land).await?;

    land.go_offline()?;
    link.accepted(CommandBody::SyncHome).await?;
    let behind = home_until(&mut link, |home| {
        matches!(home.sync, proto::HomeSync::Behind { .. })
    })
    .await?;
    // The clone stays as of its last sync, with its projects.
    assert_eq!((behind.synced.is_some(), behind.projects.len()), (true, 2));
    assert!(behind.synced >= synced.synced);
    link.close().await;
    fx.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

fn alpha_issues(home: &proto::Home) -> Vec<proto::Issue> {
    home.projects
        .iter()
        .find(|project| project.id == project_id("alpha"))
        .map_or_else(Vec::new, |project| project.issues.clone())
}

fn issue_name(issue: proto::IssueId) -> String {
    issue.to_string()
}

/// The issue's file as the home's remote has it.
fn pushed_issue(land: &Homeland, issue: proto::IssueId) -> Fallible<String> {
    let path = format!("HEAD:{ALPHA_ISSUES}/{}.md", issue_name(issue));
    git(&land.home_bare, &["show", &path])
}

fn retitle(title: &str) -> IssueChange {
    IssueChange::Title(String::from(title))
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
        .join(ALPHA_ISSUES)
        .join(format!("{}.md", issue_name(issue)));
    let text = std::fs::read_to_string(&file)?;
    std::fs::write(&file, format!("{text}\n## Notes\n- by hand\n"))?;
    git(&person, &["commit", "--quiet", "-am", "a note by hand"])?;
    git(&person, &["push", "--quiet"])?;
    Ok(())
}

fn create_issue(title: &str) -> CommandBody {
    create_issue_in("alpha", title)
}

fn create_issue_in(project: &str, title: &str) -> CommandBody {
    CommandBody::CreateIssue(proto::NewIssue {
        project: project_id(project),
        kind: String::from("task"),
        title: String::from(title),
        fields: Vec::from([(
            String::from("labels"),
            proto::Datum::Texts(Vec::from([String::from("app")])),
        )]),
    })
}

/// An issue without a title is refused, and one with one is opened in
/// alpha and pushed.
async fn opens_an_issue(
    link: &mut RawClient,
    land: &Homeland,
) -> Fallible<proto::IssueId> {
    assert_eq!(
        link.send(create_issue(" ")).await?.value,
        CommandResult::Rejected(Rejection::IssueRefused {
            reason: String::from("an issue needs a title")
        })
    );
    let issue = match link.send(create_issue("Sign in")).await?.value {
        CommandResult::IssueWritten { issue, .. } => issue,
        other => return Err(format!("expected an issue, not {other:?}").into()),
    };
    let opened = home_until(link, |home| {
        home.unpushed == 0 && !alpha_issues(home).is_empty()
    })
    .await?;
    assert_eq!(
        alpha_issues(&opened)
            .iter()
            .map(|shown| (shown.id, shown.title.clone(), shown.status.clone()))
            .collect::<Vec<_>>(),
        [(issue, String::from("Sign in"), String::from("open"))]
    );
    assert!(pushed_issue(land, issue)?.contains("title = \"Sign in\""));
    Ok(issue)
}

/// A change made while the home's remote is away is kept, signed, in the
/// clone, and lands on the next sync.
async fn lands_an_offline_note(
    link: &mut RawClient,
    land: &Homeland,
    issue: proto::IssueId,
) -> Fallible {
    land.go_offline()?;
    wrote(
        link,
        change(issue, IssueChange::Note(String::from("offline"))),
    )
    .await?;
    let waiting = home_until(link, |home| home.unpushed == 1).await?;
    let signed = format!("test-host·{} note: offline", test_client().0);
    assert!(
        alpha_issues(&waiting)
            .iter()
            .flat_map(|shown| shown
                .log
                .iter()
                .map(home::flow::file::log_line)
                .collect::<Vec<_>>())
            .any(|note| note.ends_with(&signed)),
        "the clone has the note, signed by the host and the client, before the remote does"
    );
    land.go_online()?;
    link.accepted(CommandBody::SyncHome).await?;
    let _ = home_until(link, |home| home.unpushed == 0).await?;
    let landed = pushed_issue(land, issue)?;
    assert!(
        landed.contains(&signed) && landed.contains("- by hand"),
        "{landed}"
    );
    Ok(())
}

async fn create_offline_issue(link: &RawClient) -> Fallible<proto::IssueId> {
    let issue = match link.send(create_issue("Offline issue")).await?.value {
        CommandResult::IssueWritten { issue, .. } => issue,
        other => {
            return Err(format!("expected a saved issue, got {other:?}").into());
        }
    };
    wrote(
        link,
        change(issue, IssueChange::Note(String::from("offline note"))),
    )
    .await?;
    Ok(issue)
}

fn home_writer(land: &Homeland) -> Fallible<PathBuf> {
    let writer = land.scratch.join("writer");
    git(
        &land.scratch,
        &[
            "clone",
            "--quiet",
            &land.home_url,
            &writer.display().to_string(),
        ],
    )?;
    Ok(writer)
}

fn push_home_edit(writer: &Path) -> Fallible {
    git(writer, &["add", "-A"])?;
    git(writer, &["commit", "--quiet", "-m", "Edit home"])?;
    git(writer, &["push", "--quiet"])?;
    Ok(())
}

#[tokio::test]
async fn a_skill_added_to_the_home_is_installed_for_every_harness_once_it_syncs()
-> Fallible {
    let mut feed = Recorder::new()?;
    let land = homeland("skills")?;
    let user = land.scratch.join("user");
    let (fx, _) =
        host_at_home_for(land.home_url.clone(), Vec::new(), Some(user.clone()))
            .await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let _ = home_until(&mut link, |home| home.projects.len() == 2).await?;
    let writer = home_writer(&land)?;
    let added = writer.join("skills").join("build");
    std::fs::create_dir_all(&added)?;
    std::fs::write(added.join("SKILL.md"), "the build skill")?;
    push_home_edit(&writer)?;
    link.accepted(CommandBody::SyncHome).await?;
    for harness in [".claude", ".agents"] {
        let installed = user.join(harness).join("skills/build/SKILL.md");
        assert_eq!(std::fs::read_to_string(&installed)?, "the build skill");
    }
    link.close().await;
    fx.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

#[tokio::test]
async fn offline_creation_and_note_converge_after_a_failed_queue_shortening()
-> Fallible {
    let mut feed = Recorder::new()?;
    let land = homeland("issue-queue-shortening")?;
    let (fx, data) = host_at_home(land.home_url.clone(), Vec::new()).await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let _ = home_until(&mut link, |home| home.projects.len() == 2).await?;
    land.go_offline()?;
    let issue = create_offline_issue(&link).await?;
    let _ = home_until(&mut link, |home| home.unpushed == 2).await?;
    let state = data.join("home-edits.state");
    let saved = std::fs::read(&state)?;
    let blocked = data.join("home-edits.new");
    std::fs::create_dir(&blocked)?;
    link.close().await;
    fx.shutdown().await?;

    let reopened = serve(endpoint().await?, claude_plugins(), &data).await?;
    let mut link =
        RawClient::live(&reopened.target, "test-client", &mut feed).await?;
    let waiting = home_until(&mut link, |home| home.unpushed == 2).await?;
    assert_eq!(
        alpha_issues(&waiting).first().map(|shown| shown.id),
        Some(issue)
    );
    land.go_online()?;
    link.accepted(CommandBody::SyncHome).await?;
    assert_eq!(
        pushed_issue(&land, issue)?
            .matches(": offline note")
            .count(),
        1
    );
    assert_eq!(std::fs::read(&state)?, saved);

    let (second, _) = host_at_home(land.home_url.clone(), Vec::new()).await?;
    let mut peer =
        RawClient::live(&second.target, "test-client", &mut feed).await?;
    let _ =
        home_until(&mut peer, |home| !alpha_issues(home).is_empty()).await?;
    wrote(
        &peer,
        change(issue, IssueChange::Note(String::from("second host note"))),
    )
    .await?;
    let pushed = git(&land.home_bare, &["rev-parse", "HEAD"])?;
    link.close().await;
    reopened.shutdown().await?;

    let reopened = serve(endpoint().await?, claude_plugins(), &data).await?;
    let mut link =
        RawClient::live(&reopened.target, "test-client", &mut feed).await?;
    let _ = home_until(&mut link, |home| home.unpushed == 2).await?;
    std::fs::remove_dir(&blocked)?;
    link.accepted(CommandBody::SyncHome).await?;
    let landed = home_until(&mut link, |home| home.unpushed == 0).await?;
    assert_eq!(
        alpha_issues(&landed)
            .iter()
            .map(|shown| shown.id)
            .collect::<Vec<_>>(),
        [issue]
    );
    let text = pushed_issue(&land, issue)?;
    assert_eq!(text.matches(": offline note").count(), 1);
    assert_eq!(text.matches(": second host note").count(), 1);
    assert_eq!(git(&land.home_bare, &["rev-parse", "HEAD"])?, pushed);
    assert_ne!(std::fs::read(&state)?, saved);
    peer.close().await;
    second.shutdown().await?;
    link.close().await;
    reopened.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

#[tokio::test]
async fn a_blocked_creation_keeps_its_dependent_note_for_replay() -> Fallible {
    let mut feed = Recorder::new()?;
    let land = homeland("blocked-creation")?;
    let (fx, data) = host_at_home(land.home_url.clone(), Vec::new()).await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let _ = home_until(&mut link, |home| home.projects.len() == 2).await?;
    let other = match link
        .send(create_issue_in("beta", "Independent issue"))
        .await?
        .value
    {
        CommandResult::IssueWritten { issue, .. } => issue,
        result => {
            return Err(format!("expected an issue, got {result:?}").into());
        }
    };
    let writer = home_writer(&land)?;
    land.go_offline()?;
    let issue = create_offline_issue(&link).await?;
    wrote(
        &link,
        change(other, IssueChange::Note(String::from("independent note"))),
    )
    .await?;
    let _ = home_until(&mut link, |home| home.unpushed == 3).await?;
    let state = data.join("home-edits.state");
    let saved = std::fs::read(&state)?;
    land.go_online()?;

    let obstruction = writer.join(ALPHA_ISSUES);
    std::fs::write(
        &obstruction,
        "The issue directory is blocked by this file.",
    )?;
    push_home_edit(&writer)?;
    link.accepted(CommandBody::SyncHome).await?;
    let _ = home_until(&mut link, |home| home.unpushed == 2).await?;
    assert_ne!(std::fs::read(&state)?, saved);
    let other_file = format!(
        "HEAD:projects/{}/issues/{}.md",
        project_id("beta"),
        issue_name(other)
    );
    assert!(
        git(&land.home_bare, &["show", &other_file])?
            .contains(": independent note")
    );

    git(&writer, &["pull", "--quiet", "--ff-only"])?;
    std::fs::remove_file(obstruction)?;
    push_home_edit(&writer)?;
    link.accepted(CommandBody::SyncHome).await?;
    let _ = home_until(&mut link, |home| home.unpushed == 0).await?;
    assert_eq!(
        pushed_issue(&land, issue)?
            .matches(": offline note")
            .count(),
        1
    );
    link.close().await;
    fx.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

#[tokio::test]
async fn a_pending_note_preserves_the_tail_of_a_large_upstream_issue()
-> Fallible {
    let mut feed = Recorder::new()?;
    let land = homeland("large-upstream-issue")?;
    let (fx, _) = host_at_home(land.home_url.clone(), Vec::new()).await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let _ = home_until(&mut link, |home| home.projects.len() == 2).await?;
    let issue = opens_an_issue(&mut link, &land).await?;
    let writer = home_writer(&land)?;
    land.go_offline()?;
    wrote(
        &link,
        change(issue, IssueChange::Note(String::from("offline addition"))),
    )
    .await?;
    let _ = home_until(&mut link, |home| home.unpushed == 1).await?;
    land.go_online()?;

    let file = writer
        .join(ALPHA_ISSUES)
        .join(format!("{}.md", issue_name(issue)));
    let upstream = ": large upstream note\n  ".to_owned()
        + &"x".repeat(80_000)
        + "\n  upstream tail\n";
    let original = std::fs::read_to_string(&file)?;
    std::fs::write(
        file,
        format!("{original}\n## Notes\n\n- 2026-01-01 Writer{upstream}"),
    )?;
    push_home_edit(&writer)?;
    link.accepted(CommandBody::SyncHome).await?;
    let _ = home_until(&mut link, |home| home.unpushed == 0).await?;
    let landed = pushed_issue(&land, issue)?;
    assert!(landed.contains("upstream tail"));
    assert_eq!(landed.matches(": offline addition").count(), 1);
    link.close().await;
    fx.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

#[tokio::test]
async fn a_pending_note_waits_for_a_readable_issue() -> Fallible {
    let mut feed = Recorder::new()?;
    let land = homeland("unreadable-issue")?;
    let (fx, data) = host_at_home(land.home_url.clone(), Vec::new()).await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let _ = home_until(&mut link, |home| home.projects.len() == 2).await?;
    let issue = opens_an_issue(&mut link, &land).await?;
    let writer = home_writer(&land)?;
    let file = writer
        .join(ALPHA_ISSUES)
        .join(format!("{}.md", issue_name(issue)));
    let original = std::fs::read(&file)?;
    land.go_offline()?;
    wrote(
        &link,
        change(issue, IssueChange::Note(String::from("wait for repair"))),
    )
    .await?;
    let _ = home_until(&mut link, |home| home.unpushed == 1).await?;
    let state = data.join("home-edits.state");
    let saved = std::fs::read(&state)?;
    land.go_online()?;

    for unreadable in [b"invalid UTF-8: \xff".as_slice(), b"malformed Markdown"]
    {
        std::fs::write(&file, unreadable)?;
        push_home_edit(&writer)?;
        link.accepted(CommandBody::SyncHome).await?;
        assert_eq!(std::fs::read(&state)?, saved);
    }

    std::fs::write(file, original)?;
    push_home_edit(&writer)?;
    link.accepted(CommandBody::SyncHome).await?;
    let _ = home_until(&mut link, |home| home.unpushed == 0).await?;
    assert_eq!(
        pushed_issue(&land, issue)?
            .matches(": wait for repair")
            .count(),
        1
    );
    link.close().await;
    fx.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

#[tokio::test]
async fn a_stale_queue_does_not_recreate_an_issue_deleted_upstream() -> Fallible
{
    let mut feed = Recorder::new()?;
    let land = homeland("deleted-delivered-issue")?;
    let (fx, data) = host_at_home(land.home_url.clone(), Vec::new()).await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let _ = home_until(&mut link, |home| home.projects.len() == 2).await?;
    land.go_offline()?;
    let issue = create_offline_issue(&link).await?;
    let _ = home_until(&mut link, |home| home.unpushed == 2).await?;
    let blocked = data.join("home-edits.new");
    std::fs::create_dir(&blocked)?;
    land.go_online()?;
    link.accepted(CommandBody::SyncHome).await?;
    assert!(pushed_issue(&land, issue)?.contains(": offline note"));

    let writer = home_writer(&land)?;
    std::fs::remove_file(
        writer
            .join(ALPHA_ISSUES)
            .join(format!("{}.md", issue_name(issue))),
    )?;
    push_home_edit(&writer)?;
    let deleted = git(&land.home_bare, &["rev-parse", "HEAD"])?;
    std::fs::remove_dir(&blocked)?;
    link.accepted(CommandBody::SyncHome).await?;
    let _ = home_until(&mut link, |home| {
        home.unpushed == 0 && alpha_issues(home).is_empty()
    })
    .await?;
    assert_eq!(git(&land.home_bare, &["rev-parse", "HEAD"])?, deleted);
    link.close().await;
    fx.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

#[tokio::test]
async fn a_later_setter_is_delivered_after_an_older_uncommitted_setter()
-> Fallible {
    let mut feed = Recorder::new()?;
    let land = homeland("ordered-issue-setters")?;
    let (fx, data) = host_at_home(land.home_url.clone(), Vec::new()).await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let _ = home_until(&mut link, |home| home.projects.len() == 2).await?;
    let issue = opens_an_issue(&mut link, &land).await?;
    let before = git(&land.home_bare, &["rev-parse", "HEAD"])?;
    let checkout = data.join("home-repo");
    let lock = checkout.join(".git/index.lock");
    std::fs::write(&lock, "")?;
    let title = |text: &str| change(issue, retitle(text));
    wrote(&link, title("First setter")).await?;
    let _ = home_until(&mut link, |home| home.unpushed == 1).await?;
    std::fs::remove_file(&lock)?;
    git(
        &checkout,
        &[
            "checkout",
            "HEAD",
            "--",
            &format!("{ALPHA_ISSUES}/{}.md", issue_name(issue)),
        ],
    )?;

    wrote(&link, title("Second setter")).await?;
    let landed = home_until(&mut link, |home| home.unpushed == 0).await?;
    assert_eq!(
        alpha_issues(&landed)
            .first()
            .map(|shown| shown.title.as_str()),
        Some("Second setter")
    );
    assert!(pushed_issue(&land, issue)?.contains("title = \"Second setter\""));
    assert_eq!(
        git(
            &land.home_bare,
            &["rev-list", "--count", &format!("{}..HEAD", before.trim())],
        )?
        .trim(),
        "2"
    );
    link.close().await;
    fx.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

#[tokio::test]
async fn a_queue_temporarily_unreadable_at_startup_is_loaded_before_the_next_edit()
-> Fallible {
    let mut feed = Recorder::new()?;
    let land = homeland("queue-read-at-startup")?;
    let (fx, data) = host_at_home(land.home_url.clone(), Vec::new()).await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let _ = home_until(&mut link, |home| home.projects.len() == 2).await?;
    let issue = opens_an_issue(&mut link, &land).await?;
    land.go_offline()?;
    let note =
        |text: &str| change(issue, IssueChange::Note(String::from(text)));
    wrote(&link, note("first note")).await?;
    let _ = home_until(&mut link, |home| home.unpushed == 1).await?;
    link.close().await;
    fx.shutdown().await?;

    let state = data.join("home-edits.state");
    let kept = data.join("home-edits.saved");
    std::fs::rename(&state, &kept)?;
    std::fs::create_dir(&state)?;
    let reopened = serve(endpoint().await?, claude_plugins(), &data).await?;
    let mut link =
        RawClient::live(&reopened.target, "test-client", &mut feed).await?;
    link.accepted(CommandBody::SyncHome).await?;
    assert!(matches!(
        link.send(note("cannot save yet")).await?.value,
        CommandResult::Rejected(Rejection::IssueNotSaved { .. })
    ));
    std::fs::remove_dir(&state)?;
    std::fs::rename(&kept, &state)?;
    wrote(&link, note("second note")).await?;
    let _ = home_until(&mut link, |home| home.unpushed == 2).await?;
    land.go_online()?;
    link.accepted(CommandBody::SyncHome).await?;
    let _ = home_until(&mut link, |home| home.unpushed == 0).await?;
    let landed = pushed_issue(&land, issue)?;
    assert_eq!(landed.matches(": first note").count(), 1);
    assert_eq!(landed.matches(": second note").count(), 1);
    assert!(!landed.contains(": cannot save yet"));
    link.close().await;
    reopened.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

#[tokio::test]
async fn an_issue_edit_rejected_when_the_queue_cannot_be_saved_can_be_retried()
-> Fallible {
    let mut feed = Recorder::new()?;
    let land = homeland("issue-queue-write-failure")?;
    let (fx, data) = host_at_home(land.home_url.clone(), Vec::new()).await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let _ = home_until(&mut link, |home| home.projects.len() == 2).await?;
    let issue = opens_an_issue(&mut link, &land).await?;
    land.go_offline()?;
    let blocked = data.join("home-edits.new");
    std::fs::create_dir(&blocked)?;
    let phone = endpoint().await?;
    let connection = authenticated(&phone, &fx.target).await?;
    let note = proto::Command {
        id: proto::CommandId(80),
        body: change(issue, IssueChange::Note(String::from("retry me"))),
    };
    let invalid = proto::Command {
        id: proto::CommandId(81),
        body: change(issue, retitle("")),
    };
    let invalid_result = retry_command(&connection, &invalid).await?;
    assert_eq!(
        invalid_result,
        CommandResult::Rejected(Rejection::IssueRefused {
            reason: String::from("an issue needs a title")
        })
    );
    let before = git(&data.join("home-repo"), &["rev-parse", "HEAD"])?;
    let refused = retry_command(&connection, &note).await?;
    assert!(matches!(
        refused,
        CommandResult::Rejected(Rejection::IssueNotSaved { .. })
    ));
    link.accepted(CommandBody::SyncHome).await?;
    let still_open = home_until(&mut link, |home| {
        matches!(home.sync, proto::HomeSync::Behind { .. })
    })
    .await?;
    assert_eq!(still_open.unpushed, 0);
    assert!(alpha_issues(&still_open).iter().all(|it| it.log.len() == 1));
    assert_eq!(
        git(&data.join("home-repo"), &["rev-parse", "HEAD"])?,
        before
    );

    std::fs::remove_dir(&blocked)?;
    let reuse_invalid_id = proto::Command {
        id: invalid.id,
        body: note.body.clone(),
    };
    assert_eq!(
        retry_command(&connection, &reuse_invalid_id).await?,
        invalid_result
    );
    assert!(matches!(
        retry_command(&connection, &note).await?,
        CommandResult::IssueWritten { .. }
    ));
    let kept = home_until(&mut link, |home| home.unpushed == 1).await?;
    assert_issue_notes(&kept, issue, 2);
    link.close().await;
    fx.shutdown().await?;

    let reopened = serve(endpoint().await?, claude_plugins(), &data).await?;
    let mut link =
        RawClient::live(&reopened.target, "test-client", &mut feed).await?;
    let kept = home_until(&mut link, |home| home.unpushed == 1).await?;
    assert_issue_notes(&kept, issue, 2);
    wrote(
        &link,
        change(issue, IssueChange::Note(String::from("still offline"))),
    )
    .await?;
    let visible = home_until(&mut link, |home| home.unpushed == 2).await?;
    assert_issue_notes(&visible, issue, 3);
    land.go_online()?;
    link.accepted(CommandBody::SyncHome).await?;
    let _ = home_until(&mut link, |home| home.unpushed == 0).await?;
    let landed = pushed_issue(&land, issue)?;
    assert_eq!(landed.matches(": retry me").count(), 1);
    assert_eq!(landed.matches(": still offline").count(), 1);
    assert_eq!(
        git(
            &land.home_bare,
            &["rev-list", "--count", &format!("{}..HEAD", before.trim())]
        )?
        .trim(),
        "2"
    );
    link.close().await;
    reopened.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

/// The issue's log has `count` lines, the one opening it among them.
fn assert_issue_notes(home: &proto::Home, issue: proto::IssueId, count: usize) {
    assert_eq!(
        alpha_issues(home)
            .iter()
            .map(|it| (it.id, it.log.len()))
            .collect::<Vec<_>>(),
        [(issue, count)]
    );
}

#[tokio::test]
async fn an_issue_creation_with_a_failed_local_commit_lands_after_restart()
-> Fallible {
    let mut feed = Recorder::new()?;
    let land = homeland("issue-commit-write-failure")?;
    let (fx, data) = host_at_home(land.home_url.clone(), Vec::new()).await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let _ = home_until(&mut link, |home| home.projects.len() == 2).await?;
    land.go_offline()?;
    let lock = data.join("home-repo/.git/index.lock");
    std::fs::write(&lock, "")?;
    let issue =
        match link.send(create_issue("Commit after restart")).await?.value {
            CommandResult::IssueWritten { issue, .. } => issue,
            other => {
                return Err(
                    format!("expected a saved issue, got {other:?}").into()
                );
            }
        };
    let queued = home_until(&mut link, |home| home.unpushed == 1).await?;
    assert_eq!(
        alpha_issues(&queued).first().map(|shown| shown.id),
        Some(issue)
    );
    let checkout = data.join("home-repo");
    assert_eq!(git(&checkout, &["ls-files", "--", ALPHA_ISSUES])?, "");
    link.close().await;
    fx.shutdown().await?;
    std::fs::remove_file(&lock)?;

    let reopened = serve(endpoint().await?, claude_plugins(), &data).await?;
    let mut link =
        RawClient::live(&reopened.target, "test-client", &mut feed).await?;
    let _ = home_until(&mut link, |home| home.unpushed == 1).await?;
    land.go_online()?;
    link.accepted(CommandBody::SyncHome).await?;
    let landed = home_until(&mut link, |home| home.unpushed == 0).await?;
    assert_eq!(
        alpha_issues(&landed).first().map(|shown| shown.id),
        Some(issue)
    );
    assert!(
        pushed_issue(&land, issue)?
            .contains("title = \"Commit after restart\"")
    );
    link.close().await;
    reopened.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

#[tokio::test]
async fn a_create_without_a_receipt_recovers_after_the_issue_file_breaks()
-> Fallible {
    let mut feed = Recorder::new()?;
    let land = homeland("issue-create-receipt-gap")?;
    let (fx, data) = host_at_home(land.home_url.clone(), Vec::new()).await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let _ = home_until(&mut link, |home| home.projects.len() == 2).await?;
    let phone = endpoint().await?;
    let connection = authenticated(&phone, &fx.target).await?;
    let command = proto::Command {
        id: proto::CommandId(172),
        body: create_issue("Published before the receipt"),
    };
    let created = retry_command(&connection, &command).await?;
    let CommandResult::IssueWritten { issue, .. } = created else {
        return Err(format!("expected an issue, got {created:?}").into());
    };
    let _ = home_until(&mut link, |home| {
        home.unpushed == 0 && !alpha_issues(home).is_empty()
    })
    .await?;
    phone.close().await;
    link.close().await;
    fx.shutdown().await?;

    {
        let database = redb::Database::open(data.join("conductor.redb"))?;
        let write = database.begin_write()?;
        let receipts =
            redb::TableDefinition::<u128, Vec<u8>>::new("command_receipts");
        let _ = write.open_table(receipts)?.remove(command.id.0)?;
        write.commit()?;
    }
    let writer = home_writer(&land)?;
    let relative = format!("{ALPHA_ISSUES}/{}.md", issue_name(issue));
    std::fs::write(writer.join(relative), "Unparseable issue document")?;
    push_home_edit(&writer)?;

    let reopened = serve(endpoint().await?, claude_plugins(), &data).await?;
    let mut link =
        RawClient::live(&reopened.target, "test-client", &mut feed).await?;
    link.accepted(CommandBody::SyncHome).await?;
    let _ = home_until(&mut link, |home| {
        home.projects.len() == 2 && alpha_issues(home).is_empty()
    })
    .await?;
    let before = git(&land.home_bare, &["rev-parse", "HEAD"])?;
    let phone = endpoint().await?;
    let connection = authenticated(&phone, &reopened.target).await?;
    assert_eq!(retry_command(&connection, &command).await?, created);
    assert_eq!(git(&land.home_bare, &["rev-parse", "HEAD"])?, before);
    phone.close().await;
    link.close().await;
    reopened.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

#[tokio::test]
async fn a_queue_stored_in_an_older_shape_is_set_aside() -> Fallible {
    let mut feed = Recorder::new()?;
    let land = homeland("older-issue-queue")?;
    let (fx, data) = host_at_home(land.home_url.clone(), Vec::new()).await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let _ = home_until(&mut link, |home| home.projects.len() == 2).await?;
    link.close().await;
    fx.shutdown().await?;

    // A queue in the store's previous version.
    let older =
        b"\x80\x00\x03\x01\x01\x00\x05alpha\x2a\x01\x06legacy\x01\x08\x09identical\x00";
    let state = data.join("home-edits.state");
    std::fs::write(&state, older)?;
    let reopened = serve(endpoint().await?, claude_plugins(), &data).await?;
    let mut link =
        RawClient::live(&reopened.target, "test-client", &mut feed).await?;
    link.accepted(CommandBody::SyncHome).await?;
    let synced = home_until(&mut link, |home| {
        matches!(home.sync, proto::HomeSync::Synced)
    })
    .await?;
    assert_eq!(synced.unpushed, 0);
    assert_eq!(
        std::fs::read(data.join("home-edits.unreadable"))?,
        older.to_vec(),
        "the older queue is kept aside, unread"
    );
    link.close().await;
    reopened.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

#[tokio::test]
async fn an_offline_issue_change_matching_the_local_file_is_kept_for_replay()
-> Fallible {
    let mut feed = Recorder::new()?;
    let land = homeland("matching-offline-issue-change")?;
    let (fx, data) = host_at_home(land.home_url.clone(), Vec::new()).await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let _ = home_until(&mut link, |home| home.projects.len() == 2).await?;
    let issue = opens_an_issue(&mut link, &land).await?;

    let (second, _) = host_at_home(land.home_url.clone(), Vec::new()).await?;
    let mut writer =
        RawClient::live(&second.target, "test-client", &mut feed).await?;
    let _ =
        home_until(&mut writer, |home| !alpha_issues(home).is_empty()).await?;
    wrote(&writer, change(issue, retitle("Upstream title"))).await?;
    writer.close().await;
    second.shutdown().await?;
    assert!(pushed_issue(&land, issue)?.contains("title = \"Upstream title\""));

    land.go_offline()?;
    let phone = endpoint().await?;
    let conn = authenticated(&phone, &fx.target).await?;
    let matching = proto::Command {
        id: proto::CommandId(80),
        body: change(issue, retitle("Sign in")),
    };
    assert_eq!(
        retry_command(&conn, &matching).await?,
        CommandResult::IssueWritten {
            issue,
            warnings: Vec::new(),
        }
    );
    link.accepted(CommandBody::SyncHome).await?;
    let waiting = home_until(&mut link, |home| {
        matches!(home.sync, proto::HomeSync::Behind { .. })
    })
    .await?;
    assert_eq!(waiting.unpushed, 1);
    link.close().await;
    phone.close().await;
    fx.shutdown().await?;

    let reopened = serve(endpoint().await?, claude_plugins(), &data).await?;
    let mut link =
        RawClient::live(&reopened.target, "test-client", &mut feed).await?;
    let waiting = home_until(&mut link, |home| home.unpushed == 1).await?;
    assert_eq!(
        alpha_issues(&waiting)
            .first()
            .map(|shown| shown.title.as_str()),
        Some("Sign in")
    );
    let retry = endpoint().await?;
    let conn = authenticated(&retry, &reopened.target).await?;
    assert_eq!(
        retry_command(&conn, &matching).await?,
        CommandResult::IssueWritten {
            issue,
            warnings: Vec::new(),
        }
    );
    land.go_online()?;
    link.accepted(CommandBody::SyncHome).await?;
    let _ = home_until(&mut link, |home| home.unpushed == 0).await?;
    assert!(pushed_issue(&land, issue)?.contains("title = \"Sign in\""));
    retry.close().await;
    link.close().await;
    reopened.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

#[tokio::test]
async fn issues_are_written_to_the_home_and_replayed_over_a_concurrent_push()
-> Fallible {
    let mut feed = Recorder::new()?;
    let land = homeland("issues")?;
    let (fx, _) = host_at_home(land.home_url.clone(), Vec::new()).await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let _ = home_until(&mut link, |home| home.projects.len() == 2).await?;
    let issue = opens_an_issue(&mut link, &land).await?;
    noted_by_hand(&land, issue)?;
    wrote(
        &link,
        change(
            issue,
            IssueChange::Move {
                part: proto::Part::Status,
                to: String::from("active"),
                reason: String::new(),
                signed_off: false,
            },
        ),
    )
    .await?;
    let replayed = pushed_issue(&land, issue)?;
    assert!(
        replayed.contains("status = \"active\"")
            && replayed.contains("- by hand"),
        "the host's change lands on top of the person's: {replayed}"
    );
    assert_eq!(
        link.send(change(
            proto::IssueId::new(0, 7),
            IssueChange::Note(String::from("x"))
        ))
        .await?
        .value,
        CommandResult::Rejected(Rejection::UnknownIssue)
    );
    lands_an_offline_note(&mut link, &land, issue).await?;
    link.close().await;
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

async fn pause_first_fetch(
    checkout: &Path,
    script: &Path,
) -> Fallible<TcpListener> {
    use std::os::unix::fs::PermissionsExt;

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    std::fs::write(
        script,
        format!(
            r#"#!/bin/bash
if mkdir "$0.entered" 2>/dev/null; then
    exec 3<>/dev/tcp/127.0.0.1/{port} || exit 1
    read -r release <&3 || exit 1
    exec 3<&-
fi
exec git upload-pack "$@"
"#
        ),
    )?;
    std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o700))?;
    let path = script.to_str().ok_or("non-UTF-8 upload-pack path")?;
    let quoted = path.replace('\'', "'\"'\"'");
    git(
        checkout,
        &["config", "remote.origin.uploadpack", &format!("'{quoted}'")],
    )?;
    Ok(listener)
}

async fn competing_start_during_fetch(
    fx: &Fixture,
    listener: TcpListener,
    issue: proto::IssueId,
    feed: &mut Recorder,
) -> Fallible<CommandResult> {
    let (mut barrier, _) =
        tokio::time::timeout(STEP, listener.accept()).await??;
    let other = RawClient::live(&fx.target, "test-client", feed).await?;
    let release = async {
        barrier.write_all(b"continue\n").await?;
        Ok(())
    };
    let (competing, ()) =
        tokio::try_join!(other.send(on_issue(issue)), release)?;
    other.close().await;
    Ok(competing.value)
}

#[tokio::test]
async fn a_session_on_an_issue_works_on_its_branch_from_the_issues_brief()
-> Fallible {
    let mut feed = Recorder::new()?;
    let land = homeland("issue-sessions")?;
    let (fx, _) =
        host_at_home(land.home_url.clone(), Vec::from([land.roots.clone()]))
            .await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let _ =
        home_until(&mut link, |home| !checkouts_of(home, "alpha").is_empty())
            .await?;
    let issue = opens_an_issue(&mut link, &land).await?;
    let alpha = land.roots.join("apps").join("alpha");
    let listener =
        pause_first_fetch(&alpha, &land.scratch.join("upload-pack")).await?;
    let (session, competing) = tokio::try_join!(
        link.started(on_issue(issue)),
        competing_start_during_fetch(&fx, listener, issue, &mut feed),
    )?;
    assert_eq!(
        competing,
        CommandResult::Rejected(Rejection::IssueTaken { session })
    );
    let events = raw_events_until(&mut link, Until::Request).await?;
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
                && text.contains("## What")
                && *by == test_client()
        }),
        "the first message is the issue's brief: {brief:?}"
    );
    let branch = format!("issue/{}", issue_name(issue));
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
    let moved = home_until(&mut link, |home| {
        alpha_issues(home)
            .iter()
            .any(|shown| shown.status == "active")
    })
    .await?;
    assert_eq!(moved.unpushed, 0);
    link.close().await;
    fx.shutdown().await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

#[tokio::test]
async fn finishing_lands_work_and_notes_only_the_summary_before_review()
-> Fallible {
    let mut feed = Recorder::new()?;
    let land = homeland("finish")?;
    let alpha = land.roots.join("apps").join("alpha");
    std::fs::copy(
        recording_file("claude-worktrees.jsonl"),
        alpha.join("stdout.jsonl"),
    )?;
    let (fx, data) =
        host_at_home(land.home_url.clone(), Vec::from([land.roots.clone()]))
            .await?;
    let mut link =
        RawClient::live(&fx.target, "test-client", &mut feed).await?;
    let _ =
        home_until(&mut link, |home| !checkouts_of(home, "alpha").is_empty())
            .await?;
    let issue = opens_an_issue(&mut link, &land).await?;
    let session = link.started(on_issue(issue)).await?;
    let _ = raw_events_until(&mut link, Until::TurnEnd).await?;
    link.accepted(CommandBody::FinishSession { session })
        .await?;
    let finishing = of(
        session,
        &raw_events_until(&mut link, Until::Finished).await?,
    );
    let remote = git(&alpha, &["remote", "get-url", "origin"])?;
    assert_eq!(
        finishing.last(),
        Some(&EventKind::SessionFinished {
            finished: proto::Finished::Landed {
                commits: 2,
                kept: Kept::Pushed {
                    remote: String::from(remote.trim())
                },
            }
        })
    );
    assert!(matches!(finishing.first(), Some(EventKind::FinishStarted)));
    assert!(finishing.contains(&EventKind::SessionClosed {
        reason: CloseReason::Stopped {
            reason: String::from("the session was finished")
        }
    }));
    let branch = format!("issue/{}", issue_name(issue));
    let pushed = git(
        Path::new(remote.trim().trim_start_matches("file://")),
        &["show", &format!("{branch}:changed.txt")],
    )?;
    assert!(
        pushed.starts_with("turn two\n"),
        "what the worktree held when it finished landed and was pushed: {pushed}"
    );
    let reviewed = home_until(&mut link, |home| {
        alpha_issues(home)
            .iter()
            .any(|shown| shown.status == "review")
    })
    .await?;
    assert_summary_noted(&reviewed);
    assert_eq!(
        link.send(CommandBody::FinishSession { session })
            .await?
            .value,
        CommandResult::Rejected(Rejection::AlreadyFinished),
        "a finished session that did nothing since doesn't finish again"
    );
    assert_eq!(
        link.send(CommandBody::FinishSession {
            session: SessionId(1)
        })
        .await?
        .value,
        CommandResult::Rejected(Rejection::UnknownSession)
    );
    link.close().await;
    fx.shutdown().await?;
    assert_finish_recovery(&land, &data, issue, session, &mut feed).await?;
    let _ = std::fs::remove_dir_all(&land.scratch);
    Ok(())
}

/// The issue's last note before it moves to review is the session's
/// summary, and its checklist wrote none.
fn assert_summary_noted(reviewed: &proto::Home) {
    let notes = alpha_issues(reviewed)
        .into_iter()
        .flat_map(|shown| shown.log)
        .map(|entry| home::flow::file::log_line(&entry))
        .collect::<Vec<String>>();
    assert!(
        !notes.iter().any(|note| note.contains("Done: turn one.")),
        "checklist completion does not write an issue note: {notes:?}"
    );
    assert_eq!(
        notes
            .iter()
            .rev()
            .take(2)
            .map(|note| {
                note.strip_prefix("- ")
                    .and_then(|line| line.split_once(' '))
                    .map_or("", |(_, rest)| rest)
            })
            .collect::<Vec<&str>>(),
        [
            "test-host·claude status active→review: a session on it landed work",
            "test-host·claude note: Completed turn 2.",
        ],
        "the summary is the issue's last note before it moves to review: {notes:?}"
    );
}

async fn assert_finish_recovery(
    land: &Homeland,
    data: &Path,
    issue: IssueId,
    session: SessionId,
    feed: &mut Recorder,
) -> Fallible {
    let project = land.roots.join("apps/alpha");
    let branch = format!("refs/heads/issue/{}", issue_name(issue));
    let old_tip = git(&project, &["rev-parse", &branch])?;
    let receipt = git(
        &project,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            "refs/conductor-landed/",
        ],
    )?;
    assert_eq!(
        receipt, old_tip,
        "the original landing durably recorded its result"
    );
    let old_home = git(&land.home_bare, &["rev-parse", "HEAD"])?;
    let old_issue = pushed_issue(land, issue)?;
    disk::lose_terminal_finish(data, session)?;
    let reopened = serve(endpoint().await?, claude_plugins(), data).await?;
    let history = raw_history(&reopened.target, feed).await?;
    let recovered = of(session, &history);
    assert!(recovered.contains(&EventKind::FinishInterrupted));
    assert!(
        !recovered
            .iter()
            .any(|event| matches!(event, EventKind::SessionFinished { .. }))
    );

    let mut link =
        RawClient::live(&reopened.target, "test-client", feed).await?;
    let _ =
        home_until(&mut link, |home| !alpha_issues(home).is_empty()).await?;
    link.accepted(CommandBody::FinishSession { session })
        .await?;
    let events = raw_events_until(&mut link, Until::Finished).await?;
    assert!(of(session, &events).iter().any(|event| matches!(
        event,
        EventKind::SessionFinished {
            finished: proto::Finished::Landed { commits: 2, .. }
        }
    )));
    assert_eq!(git(&project, &["rev-parse", &branch])?, old_tip);
    assert_eq!(git(&land.home_bare, &["rev-parse", "HEAD"])?, old_home);
    assert_eq!(pushed_issue(land, issue)?, old_issue);
    assert_eq!(
        link.send(CommandBody::FinishSession { session })
            .await?
            .value,
        CommandResult::Rejected(Rejection::AlreadyFinished)
    );
    link.close().await;
    reopened.shutdown().await
}
