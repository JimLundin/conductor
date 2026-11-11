//! One tokio task owns the `claude` child process. The handle talks to it over a
//! command channel; the task alone owns stdin, the pending control requests and
//! the open permission requests, so no state is shared behind a lock.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::convert::Infallible;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::time::Duration;

use claude_codes::ClaudeCliBuilder;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};

use super::protocol::{
    ClaudeEvent, ControlCall, Inbound, Outbound, PermissionDecision, PermissionMode,
    PermissionRequestId, SessionId, UnsupportedVersion, check_version, encode, parse_line,
};

const CONTROL_TIMEOUT: Duration = Duration::from_secs(60);
const EXIT_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSource {
    User,
    Project,
    Local,
}

impl SettingSource {
    pub fn to_cli(self) -> claude_codes::SettingSource {
        match self {
            SettingSource::User => claude_codes::SettingSource::User,
            SettingSource::Project => claude_codes::SettingSource::Project,
            SettingSource::Local => claude_codes::SettingSource::Local,
        }
    }
}

/// Which CLI versions a session agrees to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionPolicy {
    /// Refuse a CLI outside the range `claude-codes` supports.
    SupportedOnly,
    /// Run it anyway, with a warning.
    AllowUntested,
}

#[derive(Debug, Clone)]
pub struct ClaudeOptions {
    pub claude_path: PathBuf,
    pub cwd: PathBuf,
    pub model: Option<String>,
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
    #[error("claude rejected {subtype}: {message}")]
    Control { subtype: String, message: String },
    #[error("claude did not answer {0} within {CONTROL_TIMEOUT:?}")]
    Timeout(String),
    #[error(
        "claude {found} is not supported (supported: {supported}); set VersionPolicy::AllowUntested to run it anyway"
    )]
    UnsupportedVersion { found: String, supported: String },
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
        reply: oneshot::Sender<ExitStatus>,
    },
}

pub struct ClaudeSession {
    commands: mpsc::Sender<CommandMsg>,
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
        let mut child = command(&opts).spawn()?;
        let ((stdin, stdout), stderr) = child
            .stdin
            .take()
            .zip(child.stdout.take())
            .zip(child.stderr.take())
            .ok_or(Error::Command(String::from("claude stdio was not piped")))?;
        tokio::spawn(log_stderr(stderr));

        let (events_tx, events_rx) = mpsc::channel(1024);
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
            session_id: None,
            next_request: 0,
        };
        tokio::spawn(actor.run(commands_rx));

        let session = ClaudeSession {
            commands: commands_tx,
        };
        let initialize = session.control(ControlCall::Initialize).await?;
        match (check_version(initialize), opts.versions) {
            (Ok(()), _) => Ok((session, events_rx)),
            (Err(UnsupportedVersion { found, supported }), VersionPolicy::AllowUntested) => {
                tracing::warn!("running untested claude {found} (supported: {supported})");
                Ok((session, events_rx))
            }
            (Err(UnsupportedVersion { found, supported }), VersionPolicy::SupportedOnly) => {
                session.shutdown().await?;
                Err(Error::UnsupportedVersion { found, supported })
            }
        }
    }

    pub async fn send_user(&self, text: String) -> Result<(), Error> {
        let (reply, answer) = oneshot::channel();
        self.send(CommandMsg::SendUser { text, reply }).await?;
        answer.await.map_err(|_| Error::Exited)?
    }

    /// Resolves once the CLI acknowledges; the turn's `result` follows as an event.
    pub async fn interrupt(&self) -> Result<(), Error> {
        self.control(ControlCall::Interrupt).await.map(drop)
    }

    pub async fn set_permission_mode(&self, mode: PermissionMode) -> Result<(), Error> {
        self.control(ControlCall::SetPermissionMode(mode))
            .await
            .map(drop)
    }

    pub async fn respond(
        &self,
        id: PermissionRequestId,
        decision: PermissionDecision,
    ) -> Result<(), Error> {
        let (reply, answer) = oneshot::channel();
        self.send(CommandMsg::Respond {
            id,
            decision,
            reply,
        })
        .await?;
        answer.await.map_err(|_| Error::Exited)?
    }

    /// Closes stdin, waits briefly for a clean exit, then kills.
    pub async fn shutdown(self) -> Result<ExitStatus, Error> {
        let (reply, answer) = oneshot::channel();
        self.send(CommandMsg::Shutdown { reply }).await?;
        answer.await.map_err(|_| Error::Exited)
    }

    async fn control(&self, call: ControlCall) -> Result<Value, Error> {
        tokio::time::timeout(CONTROL_TIMEOUT, self.ask_control(call))
            .await
            .map_err(|_| Error::Timeout(String::from(call.subtype())))?
    }

    async fn ask_control(&self, call: ControlCall) -> Result<Value, Error> {
        let (reply, answer) = oneshot::channel();
        self.send(CommandMsg::Control { call, reply }).await?;
        answer.await.map_err(|_| Error::Exited)?
    }

    async fn send(&self, msg: CommandMsg) -> Result<(), Error> {
        self.commands.send(msg).await.map_err(|_| Error::Exited)
    }
}

pub(crate) fn command(opts: &ClaudeOptions) -> Command {
    let builder = ClaudeCliBuilder::new()
        .command(&opts.claude_path)
        .working_directory(&opts.cwd)
        .permission_prompt_tool("stdio")
        .permission_mode(opts.permission_mode.to_cli())
        .resume(opts.resume.as_ref().map(|id| id.0.clone()));
    let builder = match &opts.model {
        Some(model) => builder.model(model),
        None => builder,
    };
    let builder = match &opts.setting_sources {
        Some(sources) => {
            builder.setting_sources(sources.iter().copied().map(SettingSource::to_cli))
        }
        None => builder,
    };
    builder.build_command()
}

async fn log_stderr(stderr: ChildStderr) -> std::io::Result<()> {
    let mut lines = BufReader::new(stderr).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => tracing::warn!(target: "claude::stderr", "{line}"),
            Ok(None) => break Ok(()),
            Err(e) => break Err(e),
        }
    }
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
}

impl Running {
    async fn stop(self) -> ExitStatus {
        let Running {
            mut child,
            stdin,
            stdout: _,
        } = self;
        drop(stdin);
        match tokio::time::timeout(EXIT_GRACE, child.wait()).await {
            Ok(Ok(status)) => status,
            Ok(Err(_)) | Err(_) => {
                drop(child.start_kill());
                child.wait().await.unwrap_or_default()
            }
        }
    }
}

enum Process {
    Running(Box<Running>),
    Exited(ExitStatus),
}

/// One thing the actor reacts to.
enum Input {
    Line(std::io::Result<Option<String>>),
    Command(CommandMsg),
    /// Every handle is gone.
    Closed,
}

/// Whether an event tells the listener something new.
enum News {
    Fresh,
    Repeat,
}

/// Every handle is gone, so the actor has stopped the process and ended.
struct Stopped {}

struct Actor {
    process: Process,
    events: mpsc::Sender<ClaudeEvent>,
    stdout_log: Option<std::fs::File>,
    /// Our control requests awaiting a `control_response`, by request_id.
    pending: HashMap<String, (ControlCall, oneshot::Sender<Result<Value, Error>>)>,
    /// `can_use_tool` requests not yet answered, with the input `allow` echoes.
    open_permissions: HashMap<PermissionRequestId, Value>,
    /// The CLI repeats `system/init` every turn; only a new id is news.
    session_id: Option<SessionId>,
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
        match self.stdout_log.as_mut().map(|log| writeln!(log, "{line}")) {
            Some(Ok(())) => {}
            Some(Err(e)) => tracing::warn!("writing stdout log: {e}"),
            None => {}
        }
        let inbound = match parse_line(line) {
            Ok(inbound) => inbound,
            Err(e) => {
                tracing::warn!("claude printed a non-JSON line ({e}): {line}");
                Inbound::Event(ClaudeEvent::Unknown(Value::String(line.to_owned())))
            }
        };
        match inbound {
            Inbound::Event(event) => match self.track(&event) {
                News::Fresh => self.emit(event).await,
                News::Repeat => {}
            },
            Inbound::ControlResponse {
                request_id,
                outcome,
            } => match self.pending.remove(&request_id) {
                Some((call, reply)) => {
                    drop(reply.send(outcome.map_err(|message| Error::Control {
                        subtype: String::from(call.subtype()),
                        message,
                    })))
                }
                None => tracing::warn!("control_response for unknown request {request_id}"),
            },
            Inbound::UnsupportedControlRequest {
                request_id,
                subtype,
            } => {
                let message = format!("unsupported control request subtype: {subtype}");
                tracing::warn!("{message}");
                match self
                    .write(Outbound::ControlError {
                        request_id,
                        message,
                    })
                    .await
                {
                    Ok(()) => {}
                    Err(e) => tracing::error!("replying to {subtype}: {e}"),
                }
            }
        }
    }

    /// Records what an event changes in the open permissions and the session id.
    fn track(&mut self, event: &ClaudeEvent) -> News {
        match event {
            ClaudeEvent::PermissionRequested { id, input, .. } => {
                self.open_permissions.insert(id.clone(), input.clone());
                News::Fresh
            }
            ClaudeEvent::PermissionCancelled { id } => {
                self.open_permissions.remove(id);
                News::Fresh
            }
            ClaudeEvent::SessionStarted { session_id } => match self
                .session_id
                .replace(session_id.clone())
                .map(|seen| seen.0.cmp(&session_id.0))
            {
                Some(Ordering::Equal) => News::Repeat,
                Some(Ordering::Less | Ordering::Greater) | None => News::Fresh,
            },
            _ => News::Fresh,
        }
    }

    async fn on_command(&mut self, cmd: CommandMsg) {
        match cmd {
            CommandMsg::SendUser { text, reply } => {
                drop(reply.send(self.write(Outbound::User(text)).await));
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
                        self.pending.insert(request_id, (call, reply));
                    }
                    Err(e) => drop(reply.send(Err(e))),
                }
            }
            CommandMsg::Respond {
                id,
                decision,
                reply,
            } => {
                let result = match self.open_permissions.remove(&id) {
                    None => Err(Error::UnknownPermissionRequest(id)),
                    Some(input) => {
                        self.write(Outbound::Permission {
                            id,
                            decision,
                            input,
                        })
                        .await
                    }
                };
                drop(reply.send(result));
            }
            CommandMsg::Shutdown { reply } => {
                reply.send(self.stop().await).ok();
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
    async fn stop(&mut self) -> ExitStatus {
        match std::mem::replace(&mut self.process, Process::Exited(ExitStatus::default())) {
            Process::Running(running) => {
                let status = running.stop().await;
                self.process = Process::Exited(status);
                self.pending
                    .drain()
                    .for_each(|(_, (_, reply))| drop(reply.send(Err(Error::Exited))));
                self.open_permissions.clear();
                self.emit(ClaudeEvent::Exited { status }).await;
                status
            }
            Process::Exited(status) => {
                self.process = Process::Exited(status);
                status
            }
        }
    }

    async fn emit(&self, event: ClaudeEvent) {
        // A dropped receiver only means nobody is listening any more.
        drop(self.events.send(event).await);
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{ClaudeOptions, PermissionMode, SessionId, SettingSource, VersionPolicy, command};

    #[test]
    fn resumed_session_argv() {
        let cmd = command(&ClaudeOptions {
            claude_path: "/opt/claude".into(),
            cwd: "/tmp".into(),
            model: Some(String::from("haiku")),
            permission_mode: PermissionMode::Default,
            resume: Some(SessionId(String::from("0b1c"))),
            setting_sources: Some(Vec::from([SettingSource::Project, SettingSource::Local])),
            stdout_log: None,
            versions: VersionPolicy::SupportedOnly,
        });
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
                "--setting-sources",
                "project,local",
                "--permission-prompt-tool",
                "stdio",
            ]
        );
        let removed = std
            .get_envs()
            .filter_map(|(key, value)| match (key.to_str(), value) {
                (Some("CLAUDECODE"), None) => Some("CLAUDECODE"),
                (Some("CLAUDE_CODE_SESSION_ID"), None) => Some("CLAUDE_CODE_SESSION_ID"),
                _ => None,
            })
            .collect::<Vec<&str>>();
        assert_eq!(removed, ["CLAUDECODE", "CLAUDE_CODE_SESSION_ID"]);
    }
}
