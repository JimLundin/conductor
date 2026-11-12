//! One tokio task owns the `codex app-server` child process for the whole host.
//! [`CodexServer`] and [`CodexThread`] are handles that talk to it over a
//! command channel. The task alone owns stdin, the JSON-RPC id counter, our
//! pending requests and Codex's open requests, so no state is shared behind a
//! lock.

use std::collections::HashMap;
use std::convert::Infallible;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use proto::CodexCredential;
use serde_json::Value;

use crate::{Exit, Home};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};

use super::config;
use super::protocol::{
    AnswerError, AnswerShape, ApprovalId, Call, CodexEvent, Decision, INITIALIZE, Inbound,
    Outbound, RequestId, ThreadEvent, ThreadId, ThreadOptions, ThreadSettings, TurnId, answer,
    encode, models, parse_line, thread_started, turn_id, user_agent,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone)]
pub struct ServerOptions {
    pub codex_path: PathBuf,
    pub home: Home,
    /// A private home's model provider; `None` leaves the home's own
    /// sign-in. Codex takes it at start only, since the key is in its
    /// environment.
    pub credential: Option<CodexCredential>,
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
    Rpc { method: String, message: String },
    #[error("codex did not answer {0} within {REQUEST_TIMEOUT:?}")]
    Timeout(String),
    #[error("no open approval with id {}", .0.0)]
    UnknownApproval(ApprovalId),
    #[error("approval {} cannot be answered with that decision", .0.0)]
    DecisionNotApplicable(ApprovalId),
}

enum CommandMsg {
    Request {
        call: Call,
        reply: oneshot::Sender<Result<Value, Error>>,
    },
    Respond {
        id: ApprovalId,
        decision: Decision,
        reply: oneshot::Sender<Result<(), Error>>,
    },
    Shutdown {
        reply: oneshot::Sender<Exit>,
    },
}

/// The host's app-server. Cloning yields another handle to the same process.
#[derive(Clone)]
pub struct CodexServer {
    commands: mpsc::Sender<CommandMsg>,
    user_agent: String,
    pid: Option<u32>,
}

/// A thread on a [`CodexServer`]. Its events arrive on the server's event
/// stream, tagged with [`CodexThread::id`].
#[derive(Clone)]
pub struct CodexThread {
    pub id: ThreadId,
    pub settings: ThreadSettings,
    server: CodexServer,
}

impl CodexServer {
    /// Starts `codex app-server` and completes the `initialize` handshake.
    pub async fn spawn(
        opts: ServerOptions,
    ) -> Result<(CodexServer, mpsc::Receiver<CodexEvent>), Error> {
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
        if let Home::Private(dir) = &opts.home {
            config::write(dir, opts.credential.as_ref())?;
        }
        let mut child = command(&opts).spawn()?;
        let pid = child.id();
        let ((stdin, stdout), stderr) = child
            .stdin
            .take()
            .zip(child.stdout.take())
            .zip(child.stderr.take())
            .ok_or(Error::Command(String::from("codex stdio was not piped")))?;
        tokio::spawn(log_stderr(stderr));

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

        let initialize = request(&commands_tx, Call::Initialize).await?;
        let server = CodexServer {
            commands: commands_tx,
            user_agent: user_agent(initialize)?,
            pid,
        };
        Ok((server, events_rx))
    }

    /// `userAgent` from the handshake. Its first segment carries the Codex
    /// version, as in `conductor/0.166.0 (...)`.
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    pub fn codex_version(&self) -> Option<&str> {
        let first = self.user_agent.split_whitespace().next()?;
        first.split_once('/').map(|(_, version)| version)
    }

    pub async fn list_models(&self) -> Result<proto::Models, Error> {
        Ok(models(request(&self.commands, Call::ModelList).await?)?)
    }

    pub async fn start_thread(&self, opts: ThreadOptions) -> Result<CodexThread, Error> {
        let (id, settings) =
            thread_started(request(&self.commands, Call::ThreadStart(opts)).await?)?;
        Ok(self.thread(id, settings))
    }

    /// Loads a thread from Codex's own history; works in a new process.
    pub async fn resume_thread(
        &self,
        id: ThreadId,
        opts: ThreadOptions,
    ) -> Result<CodexThread, Error> {
        let (id, settings) =
            thread_started(request(&self.commands, Call::ThreadResume(id, opts)).await?)?;
        Ok(self.thread(id, settings))
    }

    /// Answers an open approval or user-input request exactly once.
    pub async fn respond(&self, id: ApprovalId, decision: Decision) -> Result<(), Error> {
        ask(&self.commands, |reply| CommandMsg::Respond {
            id,
            decision,
            reply,
        })
        .await?
    }

    /// Closes stdin, waits briefly for a clean exit, then kills.
    pub async fn shutdown(&self) -> Result<Exit, Error> {
        ask(&self.commands, |reply| CommandMsg::Shutdown { reply }).await
    }

    fn thread(&self, id: ThreadId, settings: ThreadSettings) -> CodexThread {
        CodexThread {
            id,
            settings,
            server: self.clone(),
        }
    }
}

impl CodexThread {
    /// Starts a turn. The turn's events follow on the server's stream.
    pub async fn send_turn(&self, text: String) -> Result<TurnId, Error> {
        let call = Call::TurnStart {
            thread: self.id.clone(),
            text,
        };
        Ok(turn_id(request(&self.server.commands, call).await?)?)
    }

    /// Resolves once Codex accepts; `TurnCompleted` follows as an event.
    pub async fn interrupt(&self, turn: TurnId) -> Result<(), Error> {
        let call = Call::TurnInterrupt {
            thread: self.id.clone(),
            turn,
        };
        let _ = request(&self.server.commands, call).await?;
        Ok(())
    }

    /// Answers one of this thread's open approvals exactly once.
    pub async fn respond(&self, id: ApprovalId, decision: Decision) -> Result<(), Error> {
        self.server.respond(id, decision).await
    }
}

/// Sends `call` to the actor and waits for Codex's answer.
async fn request(commands: &mpsc::Sender<CommandMsg>, call: Call) -> Result<Value, Error> {
    let method = String::from(call.method());
    tokio::time::timeout(
        REQUEST_TIMEOUT,
        ask(commands, |reply| CommandMsg::Request { call, reply }),
    )
    .await
    .map_err(|_| Error::Timeout(method))??
}

async fn ask<T>(
    commands: &mpsc::Sender<CommandMsg>,
    build: impl FnOnce(oneshot::Sender<T>) -> CommandMsg,
) -> Result<T, Error> {
    let (reply, answer) = oneshot::channel();
    commands
        .send(build(reply))
        .await
        .map_err(|_| Error::Exited)?;
    answer.await.map_err(|_| Error::Exited)
}

/// `codex` with its home.
pub fn cli(codex_path: &Path, home: &Home) -> Command {
    let mut cmd = Command::new(codex_path);
    if let Home::Private(dir) = home {
        cmd.env_remove("CODEX_HOME");
        cmd.env_remove(config::KEY_VAR);
        cmd.env("CODEX_HOME", dir);
    }
    cmd
}

/// `codex login status`, which succeeds when `home` is signed in.
pub fn login_status(codex_path: &Path, home: &Home) -> Command {
    let mut cmd = cli(codex_path, home);
    cmd.args(["login", "status"]);
    cmd
}

/// The ChatGPT sign-in by device code, `codex login --device-auth`, into
/// `home`. It stores the credential there itself.
pub fn login(codex_path: &Path, home: &Home) -> Command {
    let mut cmd = cli(codex_path, home);
    cmd.args(["login", "--device-auth"]);
    cmd
}

pub(crate) fn command(opts: &ServerOptions) -> Command {
    let mut cmd = cli(&opts.codex_path, &opts.home);
    match &opts.credential {
        Some(CodexCredential::Provider { key, .. }) => {
            cmd.env(config::KEY_VAR, key);
        }
        None => {}
    }
    cmd.args(["app-server", "--listen", "stdio://"])
        .current_dir(&opts.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::process::no_window(cmd)
}

async fn log_stderr(stderr: ChildStderr) -> std::io::Result<()> {
    let mut lines = BufReader::new(stderr).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => tracing::debug!(target: "codex::stderr", "{line}"),
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

/// A Codex request we have not answered yet.
struct Open {
    rpc_id: RequestId,
    thread: ThreadId,
    shape: AnswerShape,
}

/// A request we sent, waiting for its response.
struct Pending {
    method: String,
    reply: oneshot::Sender<Result<Value, Error>>,
}

struct Actor {
    process: Process,
    events: mpsc::Sender<CodexEvent>,
    stdout_log: Option<std::fs::File>,
    /// Our requests awaiting a response, by JSON-RPC id.
    pending: HashMap<i64, Pending>,
    open: HashMap<ApprovalId, Open>,
    next_id: i64,
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
                tracing::error!("reading codex stdout: {e}");
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
                tracing::warn!("codex printed a non-JSON line ({e}): {line}");
                Inbound::Event(CodexEvent::Unknown(Value::String(line.to_owned())))
            }
        };
        match inbound {
            Inbound::Event(event) => self.emit(event).await,
            Inbound::Response { id, outcome } => self.on_response(id, outcome).await,
            Inbound::Approval {
                rpc_id,
                thread,
                shape,
                event,
            } => {
                self.open.insert(
                    ApprovalId::of(&rpc_id),
                    Open {
                        rpc_id,
                        thread: thread.clone(),
                        shape,
                    },
                );
                self.emit(CodexEvent::Thread { thread, event }).await;
            }
            Inbound::Resolved { thread, rpc_id } => {
                let id = ApprovalId::of(&rpc_id);
                // Codex also resolves requests we answered; only an unanswered
                // one means Codex withdrew it.
                if let Some(Open { .. }) = self.open.remove(&id) {
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
                if let Err(e) = self.write(Outbound::Reject { rpc_id, message }).await {
                    tracing::error!("rejecting {method}: {e}");
                }
                self.emit(event).await;
            }
            Inbound::Declined { rpc_id, event } => {
                tracing::info!("declined a question that asks for a secret answer");
                let message = String::from("conductor does not take secret answers");
                if let Err(e) = self.write(Outbound::Reject { rpc_id, message }).await {
                    tracing::error!("declining a secret question: {e}");
                }
                self.emit(event).await;
            }
        }
    }

    async fn on_response(&mut self, id: RequestId, outcome: Result<Value, String>) {
        let pending = match &id {
            RequestId::Integer(n) => self.pending.remove(n),
            RequestId::String(_) => None,
        };
        match pending {
            Some(Pending { method, reply }) => {
                if let (INITIALIZE, Ok(_)) = (method.as_str(), &outcome) {
                    self.initialized().await;
                }
                let _ = reply.send(outcome.map_err(|message| Error::Rpc { method, message }));
            }
            None => tracing::warn!("response for unknown request {id:?}"),
        }
    }

    async fn initialized(&mut self) {
        if let Err(e) = self.write(Outbound::Initialized).await {
            tracing::error!("sending initialized: {e}");
        }
    }

    async fn on_command(&mut self, cmd: CommandMsg) {
        match cmd {
            CommandMsg::Request { call, reply } => {
                self.next_id = self.next_id.saturating_add(1);
                let id = self.next_id;
                let method = String::from(call.method());
                match self.write(Outbound::Request { id, call }).await {
                    Ok(()) => {
                        self.pending.insert(id, Pending { method, reply });
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

    async fn respond(&mut self, id: ApprovalId, decision: Decision) -> Result<(), Error> {
        let open = self
            .open
            .remove(&id)
            .ok_or(Error::UnknownApproval(id.clone()))?;
        let answered = answer(&open.shape, decision);
        self.settle(id, open, answered).await
    }

    /// Sends the answer, or puts the approval back when the decision didn't fit.
    async fn settle(
        &mut self,
        id: ApprovalId,
        open: Open,
        answered: Result<Value, AnswerError>,
    ) -> Result<(), Error> {
        match answered {
            Ok(result) => {
                tracing::debug!("answering {} on thread {}", id.0, open.thread.0);
                self.write(Outbound::Answer {
                    rpc_id: open.rpc_id,
                    result,
                })
                .await
            }
            Err(AnswerError::NotApplicable) => {
                self.open.insert(id.clone(), open);
                Err(Error::DecisionNotApplicable(id))
            }
            Err(AnswerError::Encode(e)) => Err(Error::Json(e)),
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
                self.open.clear();
                self.emit(CodexEvent::Exited {
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

    async fn emit(&self, event: CodexEvent) {
        // A dropped receiver only means nobody is listening any more.
        let _ = self.events.send(event).await;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::path::Path;

    use tokio::sync::mpsc;

    use super::{
        Actor, AnswerError, AnswerShape, ApprovalId, Decision, Exit, Home, Open, Process,
        RequestId, ServerOptions, ThreadId, command,
    };

    #[tokio::test]
    async fn an_answer_that_does_not_encode_fails_and_closes_the_approval() {
        let (events, _listening) = mpsc::channel(1);
        let mut actor = Actor {
            process: Process::Exited(Exit::Unknown(String::from("never started"))),
            events,
            stdout_log: None,
            pending: HashMap::new(),
            open: HashMap::new(),
            next_id: 0,
        };
        let id = ApprovalId(String::from("ap_1"));
        let open = Open {
            rpc_id: RequestId::Integer(3),
            thread: ThreadId(String::from("th_1")),
            shape: AnswerShape::Command,
        };
        let failure = serde_json::to_value(BTreeMap::from([((1, 2), 0)])).unwrap_err();
        let first = actor
            .settle(id.clone(), open, Err(AnswerError::Encode(failure)))
            .await;
        let again = actor.respond(id, Decision::Accept).await;
        assert_eq!(
            (
                first.map_err(|e| e.to_string()),
                again.map_err(|e| e.to_string())
            ),
            (
                Err(String::from(
                    "could not encode or decode a codex message: key must be a string"
                )),
                Err(String::from("no open approval with id ap_1"))
            )
        );
    }

    #[test]
    fn app_server_argv() {
        let cmd = command(&ServerOptions {
            codex_path: "/opt/codex".into(),
            home: Home::User,
            credential: None,
            cwd: "/tmp".into(),
            stdout_log: None,
        });
        let std = cmd.as_std();
        let args = std
            .get_args()
            .map(|a| a.to_str().unwrap())
            .collect::<Vec<&str>>();
        assert_eq!(std.get_program(), "/opt/codex");
        assert_eq!(std.get_current_dir(), Some(Path::new("/tmp")));
        assert_eq!(args, ["app-server", "--listen", "stdio://"]);
    }
}
