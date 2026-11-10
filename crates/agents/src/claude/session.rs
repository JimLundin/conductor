//! One tokio task owns the `claude` child process. The handle talks to it over a
//! command channel; the task alone owns stdin, the pending control requests and
//! the open permission requests, so no state is shared behind a lock.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use claude_codes::ClaudeCliBuilder;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};

use super::protocol::{
    ClaudeEvent, ControlCall, Inbound, Outbound, PermissionDecision, PermissionMode,
    PermissionRequestId, SessionId, encode, parse_line,
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
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
        }
    }
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
    Control {
        subtype: &'static str,
        message: String,
    },
    #[error("claude did not answer {0} within {CONTROL_TIMEOUT:?}")]
    Timeout(&'static str),
    #[error("no open permission request with id {0}")]
    UnknownPermissionRequest(PermissionRequestId),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

enum CommandMsg {
    SendUser {
        text: String,
        reply: oneshot::Sender<Result<()>>,
    },
    Control {
        call: ControlCall,
        reply: oneshot::Sender<Result<Value>>,
    },
    Respond {
        id: PermissionRequestId,
        decision: PermissionDecision,
        reply: oneshot::Sender<Result<()>>,
    },
    Shutdown {
        reply: oneshot::Sender<ExitStatus>,
    },
}

pub struct ClaudeSession {
    commands: mpsc::Sender<CommandMsg>,
}

impl ClaudeSession {
    /// Starts `claude` and completes the `initialize` handshake.
    pub async fn spawn(opts: ClaudeOptions) -> Result<(Self, mpsc::Receiver<ClaudeEvent>)> {
        let stdout_log = match &opts.stdout_log {
            Some(path) => Some(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?,
            ),
            None => None,
        };
        let mut child = command(&opts)?.spawn()?;
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(Error::Command("claude stdio was not piped".into()));
        };
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::warn!(target: "claude::stderr", "{line}");
            }
        });

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
            next_request: 0,
        };
        tokio::spawn(actor.run(commands_rx));

        let session = Self {
            commands: commands_tx,
        };
        session.control(ControlCall::Initialize).await?;
        Ok((session, events_rx))
    }

    pub async fn send_user(&self, text: String) -> Result<()> {
        self.request(|reply| CommandMsg::SendUser { text, reply })
            .await?
    }

    /// Resolves once the CLI acknowledges; the turn's `result` follows as an event.
    pub async fn interrupt(&self) -> Result<()> {
        self.control(ControlCall::Interrupt).await.map(drop)
    }

    pub async fn set_permission_mode(&self, mode: PermissionMode) -> Result<()> {
        self.control(ControlCall::SetPermissionMode(mode))
            .await
            .map(drop)
    }

    pub async fn respond(
        &self,
        id: PermissionRequestId,
        decision: PermissionDecision,
    ) -> Result<()> {
        self.request(|reply| CommandMsg::Respond {
            id,
            decision,
            reply,
        })
        .await?
    }

    /// Closes stdin, waits briefly for a clean exit, then kills.
    pub async fn shutdown(self) -> Result<ExitStatus> {
        self.request(|reply| CommandMsg::Shutdown { reply }).await
    }

    async fn control(&self, call: ControlCall) -> Result<Value> {
        let subtype = call.subtype();
        let answer = self.request(|reply| CommandMsg::Control { call, reply });
        tokio::time::timeout(CONTROL_TIMEOUT, answer)
            .await
            .map_err(|_| Error::Timeout(subtype))??
    }

    async fn request<T>(&self, build: impl FnOnce(oneshot::Sender<T>) -> CommandMsg) -> Result<T> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(build(reply))
            .await
            .map_err(|_| Error::Exited)?;
        answer.await.map_err(|_| Error::Exited)
    }
}

pub(crate) fn command(opts: &ClaudeOptions) -> Result<Command> {
    let mut builder = ClaudeCliBuilder::new()
        .command(&opts.claude_path)
        .working_directory(&opts.cwd)
        .permission_prompt_tool("stdio")
        .resume(opts.resume.as_ref().map(|id| id.0.clone()));
    if let Some(model) = &opts.model {
        builder = builder.model(model);
    }
    let mut cmd = builder
        .build_command()
        .map_err(|e| Error::Command(e.to_string()))?;
    // The claude-codes builder cannot express `auto` or setting sources, so these
    // two flags are appended here.
    cmd.arg(format!(
        "--permission-mode={}",
        opts.permission_mode.as_str()
    ));
    if let Some(sources) = &opts.setting_sources {
        let joined: Vec<&str> = sources.iter().map(|s| s.as_str()).collect();
        cmd.arg(format!("--setting-sources={}", joined.join(",")));
    }
    cmd.env_remove("CLAUDECODE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    Ok(cmd)
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
}

impl Running {
    async fn stop(self) -> ExitStatus {
        let Running {
            mut child, stdin, ..
        } = self;
        drop(stdin);
        if let Ok(Ok(status)) = tokio::time::timeout(EXIT_GRACE, child.wait()).await {
            return status;
        }
        let _ = child.start_kill();
        child.wait().await.unwrap_or_default()
    }
}

enum Process {
    Running(Box<Running>),
    Exited(ExitStatus),
}

struct Actor {
    process: Process,
    events: mpsc::Sender<ClaudeEvent>,
    stdout_log: Option<std::fs::File>,
    /// Our control requests awaiting a `control_response`, by request_id.
    pending: HashMap<String, (ControlCall, oneshot::Sender<Result<Value>>)>,
    /// `can_use_tool` requests not yet answered, with the input `allow` echoes.
    open_permissions: HashMap<PermissionRequestId, Value>,
    next_request: u64,
}

impl Actor {
    async fn run(mut self, mut commands: mpsc::Receiver<CommandMsg>) {
        loop {
            let Process::Running(running) = &mut self.process else {
                let Some(cmd) = commands.recv().await else {
                    return;
                };
                self.on_command(cmd).await;
                continue;
            };
            tokio::select! {
                line = running.stdout.next_line() => match line {
                    Ok(Some(line)) => self.on_line(&line).await,
                    Ok(None) => {
                        self.stop().await;
                    }
                    Err(e) => {
                        tracing::error!("reading claude stdout: {e}");
                        self.stop().await;
                    }
                },
                cmd = commands.recv() => match cmd {
                    Some(cmd) => self.on_command(cmd).await,
                    None => {
                        self.stop().await;
                        return;
                    }
                },
            }
        }
    }

    async fn on_line(&mut self, line: &str) {
        if let Some(log) = &mut self.stdout_log
            && let Err(e) = writeln!(log, "{line}")
        {
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
                match &event {
                    ClaudeEvent::PermissionRequested { id, input, .. } => {
                        self.open_permissions.insert(id.clone(), input.clone());
                    }
                    ClaudeEvent::PermissionCancelled { id } => {
                        self.open_permissions.remove(id);
                    }
                    _ => {}
                }
                self.emit(event).await;
            }
            Inbound::ControlResponse {
                request_id,
                outcome,
            } => match self.pending.remove(&request_id) {
                Some((call, reply)) => {
                    let _ = reply.send(outcome.map_err(|message| Error::Control {
                        subtype: call.subtype(),
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
                        request_id: &request_id,
                        message: &message,
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
                let _ = reply.send(self.write(Outbound::User(&text)).await);
            }
            CommandMsg::Control { call, reply } => {
                self.next_request += 1;
                let request_id = format!("req_{}", self.next_request);
                match self
                    .write(Outbound::Control {
                        request_id: &request_id,
                        call,
                    })
                    .await
                {
                    Ok(()) => {
                        self.pending.insert(request_id, (call, reply));
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
                    Some(input) => {
                        self.write(Outbound::Permission {
                            id: &id,
                            decision,
                            input,
                        })
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

    async fn write(&mut self, msg: Outbound<'_>) -> Result<()> {
        let Process::Running(running) = &mut self.process else {
            return Err(Error::Exited);
        };
        let mut line = encode(msg)?;
        line.push('\n');
        running.stdin.write_all(line.as_bytes()).await?;
        running.stdin.flush().await?;
        Ok(())
    }

    /// Moves to `Exited`, failing everything still waiting on the process.
    async fn stop(&mut self) -> ExitStatus {
        let running =
            match std::mem::replace(&mut self.process, Process::Exited(ExitStatus::default())) {
                Process::Running(running) => running,
                Process::Exited(status) => {
                    self.process = Process::Exited(status);
                    return status;
                }
            };
        let status = (*running).stop().await;
        self.process = Process::Exited(status);
        for (_, (_, reply)) in self.pending.drain() {
            let _ = reply.send(Err(Error::Exited));
        }
        self.open_permissions.clear();
        self.emit(ClaudeEvent::Exited { status }).await;
        status
    }

    async fn emit(&self, event: ClaudeEvent) {
        // A dropped receiver only means nobody is listening any more.
        let _ = self.events.send(event).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resumed_session_argv() {
        let cmd = command(&ClaudeOptions {
            claude_path: "/opt/claude".into(),
            cwd: "/tmp".into(),
            model: Some("haiku".into()),
            permission_mode: PermissionMode::Default,
            resume: Some(SessionId("0b1c".into())),
            setting_sources: Some(vec![SettingSource::Project, SettingSource::Local]),
            stdout_log: None,
        })
        .unwrap();
        let std = cmd.as_std();
        let args: Vec<_> = std.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(std.get_program(), "/opt/claude");
        assert_eq!(std.get_current_dir(), Some(std::path::Path::new("/tmp")));
        assert_eq!(
            args,
            [
                "--print",
                "--verbose",
                "--output-format",
                "stream-json",
                "--input-format",
                "stream-json",
                "--resume",
                "0b1c",
                "--model",
                "haiku",
                "--permission-prompt-tool",
                "stdio",
                "--permission-mode=default",
                "--setting-sources=project,local",
            ]
        );
        assert!(
            std.get_envs()
                .any(|(k, v)| k == "CLAUDECODE" && v.is_none())
        );
    }
}
