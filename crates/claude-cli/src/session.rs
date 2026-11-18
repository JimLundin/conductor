//! One tokio task owns the `claude` child process. The handle talks to it over a
//! command channel; the task alone owns stdin and the session's state
//! machine ([`claude::Session`]), so no state is shared behind a lock. The
//! task turns what happens into the machine's inputs and performs its
//! effects.

use std::convert::Infallible;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use claude::ClaudeCredential;
use proto::{Effort, Models};
use serde_json::Value;

use agents::Exit;
use install::Home;
use n0_future::StreamExt;
use std::future::pending;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};

use claude::{
    After, Applied, ClaudeEvent, ControlCall, Failure, Launch,
    PermissionDecision, PermissionMode, PermissionRequestId, Resume, Sandbox,
    Session, SessionId, SettingSource, Then, UnsupportedVersion, VersionPolicy,
    applied, models,
};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(60);

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
    pub resume: Resume,
    /// Conductor's stable session ULID isolates its transcript directory.
    pub project_name: Option<String>,
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
    /// Only the CLI's exact missing-transcript reason for the requested resume id.
    #[error("claude exited: No conversation found with session ID: {}", .0.0)]
    MissingSession(SessionId),
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

impl From<Failure<std::io::Error>> for Error {
    fn from(failure: Failure<std::io::Error>) -> Self {
        match failure {
            Failure::Exited => Self::Exited,
            Failure::Io(error) => Self::Io(error),
            Failure::Encode(error) => Self::Encode(error),
            Failure::Control { subtype, message } => {
                Self::Control { subtype, message }
            }
            Failure::UnknownPermissionRequest(id) => {
                Self::UnknownPermissionRequest(id)
            }
        }
    }
}

impl From<UnsupportedVersion> for Error {
    fn from(unsupported: UnsupportedVersion) -> Self {
        match unsupported {
            UnsupportedVersion::Outside { found, supported } => {
                Self::UnsupportedVersion { found, supported }
            }
            UnsupportedVersion::Unreported => Self::UnreportedVersion,
        }
    }
}

/// Who waits for a command's answer.
type Reply = oneshot::Sender<Result<Value, Error>>;
/// Who waits for a shutdown's exit.
type Waiter = oneshot::Sender<Exit>;
type Input = claude::Input<Reply, Waiter, std::io::Error>;
type Effect = claude::Effect<Reply, Waiter, std::io::Error>;
type CommandMsg = claude::Command<Reply, Waiter>;

pub struct ClaudeSession {
    commands: mpsc::Sender<CommandMsg>,
    id: SessionId,
    /// What the `initialize` response lists.
    models: Models,
    pid: Option<u32>,
}

/// A session whose process runs, before it has answered `initialize`.
struct Starting {
    session: ClaudeSession,
    events: mpsc::Receiver<ClaudeEvent>,
    last_stderr: tokio::task::JoinHandle<Option<String>>,
}

impl ClaudeSession {
    /// Starts `claude` and completes the `initialize` handshake. Refuses a CLI
    /// version outside the supported range unless `VersionPolicy::AllowUntested`.
    ///
    /// # Errors
    /// Fails when the CLI cannot be started, exits or refuses before it
    /// answers `initialize`, or reports a version the policy refuses.
    pub async fn spawn(
        opts: ClaudeOptions,
    ) -> Result<(Self, mpsc::Receiver<ClaudeEvent>), Error> {
        let id = opts
            .resume
            .target()
            .cloned()
            .unwrap_or_else(|| SessionId(uuid::Uuid::new_v4().to_string()));
        let mut starting = start(&opts, id)?;
        let initialize = starting.initialize(&opts.resume).await?;
        starting.admit(initialize, opts.versions).await
    }

    /// The models and efforts this CLI offers.
    #[must_use]
    pub const fn models(&self) -> &Models {
        &self.models
    }

    /// The id `--resume` takes, which the CLI prints in every `init`.
    #[must_use]
    pub const fn id(&self) -> &SessionId {
        &self.id
    }

    /// The CLI's process id, while it runs.
    #[must_use]
    pub const fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// The model and effort the CLI runs with (`get_settings`).
    ///
    /// # Errors
    /// Fails when the CLI has exited, does not answer in time or rejects
    /// the request.
    pub async fn applied(&self) -> Result<Applied, Error> {
        Ok(applied(self.control(ControlCall::GetSettings).await?)?)
    }

    /// Changes the effort of the CLI's later API requests, in this process
    /// (`apply_flag_settings`); `applied` reports it back.
    ///
    /// # Errors
    /// Fails when the CLI has exited, does not answer in time or rejects
    /// the request.
    pub async fn set_effort(&self, effort: Effort) -> Result<(), Error> {
        let _ = self.control(ControlCall::SetEffort(effort)).await?;
        Ok(())
    }

    /// # Errors
    /// Fails when the CLI has exited or the message cannot be written.
    pub async fn send_user(&self, text: String) -> Result<(), Error> {
        let _ = self
            .ask(|reply| CommandMsg::SendUser { text, reply })
            .await??;
        Ok(())
    }

    /// Resolves once the CLI acknowledges; the turn's `result` follows as an event.
    ///
    /// # Errors
    /// Fails when the CLI has exited, does not answer in time or rejects
    /// the request.
    pub async fn interrupt(&self) -> Result<(), Error> {
        let _ = self.control(ControlCall::Interrupt).await?;
        Ok(())
    }

    /// # Errors
    /// Fails when the CLI has exited, does not answer in time or rejects
    /// the mode.
    pub async fn set_permission_mode(
        &self,
        mode: PermissionMode,
    ) -> Result<(), Error> {
        let _ = self.control(ControlCall::SetPermissionMode(mode)).await?;
        Ok(())
    }

    /// # Errors
    /// Fails when the CLI has exited, no request with `id` is open, or the
    /// answer cannot be written.
    pub async fn respond(
        &self,
        id: PermissionRequestId,
        decision: PermissionDecision,
    ) -> Result<(), Error> {
        let _ = self
            .ask(|reply| CommandMsg::Respond {
                id,
                decision,
                reply,
            })
            .await??;
        Ok(())
    }

    /// Closes stdin, waits briefly for a clean exit, then kills.
    ///
    /// # Errors
    /// Fails when the session's actor has already stopped.
    pub async fn shutdown(self) -> Result<Exit, Error> {
        self.ask(CommandMsg::Shutdown).await
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

/// Starts the process and its actor.
fn start(opts: &ClaudeOptions, id: SessionId) -> Result<Starting, Error> {
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
    let mut child = command(opts, &id).spawn()?;
    let pid = child.id();
    let ((stdin, stdout), stderr) = child
        .stdin
        .take()
        .zip(child.stdout.take())
        .zip(child.stderr.take())
        .ok_or_else(|| {
            Error::Command(String::from("claude stdio was not piped"))
        })?;
    let (events_tx, events) = mpsc::channel(1024);
    let (commands_tx, commands) = mpsc::channel(32);
    let actor = Actor {
        session: Session::running(),
        process: Some(Running {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            stdout_log,
        }),
        events: events_tx,
    };
    tokio::spawn(actor.run(commands));
    Ok(Starting {
        session: ClaudeSession {
            commands: commands_tx,
            id,
            models: Models::none(),
            pid,
        },
        events,
        last_stderr: tokio::spawn(log_stderr(stderr)),
    })
}

impl Starting {
    /// The `initialize` answer, or why the CLI exited before it gave one.
    async fn initialize(&mut self, resume: &Resume) -> Result<Value, Error> {
        let answer = self.session.control(ControlCall::Initialize).await;
        self.unless_exited(answer, resume).await
    }

    async fn unless_exited(
        &mut self,
        answer: Result<Value, Error>,
        resume: &Resume,
    ) -> Result<Value, Error> {
        match answer {
            Err(Error::Exited) => Err(self.refusal(resume).await),
            received => received,
        }
    }

    /// Why a CLI exited before it answered `initialize`: the `errors` of the
    /// `result` it printed, else its last stderr line.
    async fn refusal(&mut self, resume: &Resume) -> Error {
        let events = std::iter::from_fn(|| self.events.try_recv().ok())
            .collect::<Vec<ClaudeEvent>>();
        let reason =
            reason(claude::reported_refusal(events), &mut self.last_stderr)
                .await;
        resume
            .missing(&reason)
            .map_or(Error::Refused(reason), Error::MissingSession)
    }

    async fn admit(
        self,
        initialize: Value,
        policy: VersionPolicy,
    ) -> Result<(ClaudeSession, mpsc::Receiver<ClaudeEvent>), Error> {
        let session = ClaudeSession {
            models: models(&initialize),
            ..self.session
        };
        match claude::admit(initialize, policy) {
            Ok(warning) => {
                warning.iter().for_each(runtime::log);
                Ok((session, self.events))
            }
            Err(unsupported) => Err(session.refuse(unsupported).await),
        }
    }
}

impl ClaudeSession {
    async fn refuse(self, unsupported: UnsupportedVersion) -> Error {
        self.shutdown()
            .await
            .err()
            .unwrap_or_else(|| Error::from(unsupported))
    }
}

async fn reason(
    reported: Option<String>,
    last_stderr: &mut tokio::task::JoinHandle<Option<String>>,
) -> String {
    match reported {
        Some(reason) => reason,
        None => stderr_reason(last_stderr).await,
    }
}

async fn stderr_reason(
    last_stderr: &mut tokio::task::JoinHandle<Option<String>>,
) -> String {
    tokio::time::timeout(CONTROL_TIMEOUT, last_stderr)
        .await
        .ok()
        .and_then(Result::ok)
        .flatten()
        .unwrap_or_else(|| String::from("it printed no reason"))
}

/// Whether Claude's command sandbox can run here (`claude::sandbox`).
#[must_use]
pub fn sandbox() -> Sandbox {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::env::split_paths(&path).collect::<Vec<PathBuf>>();
    let tools = claude::SANDBOX_TOOLS
        .iter()
        .all(|tool| dirs.iter().any(|dir| dir.join(tool).is_file()));
    claude::sandbox(std::env::consts::OS, tools)
}

fn command(opts: &ClaudeOptions, id: &SessionId) -> Command {
    let launch = Launch {
        home: opts.home.kind(),
        credential: opts.credential.clone(),
        project_name: opts.project_name.clone(),
        permission_mode: opts.permission_mode,
        resume: opts.resume.clone(),
        id: id.clone(),
        model: opts.model.clone(),
        effort: opts.effort,
        setting_sources: opts.setting_sources.clone(),
        sandbox: sandbox(),
    };
    let mut cmd = install::process::command(
        &opts.claude_path,
        &opts.home,
        claude::session(&launch),
    );
    cmd.current_dir(&opts.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    install::process::no_window(cmd)
}

/// Logs each stderr line, and returns the last once stderr closes.
async fn log_stderr(stderr: ChildStderr) -> Option<String> {
    n0_future::stream::unfold(
        BufReader::new(stderr).lines(),
        |mut lines| async move {
            lines
                .next_line()
                .await
                .ok()
                .flatten()
                .map(|line| (line, lines))
        },
    )
    .inspect(|line| tracing::warn!(target: "claude::stderr", "{line}"))
    .last()
    .await
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    /// Gets every raw stdout line.
    stdout_log: Option<std::fs::File>,
}

impl Running {
    async fn read(&mut self) -> Input {
        let read = self.stdout.next_line().await;
        record(self.stdout_log.as_mut(), &read);
        Input::Read(read.map_err(|error| error.to_string()))
    }
}

fn record(
    stdout_log: Option<&mut std::fs::File>,
    read: &std::io::Result<Option<String>>,
) {
    if let (Some(log), Ok(Some(line))) = (stdout_log, read)
        && let Err(error) = writeln!(log, "{line}")
    {
        tracing::warn!("writing stdout log: {error}");
    }
}

/// Every handle is gone, so the actor has stopped the process and ended.
struct Stopped {}

struct Actor {
    session: Session<Reply>,
    /// The process, until it is stopped.
    process: Option<Running>,
    events: mpsc::Sender<ClaudeEvent>,
}

impl Actor {
    async fn run(
        mut self,
        mut commands: mpsc::Receiver<CommandMsg>,
    ) -> Result<Infallible, Stopped> {
        loop {
            let input = self.next_input(&mut commands).await;
            self.feed(input).await?;
        }
    }

    /// The next stdout line or command, and only commands once the process
    /// has stopped.
    async fn next_input(
        &mut self,
        commands: &mut mpsc::Receiver<CommandMsg>,
    ) -> Input {
        tokio::select! {
            biased;
            read = read(self.process.as_mut()) => read,
            command = commands.recv() => command.map_or(Input::Closed, Input::Command),
        }
    }

    /// Steps the session and performs its effects in order, each effect's
    /// own effects before the next.
    async fn feed(&mut self, input: Input) -> Result<(), Stopped> {
        let mut effects = self.session.step(input);
        effects.reverse();
        while let Some(effect) = effects.pop() {
            let more = self.perform(effect).await?;
            effects.extend(more.into_iter().rev());
        }
        Ok(())
    }

    async fn perform(
        &mut self,
        effect: Effect,
    ) -> Result<Vec<Effect>, Stopped> {
        match effect {
            Effect::Write { line, then } => self.write(line, then).await,
            Effect::Emit(event) => self.emit(event).await,
            Effect::Log(note) => {
                runtime::log(&note);
                Ok(Vec::new())
            }
            Effect::Reply(reply, answer) => {
                Ok(deliver(reply, answer.map_err(Error::from)))
            }
            Effect::Report(waiter, exit) => Ok(deliver(waiter, exit)),
            Effect::Stop(then) => self.stop(then).await,
            Effect::End => Err(Stopped {}),
        }
    }

    async fn write(
        &mut self,
        line: String,
        then: Then<Reply>,
    ) -> Result<Vec<Effect>, Stopped> {
        let outcome = put_line(self.process.as_mut(), &line).await;
        Ok(self.session.step(Input::Wrote { then, outcome }))
    }

    async fn stop(
        &mut self,
        then: After<Waiter>,
    ) -> Result<Vec<Effect>, Stopped> {
        let exit = stop_process(self.process.take()).await;
        Ok(self.session.step(Input::Stopped { exit, then }))
    }

    async fn emit(&self, event: ClaudeEvent) -> Result<Vec<Effect>, Stopped> {
        // A dropped receiver only means nobody is listening any more.
        let _ = self.events.send(event).await;
        Ok(Vec::new())
    }
}

/// The next stdout line; once the process has stopped, nothing.
async fn read(running: Option<&mut Running>) -> Input {
    match running {
        Some(process) => process.read().await,
        None => pending().await,
    }
}

/// A dropped receiver only means nobody waits any more.
fn deliver<T>(reply: oneshot::Sender<T>, answer: T) -> Vec<Effect> {
    let _ = reply.send(answer);
    Vec::new()
}

async fn put_line(
    running: Option<&mut Running>,
    line: &str,
) -> std::io::Result<()> {
    let stdin = &mut running
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?
        .stdin;
    stdin.write_all(line.as_bytes()).await?;
    stdin.flush().await
}

/// The session only stops a running process.
async fn stop_process(running: Option<Running>) -> Exit {
    match running {
        Some(Running { child, stdin, .. }) => {
            install::process::stop(child, stdin).await
        }
        None => Exit::Unknown(String::from("already stopped")),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use proto::Effort;

    use super::{
        ClaudeOptions, PermissionMode, Resume, SessionId, SettingSource,
        VersionPolicy, command,
    };
    use install::Home;

    #[test]
    fn prefix_fork_uses_inclusive_uuid_and_chosen_new_session()
    -> Result<(), Box<dyn std::error::Error>> {
        let cmd = command(
            &ClaudeOptions {
                claude_path: "/opt/claude".into(),
                home: Home::User,
                credential: None,
                cwd: "/tmp".into(),
                model: None,
                effort: None,
                permission_mode: PermissionMode::Default,
                resume: Resume::Fork {
                    session: SessionId(String::from("source")),
                    through: claude::MessageId(String::from("assistant-uuid")),
                    target: SessionId(String::from("new-session")),
                },
                project_name: None,
                setting_sources: None,
                stdout_log: None,
                versions: VersionPolicy::SupportedOnly,
            },
            &SessionId(String::from("new-session")),
        );
        let args = cmd
            .as_std()
            .get_args()
            .map(|arg| arg.to_str())
            .collect::<Option<Vec<&str>>>()
            .ok_or("an argument is not UTF-8")?;
        assert_eq!(
            args,
            Vec::from([
                "--print",
                "--verbose",
                "--output-format",
                "stream-json",
                "--input-format",
                "stream-json",
                "--permission-mode",
                "default",
                "--resume=source",
                "--resume-session-at=assistant-uuid",
                "--fork-session",
                "--session-id",
                "new-session",
                "--replay-user-messages",
                "--include-partial-messages",
                "--thinking-display",
                "summarized",
                "--permission-prompt-tool",
                "stdio"
            ])
        );
        Ok(())
    }

    /// The variables the resume test checks, as the command sets them.
    fn checked_env(
        std: &std::process::Command,
    ) -> Result<Vec<(&str, Option<&str>)>, &'static str> {
        std.get_envs()
            .filter_map(|(key, setting)| match (key.to_str(), setting) {
                (
                    Some(
                        "CLAUDECODE"
                        | "CLAUDE_CODE_OAUTH_TOKEN"
                        | "CLAUDE_CODE_USE_BEDROCK"
                        | "CLAUDE_CODE_USE_VERTEX"
                        | "CLAUDE_CODE_USE_FOUNDRY"
                        | "ANTHROPIC_BASE_URL"
                        | "CLAUDE_CODE_SESSION_ID",
                    ),
                    None,
                ) => Some((key, None)),
                (
                    Some(
                        "CLAUDE_CONFIG_DIR"
                        | "CLAUDE_CODE_PROJECT_DIR_NAME"
                        | "DISABLE_UPDATES",
                    ),
                    Some(value),
                ) => Some((key, value.to_str())),
                _ => None,
            })
            .map(|(key, value)| key.to_str().map(|name| (name, value)))
            .collect::<Option<Vec<(&str, Option<&str>)>>>()
            .ok_or("a variable is not UTF-8")
    }

    #[test]
    fn resumed_session_argv() -> Result<(), Box<dyn std::error::Error>> {
        let cmd = command(
            &ClaudeOptions {
                claude_path: "/opt/claude".into(),
                home: Home::Private(PathBuf::from(
                    "/srv/conductor/home/claude",
                )),
                credential: None,
                cwd: "/tmp".into(),
                model: Some(String::from("haiku")),
                effort: Some(Effort::XHigh),
                permission_mode: PermissionMode::Default,
                resume: Resume::Session(SessionId(String::from("0b1c"))),
                project_name: Some(String::from("01JCDZ6Q0Y3K7V8M2N4P5R6S7T")),
                setting_sources: Some(Vec::from([
                    SettingSource::Project,
                    SettingSource::Local,
                ])),
                stdout_log: None,
                versions: VersionPolicy::SupportedOnly,
            },
            &SessionId(String::from("0b1c")),
        );
        let std = cmd.as_std();
        let args = std
            .get_args()
            .map(|a| a.to_str())
            .collect::<Option<Vec<&str>>>()
            .ok_or("an argument is not UTF-8")?;
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
                "--replay-user-messages",
                "--include-partial-messages",
                "--thinking-display",
                "summarized",
                "--permission-prompt-tool",
                "stdio",
            ]
        );
        assert_eq!(
            checked_env(std)?,
            [
                ("ANTHROPIC_BASE_URL", None),
                ("CLAUDECODE", None),
                ("CLAUDE_CODE_OAUTH_TOKEN", None),
                (
                    "CLAUDE_CODE_PROJECT_DIR_NAME",
                    Some("01JCDZ6Q0Y3K7V8M2N4P5R6S7T")
                ),
                ("CLAUDE_CODE_SESSION_ID", None),
                ("CLAUDE_CODE_USE_BEDROCK", None),
                ("CLAUDE_CODE_USE_FOUNDRY", None),
                ("CLAUDE_CODE_USE_VERTEX", None),
                ("CLAUDE_CONFIG_DIR", Some("/srv/conductor/home/claude")),
                ("DISABLE_UPDATES", Some("1")),
            ]
        );
        Ok(())
    }
}
