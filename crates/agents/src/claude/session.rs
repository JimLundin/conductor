//! One tokio task owns the `claude` child process. The handle talks to it over a
//! command channel; the task alone owns stdin, the pending control requests and
//! the open permission requests, so no state is shared behind a lock.

use std::collections::HashMap;
use std::convert::Infallible;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use proto::{ClaudeCredential, Effort, Models};
use serde_json::Value;

use crate::{Exit, Home};
use n0_future::StreamExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};

use super::protocol::{
    Applied, ClaudeEvent, ControlCall, Inbound, Open, Outbound, PermissionDecision, PermissionMode,
    PermissionRequestId, SessionId, UnsupportedVersion, applied, check_version, encode, models,
    parse_line,
};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSource {
    User,
    Project,
    Local,
}

impl SettingSource {
    fn wire(&self) -> &str {
        match self {
            SettingSource::User => "user",
            SettingSource::Project => "project",
            SettingSource::Local => "local",
        }
    }
}

/// Which CLI versions a session agrees to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionPolicy {
    /// Refuse a CLI outside the supported range.
    SupportedOnly,
    /// Run it anyway, with a warning.
    AllowUntested,
}

#[derive(Debug, Clone)]
pub struct ClaudeOptions {
    pub claude_path: PathBuf,
    pub home: Home,
    /// Passed in the environment; `None` leaves the home's own sign-in.
    pub credential: Option<ClaudeCredential>,
    pub cwd: PathBuf,
    pub model: Option<String>,
    /// `None` leaves the model's default effort.
    pub effort: Option<Effort>,
    pub permission_mode: PermissionMode,
    pub resume: Option<SessionId>,
    /// `None` leaves the CLI default, which loads user, project and local settings.
    pub setting_sources: Option<Vec<SettingSource>>,
    /// Append every raw stdout line to this file.
    pub stdout_log: Option<PathBuf>,
    pub versions: VersionPolicy,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not build the claude command: {0}")]
    Command(String),
    #[error("claude i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not encode a message for claude: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("the claude process has exited")]
    Exited,
    /// The CLI exited before it answered `initialize`, saying why.
    #[error("claude exited: {0}")]
    Refused(String),
    #[error("claude rejected {subtype}: {message}")]
    Control { subtype: String, message: String },
    #[error("claude did not answer {0} within {CONTROL_TIMEOUT:?}")]
    Timeout(String),
    #[error(
        "claude {found} is not supported (supported: {supported}); set VersionPolicy::AllowUntested to run it anyway"
    )]
    UnsupportedVersion { found: String, supported: String },
    #[error(
        "claude did not report its version, so it can't be checked; set VersionPolicy::AllowUntested to run it anyway"
    )]
    UnreportedVersion,
    #[error("no open permission request with id {}", .0.0)]
    UnknownPermissionRequest(PermissionRequestId),
}

enum CommandMsg {
    SendUser {
        text: String,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    Control {
        call: ControlCall,
        reply: oneshot::Sender<Result<Value, Error>>,
    },
    Respond {
        id: PermissionRequestId,
        decision: PermissionDecision,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    Shutdown {
        reply: oneshot::Sender<Exit>,
    },
}

pub struct ClaudeSession {
    commands: mpsc::Sender<CommandMsg>,
    id: SessionId,
    /// What the `initialize` response lists.
    models: Models,
    pid: Option<u32>,
}

impl ClaudeSession {
    /// Starts `claude` and completes the `initialize` handshake. Refuses a CLI
    /// version outside the supported range unless `VersionPolicy::AllowUntested`.
    pub async fn spawn(
        opts: ClaudeOptions,
    ) -> Result<(ClaudeSession, mpsc::Receiver<ClaudeEvent>), Error> {
        let stdout_log = opts
            .stdout_log
            .as_ref()
            .map(|path| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
            })
            .transpose()?;
        // A fresh session gets its id up front; `--resume` keeps the one it has.
        let id = opts
            .resume
            .clone()
            .unwrap_or_else(|| SessionId(uuid::Uuid::new_v4().to_string()));
        let mut child = command(&opts, &id).spawn()?;
        let pid = child.id();
        let ((stdin, stdout), stderr) = child
            .stdin
            .take()
            .zip(child.stdout.take())
            .zip(child.stderr.take())
            .ok_or(Error::Command(String::from("claude stdio was not piped")))?;
        let last_stderr = tokio::spawn(log_stderr(stderr));

        let (events_tx, mut events_rx) = mpsc::channel(1024);
        let (commands_tx, commands_rx) = mpsc::channel(32);
        let actor = Actor {
            process: Process::Running(Box::new(Running {
                child,
                stdin,
                stdout: BufReader::new(stdout).lines(),
            })),
            events: events_tx,
            stdout_log,
            pending: HashMap::new(),
            open_permissions: HashMap::new(),
            next_request: 0,
        };
        tokio::spawn(actor.run(commands_rx));

        let session = ClaudeSession {
            commands: commands_tx,
            id,
            models: Models::none(),
            pid,
        };
        let initialize = match session.control(ControlCall::Initialize).await {
            Err(Error::Exited) => Err(Error::Refused(refusal(&mut events_rx, last_stderr).await)),
            answer => answer,
        }?;
        let session = ClaudeSession {
            models: models(&initialize),
            ..session
        };
        match (check_version(initialize), opts.versions) {
            (Ok(()), _) => Ok((session, events_rx)),
            (
                Err(UnsupportedVersion::Outside { found, supported }),
                VersionPolicy::AllowUntested,
            ) => {
                tracing::warn!("running untested claude {found} (supported: {supported})");
                Ok((session, events_rx))
            }
            (Err(UnsupportedVersion::Unreported), VersionPolicy::AllowUntested) => {
                tracing::warn!("running a claude that did not report its version");
                Ok((session, events_rx))
            }
            (
                Err(UnsupportedVersion::Outside { found, supported }),
                VersionPolicy::SupportedOnly,
            ) => {
                session.shutdown().await?;
                Err(Error::UnsupportedVersion { found, supported })
            }
            (Err(UnsupportedVersion::Unreported), VersionPolicy::SupportedOnly) => {
                session.shutdown().await?;
                Err(Error::UnreportedVersion)
            }
        }
    }

    /// The models and efforts this CLI offers.
    pub fn models(&self) -> &Models {
        &self.models
    }

    /// The id `--resume` takes, which the CLI prints in every `init`.
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    /// The CLI's process id, while it runs.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// The model and effort the CLI runs with (`get_settings`).
    pub async fn applied(&self) -> Result<Applied, Error> {
        Ok(applied(self.control(ControlCall::GetSettings).await?)?)
    }

    pub async fn send_user(&self, text: String) -> Result<(), Error> {
        self.ask(|reply| CommandMsg::SendUser { text, reply })
            .await?
    }

    /// Resolves once the CLI acknowledges; the turn's `result` follows as an event.
    pub async fn interrupt(&self) -> Result<(), Error> {
        let _ = self.control(ControlCall::Interrupt).await?;
        Ok(())
    }

    pub async fn set_permission_mode(&self, mode: PermissionMode) -> Result<(), Error> {
        let _ = self.control(ControlCall::SetPermissionMode(mode)).await?;
        Ok(())
    }

    pub async fn respond(
        &self,
        id: PermissionRequestId,
        decision: PermissionDecision,
    ) -> Result<(), Error> {
        self.ask(|reply| CommandMsg::Respond {
            id,
            decision,
            reply,
        })
        .await?
    }

    /// Closes stdin, waits briefly for a clean exit, then kills.
    pub async fn shutdown(self) -> Result<Exit, Error> {
        self.ask(|reply| CommandMsg::Shutdown { reply }).await
    }

    async fn control(&self, call: ControlCall) -> Result<Value, Error> {
        tokio::time::timeout(
            CONTROL_TIMEOUT,
            self.ask(|reply| CommandMsg::Control { call, reply }),
        )
        .await
        .map_err(|_| Error::Timeout(String::from(call.subtype())))??
    }

    async fn ask<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> CommandMsg,
    ) -> Result<T, Error> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(build(reply))
            .await
            .map_err(|_| Error::Exited)?;
        answer.await.map_err(|_| Error::Exited)
    }
}

/// The variables a parent Claude Code session sets. They are removed so the CLI
/// starts as its own session even when Conductor runs inside one.
const PARENT_SESSION_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
];

/// What the host's own environment may carry that would take a private home
/// off its own sign-in: credentials, which take precedence over it, the
/// cloud providers and another endpoint, and another home.
const INHERITED_LOGIN: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CONFIG_DIR",
];

/// `claude` with what every run of it gets: its home, and its own updates
/// off, since the host installs the version it runs.
pub fn cli(claude_path: &Path, home: &Home) -> Command {
    let mut cmd = Command::new(claude_path);
    cmd.env("DISABLE_UPDATES", "1");
    if let Home::Private(dir) = home {
        for var in INHERITED_LOGIN {
            cmd.env_remove(var);
        }
        cmd.env("CLAUDE_CONFIG_DIR", dir);
    }
    for var in PARENT_SESSION_VARS {
        cmd.env_remove(var);
    }
    cmd
}

/// Whether Claude's command sandbox can run on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sandbox {
    Available,
    Missing,
}

/// What Claude's sandbox needs on Linux and WSL2.
const SANDBOX_TOOLS: &[&str] = &["bwrap", "socat"];

/// Whether Claude's command sandbox can run here ([sandboxing] in
/// docs/architecture.md): Seatbelt on macOS always, on Linux and WSL2 with
/// `bwrap` and `socat` on `PATH`, and never on native Windows, where Claude
/// runs commands unsandboxed.
pub fn sandbox() -> Sandbox {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::env::split_paths(&path).collect::<Vec<PathBuf>>();
    let missing = SANDBOX_TOOLS
        .iter()
        .find(|tool| !dirs.iter().any(|dir| dir.join(tool).is_file()));
    match (std::env::consts::OS, missing) {
        ("macos", _) | ("linux", None) => Sandbox::Available,
        (_, _) => Sandbox::Missing,
    }
}

/// `claude auth status`, which succeeds when `home` is signed in.
pub fn auth_status(claude_path: &Path, home: &Home) -> Command {
    let mut cmd = cli(claude_path, home);
    cmd.args(["auth", "status"]);
    cmd
}

/// The subscription sign-in, `claude auth login --claudeai`, into `home`. It
/// stores the credential there itself.
pub fn login(claude_path: &Path, home: &Home) -> Command {
    let mut cmd = cli(claude_path, home);
    cmd.args(["auth", "login", "--claudeai"]);
    cmd
}

pub(crate) fn command(opts: &ClaudeOptions, id: &SessionId) -> Command {
    let mut cmd = cli(&opts.claude_path, &opts.home);
    match &opts.credential {
        Some(ClaudeCredential::Gateway { base_url, token }) => {
            cmd.env("ANTHROPIC_BASE_URL", base_url)
                .env("ANTHROPIC_AUTH_TOKEN", token);
        }
        None => {}
    }
    cmd.args([
        "--print",
        "--verbose",
        "--output-format",
        "stream-json",
        "--input-format",
        "stream-json",
        "--permission-mode",
        opts.permission_mode.wire(),
    ]);
    // The scrub keeps credentials out of the commands, hooks and MCP servers
    // Claude starts, and Claude itself still uses them. On Linux it runs
    // every command in Claude's sandbox, so it is set only where that runs.
    match (&opts.credential, sandbox()) {
        (Some(ClaudeCredential::Gateway { .. }), Sandbox::Available) => {
            cmd.env("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB", "1");
        }
        (Some(ClaudeCredential::Gateway { .. }), Sandbox::Missing) | (None, _) => {}
    }
    // One argument, so an id that starts with `-` is not read as a flag.
    if let Some(id) = &opts.resume {
        cmd.arg(format!("--resume={}", id.0));
    }
    if let Some(model) = &opts.model {
        cmd.args(["--model", model]);
    }
    if let Some(effort) = &opts.effort {
        cmd.args(["--effort", effort.wire()]);
    }
    if let Some(sources) = &opts.setting_sources {
        cmd.args([
            "--setting-sources",
            &sources
                .iter()
                .map(SettingSource::wire)
                .collect::<Vec<&str>>()
                .join(","),
        ]);
    }
    // A hidden flag. The text arrives only where the CLI also sends
    // experimental betas, which `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`
    // turns off (PROTOCOL.md).
    cmd.args([
        "--include-partial-messages",
        "--thinking-display",
        "summarized",
        "--permission-prompt-tool",
        "stdio",
    ]);
    cmd.args(match &opts.resume {
        Some(_) => Vec::new(),
        None => Vec::from([String::from("--session-id"), id.0.clone()]),
    });
    cmd.current_dir(&opts.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::process::no_window(cmd)
}

/// Logs each stderr line, and returns the last once stderr closes.
async fn log_stderr(stderr: ChildStderr) -> Option<String> {
    n0_future::stream::unfold(BufReader::new(stderr).lines(), |mut lines| async move {
        lines
            .next_line()
            .await
            .ok()
            .flatten()
            .map(|line| (line, lines))
    })
    .inspect(|line| tracing::warn!(target: "claude::stderr", "{line}"))
    .last()
    .await
}

/// Why a CLI exited before it answered `initialize`: the `errors` of the
/// `result` it printed, else its last stderr line.
async fn refusal(
    events: &mut mpsc::Receiver<ClaudeEvent>,
    last_stderr: tokio::task::JoinHandle<Option<String>>,
) -> String {
    let errors = Box::pin(n0_future::stream::unfold(events, |events| async move {
        events.try_recv().ok().map(|event| (event, events))
    }))
    .filter_map(|event| match event {
        ClaudeEvent::TurnFinished { errors, .. } => errors.first().map(|_| errors.join("; ")),
        _ => None,
    })
    .next()
    .await;
    match errors {
        Some(errors) => errors,
        None => stderr_reason(last_stderr).await,
    }
}

async fn stderr_reason(last_stderr: tokio::task::JoinHandle<Option<String>>) -> String {
    tokio::time::timeout(CONTROL_TIMEOUT, last_stderr)
        .await
        .ok()
        .and_then(Result::ok)
        .flatten()
        .unwrap_or_else(|| String::from("it printed no reason"))
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
}

impl Running {
    async fn stop(self) -> Exit {
        let Running {
            child,
            stdin,
            stdout: _,
        } = self;
        crate::process::stop(child, stdin).await
    }
}

enum Process {
    Running(Box<Running>),
    Exited(Exit),
}

/// One thing the actor reacts to.
enum Input {
    Line(std::io::Result<Option<String>>),
    Command(CommandMsg),
    /// Every handle is gone.
    Closed,
}

/// Every handle is gone, so the actor has stopped the process and ended.
struct Stopped {}

/// A control request we sent, waiting for its `control_response`.
struct Pending {
    call: ControlCall,
    reply: oneshot::Sender<Result<Value, Error>>,
}

struct Actor {
    process: Process,
    events: mpsc::Sender<ClaudeEvent>,
    stdout_log: Option<std::fs::File>,
    /// Our control requests awaiting a `control_response`, by request_id.
    pending: HashMap<String, Pending>,
    /// `can_use_tool` requests not yet answered, with what an answer needs.
    open_permissions: HashMap<PermissionRequestId, Open>,
    next_request: u64,
}

impl Actor {
    async fn run(
        mut self,
        mut commands: mpsc::Receiver<CommandMsg>,
    ) -> Result<Infallible, Stopped> {
        loop {
            let input = self.next_input(&mut commands).await;
            self.on_input(input).await?;
        }
    }

    async fn next_input(&mut self, commands: &mut mpsc::Receiver<CommandMsg>) -> Input {
        match &mut self.process {
            Process::Running(running) => tokio::select! {
                line = running.stdout.next_line() => Input::Line(line),
                cmd = commands.recv() => cmd.map_or(Input::Closed, Input::Command),
            },
            Process::Exited(_) => commands.recv().await.map_or(Input::Closed, Input::Command),
        }
    }

    async fn on_input(&mut self, input: Input) -> Result<(), Stopped> {
        match input {
            Input::Line(Ok(Some(line))) => {
                self.on_line(&line).await;
                Ok(())
            }
            Input::Line(Ok(None)) => {
                self.stop().await;
                Ok(())
            }
            Input::Line(Err(e)) => {
                tracing::error!("reading claude stdout: {e}");
                self.stop().await;
                Ok(())
            }
            Input::Command(cmd) => {
                self.on_command(cmd).await;
                Ok(())
            }
            Input::Closed => {
                self.stop().await;
                Err(Stopped {})
            }
        }
    }

    async fn on_line(&mut self, line: &str) {
        if let Some(Err(e)) = self.stdout_log.as_mut().map(|log| writeln!(log, "{line}")) {
            tracing::warn!("writing stdout log: {e}");
        }
        let inbound = match parse_line(line) {
            Ok(inbound) => inbound,
            Err(e) => {
                tracing::warn!("claude printed a non-JSON line ({e}): {line}");
                Inbound::Event(ClaudeEvent::Unknown(Value::String(line.to_owned())))
            }
        };
        match inbound {
            Inbound::Event(event) => {
                if let ClaudeEvent::PermissionCancelled { id } = &event {
                    self.open_permissions.remove(id);
                }
                self.emit(event).await;
            }
            Inbound::Permission { id, open, event } => {
                self.open_permissions.insert(id, open);
                self.emit(event).await;
            }
            Inbound::ControlResponse {
                request_id,
                outcome,
            } => match self.pending.remove(&request_id) {
                Some(Pending { call, reply }) => {
                    let _ = reply.send(outcome.map_err(|message| Error::Control {
                        subtype: String::from(call.subtype()),
                        message,
                    }));
                }
                None => tracing::warn!("control_response for unknown request {request_id}"),
            },
            Inbound::UnsupportedControlRequest {
                request_id,
                subtype,
            } => {
                let message = format!("unsupported control request subtype: {subtype}");
                tracing::warn!("{message}");
                if let Err(e) = self
                    .write(Outbound::ControlError {
                        request_id,
                        message,
                    })
                    .await
                {
                    tracing::error!("replying to {subtype}: {e}");
                }
            }
        }
    }

    async fn on_command(&mut self, cmd: CommandMsg) {
        match cmd {
            CommandMsg::SendUser { text, reply } => {
                let _ = reply.send(self.write(Outbound::User(text)).await);
            }
            CommandMsg::Control { call, reply } => {
                self.next_request = self.next_request.saturating_add(1);
                let request_id = format!("req_{}", self.next_request);
                match self
                    .write(Outbound::Control {
                        request_id: request_id.clone(),
                        call,
                    })
                    .await
                {
                    Ok(()) => {
                        self.pending.insert(request_id, Pending { call, reply });
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                }
            }
            CommandMsg::Respond {
                id,
                decision,
                reply,
            } => {
                let result = match self.open_permissions.remove(&id) {
                    None => Err(Error::UnknownPermissionRequest(id)),
                    Some(open) => {
                        self.write(Outbound::Permission { id, decision, open })
                            .await
                    }
                };
                let _ = reply.send(result);
            }
            CommandMsg::Shutdown { reply } => {
                let _ = reply.send(self.stop().await);
            }
        }
    }

    async fn write(&mut self, msg: Outbound) -> Result<(), Error> {
        match &mut self.process {
            Process::Running(running) => {
                let line = format!("{}\n", encode(msg)?);
                running.stdin.write_all(line.as_bytes()).await?;
                running.stdin.flush().await?;
                Ok(())
            }
            Process::Exited(_) => Err(Error::Exited),
        }
    }

    /// Moves to `Exited`, failing everything still waiting on the process.
    async fn stop(&mut self) -> Exit {
        // Both arms put the real state back.
        let placeholder = Process::Exited(Exit::Unknown(String::from("stopping")));
        match std::mem::replace(&mut self.process, placeholder) {
            Process::Running(running) => {
                let status = running.stop().await;
                self.process = Process::Exited(status.clone());
                for (_, Pending { reply, .. }) in self.pending.drain() {
                    let _ = reply.send(Err(Error::Exited));
                }
                self.open_permissions.clear();
                self.emit(ClaudeEvent::Exited {
                    status: status.clone(),
                })
                .await;
                status
            }
            Process::Exited(status) => {
                self.process = Process::Exited(status.clone());
                status
            }
        }
    }

    async fn emit(&self, event: ClaudeEvent) {
        // A dropped receiver only means nobody is listening any more.
        let _ = self.events.send(event).await;
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use proto::Effort;

    use super::{ClaudeOptions, PermissionMode, SessionId, SettingSource, VersionPolicy, command};
    use crate::Home;

    #[test]
    fn resumed_session_argv() {
        let cmd = command(
            &ClaudeOptions {
                claude_path: "/opt/claude".into(),
                home: Home::Private(PathBuf::from("/srv/conductor/home/claude")),
                credential: None,
                cwd: "/tmp".into(),
                model: Some(String::from("haiku")),
                effort: Some(Effort::XHigh),
                permission_mode: PermissionMode::Default,
                resume: Some(SessionId(String::from("0b1c"))),
                setting_sources: Some(Vec::from([SettingSource::Project, SettingSource::Local])),
                stdout_log: None,
                versions: VersionPolicy::SupportedOnly,
            },
            &SessionId(String::from("0b1c")),
        );
        let std = cmd.as_std();
        let args = std
            .get_args()
            .map(|a| a.to_str().unwrap())
            .collect::<Vec<&str>>();
        assert_eq!(std.get_program(), "/opt/claude");
        assert_eq!(std.get_current_dir(), Some(Path::new("/tmp")));
        assert_eq!(
            args,
            [
                "--print",
                "--verbose",
                "--output-format",
                "stream-json",
                "--input-format",
                "stream-json",
                "--permission-mode",
                "default",
                "--resume=0b1c",
                "--model",
                "haiku",
                "--effort",
                "xhigh",
                "--setting-sources",
                "project,local",
                "--include-partial-messages",
                "--thinking-display",
                "summarized",
                "--permission-prompt-tool",
                "stdio",
            ]
        );
        let env = std
            .get_envs()
            .filter_map(|(key, value)| match (key.to_str(), value) {
                (
                    Some(
                        "CLAUDECODE"
                        | "CLAUDE_CODE_OAUTH_TOKEN"
                        | "CLAUDE_CODE_USE_BEDROCK"
                        | "CLAUDE_CODE_USE_VERTEX"
                        | "CLAUDE_CODE_USE_FOUNDRY"
                        | "ANTHROPIC_BASE_URL",
                    ),
                    None,
                ) => Some((key, None)),
                (Some("CLAUDE_CODE_SESSION_ID"), None) => Some((key, None)),
                (Some("CLAUDE_CONFIG_DIR" | "DISABLE_UPDATES"), Some(value)) => {
                    Some((key, value.to_str()))
                }
                _ => None,
            })
            .map(|(key, value)| (key.to_str().unwrap(), value))
            .collect::<Vec<(&str, Option<&str>)>>();
        assert_eq!(
            env,
            [
                ("ANTHROPIC_BASE_URL", None),
                ("CLAUDECODE", None),
                ("CLAUDE_CODE_OAUTH_TOKEN", None),
                ("CLAUDE_CODE_SESSION_ID", None),
                ("CLAUDE_CODE_USE_BEDROCK", None),
                ("CLAUDE_CODE_USE_FOUNDRY", None),
                ("CLAUDE_CODE_USE_VERTEX", None),
                ("CLAUDE_CONFIG_DIR", Some("/srv/conductor/home/claude")),
                ("DISABLE_UPDATES", Some("1")),
            ]
        );
    }
}
