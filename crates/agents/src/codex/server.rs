//! One tokio task owns the `codex app-server` child process for the whole host.
//! [`CodexServer`] and [`CodexThread`] are handles that talk to it over a
//! command channel. The task alone owns stdin, the JSON-RPC id counter, our
//! pending requests and Codex's open requests, so no state is shared behind a
//! lock.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use codex_codes::{AppServerBuilder, RequestId};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};

use super::protocol::{
    AnswerShape, ApprovalId, Call, CodexEvent, Decision, Inbound, ModelInfo, Outbound, ThreadEvent,
    ThreadId, ThreadOptions, TurnId, answer, encode, models, parse_line, thread_id, turn_id,
    user_agent,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const EXIT_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub codex_path: PathBuf,
    /// The app-server's own working directory; each thread sets its `cwd`.
    pub cwd: PathBuf,
    /// Append every raw stdout line to this file.
    pub stdout_log: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not build the codex command: {0}")]
    Command(String),
    #[error("codex i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not encode or decode a codex message: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the codex app-server has exited")]
    Exited,
    #[error("codex rejected {method}: {message}")]
    Rpc {
        method: &'static str,
        message: String,
    },
    #[error("codex did not answer {0} within {REQUEST_TIMEOUT:?}")]
    Timeout(&'static str),
    #[error("no open approval with id {0}")]
    UnknownApproval(ApprovalId),
    #[error("approval {0} cannot be answered with that decision")]
    DecisionNotApplicable(ApprovalId),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

enum CommandMsg {
    Request {
        call: Call,
        reply: oneshot::Sender<Result<Value>>,
    },
    Respond {
        id: ApprovalId,
        decision: Decision,
        reply: oneshot::Sender<Result<()>>,
    },
    Shutdown {
        reply: oneshot::Sender<ExitStatus>,
    },
}

/// The host's app-server. Cloning yields another handle to the same process.
#[derive(Clone)]
pub struct CodexServer {
    commands: mpsc::Sender<CommandMsg>,
    user_agent: String,
}

/// A thread on a [`CodexServer`]. Its events arrive on the server's event
/// stream, tagged with [`CodexThread::id`].
#[derive(Clone)]
pub struct CodexThread {
    pub id: ThreadId,
    server: CodexServer,
}

impl CodexServer {
    /// Starts `codex app-server` and completes the `initialize` handshake.
    pub async fn spawn(opts: ServerOptions) -> Result<(Self, mpsc::Receiver<CodexEvent>)> {
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
            return Err(Error::Command("codex stdio was not piped".into()));
        };
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "codex::stderr", "{line}");
            }
        });

        let (events_tx, events_rx) = mpsc::channel(4096);
        let (commands_tx, commands_rx) = mpsc::channel(64);
        let actor = Actor {
            process: Process::Running(Box::new(Running {
                child,
                stdin,
                stdout: BufReader::new(stdout).lines(),
            })),
            events: events_tx,
            stdout_log,
            pending: HashMap::new(),
            open: HashMap::new(),
            next_id: 0,
        };
        tokio::spawn(actor.run(commands_rx));

        let mut server = Self {
            commands: commands_tx,
            user_agent: String::new(),
        };
        server.user_agent = user_agent(server.call(Call::Initialize).await?)?;
        Ok((server, events_rx))
    }

    /// `userAgent` from the handshake. Its first segment carries the Codex
    /// version, as in `conductor/0.166.0 (...)`.
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    pub fn codex_version(&self) -> Option<&str> {
        let first = self.user_agent.split_whitespace().next()?;
        first.split_once('/').map(|(_, version)| version)
    }

    pub async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(models(self.call(Call::ModelList).await?)?)
    }

    pub async fn start_thread(&self, opts: ThreadOptions) -> Result<CodexThread> {
        let id = thread_id(self.call(Call::ThreadStart(opts)).await?)?;
        Ok(self.thread(id))
    }

    /// Loads a thread from Codex's own history; works in a new process.
    pub async fn resume_thread(&self, id: ThreadId, opts: ThreadOptions) -> Result<CodexThread> {
        let id = thread_id(self.call(Call::ThreadResume(id, opts)).await?)?;
        Ok(self.thread(id))
    }

    /// Answers an open approval or user-input request exactly once.
    pub async fn respond(&self, id: ApprovalId, decision: Decision) -> Result<()> {
        self.request(|reply| CommandMsg::Respond {
            id,
            decision,
            reply,
        })
        .await?
    }

    /// Closes stdin, waits briefly for a clean exit, then kills.
    pub async fn shutdown(&self) -> Result<ExitStatus> {
        self.request(|reply| CommandMsg::Shutdown { reply }).await
    }

    fn thread(&self, id: ThreadId) -> CodexThread {
        CodexThread {
            id,
            server: self.clone(),
        }
    }

    async fn call(&self, call: Call) -> Result<Value> {
        let method = call.method();
        let answer = self.request(|reply| CommandMsg::Request { call, reply });
        tokio::time::timeout(REQUEST_TIMEOUT, answer)
            .await
            .map_err(|_| Error::Timeout(method))??
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

impl CodexThread {
    /// Starts a turn. The turn's events follow on the server's stream.
    pub async fn send_turn(&self, text: String) -> Result<TurnId> {
        let call = Call::TurnStart {
            thread: self.id.clone(),
            text,
        };
        Ok(turn_id(self.server.call(call).await?)?)
    }

    /// Adds input to the running turn `expected_turn`.
    pub async fn steer(&self, expected_turn: TurnId, text: String) -> Result<()> {
        let call = Call::TurnSteer {
            thread: self.id.clone(),
            expected_turn,
            text,
        };
        self.server.call(call).await.map(drop)
    }

    /// Resolves once Codex accepts; `TurnCompleted` follows as an event.
    pub async fn interrupt(&self, turn: TurnId) -> Result<()> {
        let call = Call::TurnInterrupt {
            thread: self.id.clone(),
            turn,
        };
        self.server.call(call).await.map(drop)
    }
}

pub(crate) fn command(opts: &ServerOptions) -> Result<Command> {
    let std = AppServerBuilder::new()
        .command(&opts.codex_path)
        .working_directory(&opts.cwd)
        .build_command()
        .map_err(|e| Error::Command(e.to_string()))?;
    let mut cmd = Command::from(std);
    cmd.stdin(Stdio::piped())
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

/// A Codex request we have not answered yet.
struct Open {
    rpc_id: RequestId,
    thread: ThreadId,
    shape: AnswerShape,
}

struct Actor {
    process: Process,
    events: mpsc::Sender<CodexEvent>,
    stdout_log: Option<std::fs::File>,
    /// Our requests awaiting a response, by JSON-RPC id.
    pending: HashMap<i64, (&'static str, oneshot::Sender<Result<Value>>)>,
    open: HashMap<ApprovalId, Open>,
    next_id: i64,
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
                        tracing::error!("reading codex stdout: {e}");
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
                tracing::warn!("codex printed a non-JSON line ({e}): {line}");
                Inbound::Event(CodexEvent::Unknown(Value::String(line.to_owned())))
            }
        };
        match inbound {
            Inbound::Event(event) => self.emit(event).await,
            Inbound::Response { id, outcome } => {
                let pending = match id {
                    RequestId::Integer(n) => self.pending.remove(&n),
                    RequestId::String(_) => None,
                };
                match pending {
                    Some((method, reply)) => {
                        if method == Call::Initialize.method()
                            && outcome.is_ok()
                            && let Err(e) = self.write(Outbound::Initialized).await
                        {
                            tracing::error!("sending initialized: {e}");
                        }
                        let _ =
                            reply.send(outcome.map_err(|message| Error::Rpc { method, message }));
                    }
                    None => tracing::warn!("response for unknown request {id}"),
                }
            }
            Inbound::Approval {
                rpc_id,
                thread,
                shape,
                event,
            } => {
                self.open.insert(
                    ApprovalId::from(&rpc_id),
                    Open {
                        rpc_id,
                        thread: thread.clone(),
                        shape,
                    },
                );
                self.emit(CodexEvent::Thread { thread, event }).await;
            }
            Inbound::Resolved { thread, rpc_id } => {
                let id = ApprovalId::from(&rpc_id);
                // Codex also resolves requests we answered; only an unanswered
                // one means Codex withdrew it.
                if self.open.remove(&id).is_some() {
                    self.emit(CodexEvent::Thread {
                        thread,
                        event: ThreadEvent::ApprovalCancelled { id },
                    })
                    .await;
                }
            }
            Inbound::Unsupported {
                rpc_id,
                method,
                event,
            } => {
                let message = format!("conductor does not handle {method}");
                tracing::warn!("{message}");
                if let Err(e) = self
                    .write(Outbound::Reject {
                        rpc_id: &rpc_id,
                        message: &message,
                    })
                    .await
                {
                    tracing::error!("rejecting {method}: {e}");
                }
                self.emit(event).await;
            }
        }
    }

    async fn on_command(&mut self, cmd: CommandMsg) {
        match cmd {
            CommandMsg::Request { call, reply } => {
                self.next_id += 1;
                let id = self.next_id;
                match self.write(Outbound::Request { id, call: &call }).await {
                    Ok(()) => {
                        self.pending.insert(id, (call.method(), reply));
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
                let _ = reply.send(self.respond(id, decision).await);
            }
            CommandMsg::Shutdown { reply } => {
                let _ = reply.send(self.stop().await);
            }
        }
    }

    async fn respond(&mut self, id: ApprovalId, decision: Decision) -> Result<()> {
        let Some(open) = self.open.get(&id) else {
            return Err(Error::UnknownApproval(id));
        };
        let result =
            answer(&open.shape, decision).map_err(|_| Error::DecisionNotApplicable(id.clone()))?;
        let open = self.open.remove(&id).expect("looked up above");
        tracing::debug!("answering {id} on thread {}", open.thread.0);
        self.write(Outbound::Answer {
            rpc_id: &open.rpc_id,
            result,
        })
        .await
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
        self.open.clear();
        self.emit(CodexEvent::Exited { status }).await;
        status
    }

    async fn emit(&self, event: CodexEvent) {
        // A dropped receiver only means nobody is listening any more.
        let _ = self.events.send(event).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_server_argv() {
        let cmd = command(&ServerOptions {
            codex_path: "/opt/codex".into(),
            cwd: "/tmp".into(),
            stdout_log: None,
        })
        .unwrap();
        let std = cmd.as_std();
        let args: Vec<_> = std.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(std.get_program(), "/opt/codex");
        assert_eq!(std.get_current_dir(), Some(std::path::Path::new("/tmp")));
        assert_eq!(args, ["app-server", "--listen", "stdio://"]);
    }
}
