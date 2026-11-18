//! One tokio task owns the `codex app-server` child process for the whole host.
//! [`CodexServer`] and [`CodexThread`] are handles that talk to it over a
//! command channel. The task alone owns stdin and the server's state machine
//! ([`codex::Server`]), so no state is shared behind a lock. The task turns
//! what happens into the machine's inputs and performs its effects.

use std::convert::Infallible;
use std::future::pending;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use n0_future::StreamExt;
use proto::{CodexCredential, Effort};
use serde_json::Value;

use agents::Exit;
use install::Home;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};

use super::config;
use codex::{
    After, ApprovalId, Call, CodexEvent, Decision, Failure, History, Paging,
    Server, Then, ThreadId, ThreadOptions, ThreadSettings, TurnId,
    history_page, models, thread_started, turn_id, user_agent,
};

const STDERR_TIMEOUT: Duration = Duration::from_secs(5);

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
    #[error("the codex app-server has exited: {0}")]
    Refused(String),
    #[error("codex rejected {method}: {message}")]
    Rpc { method: String, message: String },
    #[error("codex did not answer {0} within {REQUEST_TIMEOUT:?}")]
    Timeout(String),
    #[error(
        "codex history exceeded 100 pages; refusing an incomplete undo check"
    )]
    HistoryLimit,
    #[error("no open approval with id {}", .0.0)]
    UnknownApproval(ApprovalId),
    #[error("approval {} cannot be answered with that decision", .0.0)]
    DecisionNotApplicable(ApprovalId),
    /// The hub's app-server didn't start, which every ask that waited for
    /// it shares.
    #[error("{0}")]
    Start(Arc<Self>),
    #[error("the codex hub has stopped")]
    HubStopped,
}

impl From<Failure<std::io::Error>> for Error {
    fn from(failure: Failure<std::io::Error>) -> Self {
        match failure {
            Failure::Exited => Self::Exited,
            Failure::Refused(reason) => Self::Refused(reason),
            Failure::Io(error) => Self::Io(error),
            Failure::Json(error) => Self::Json(error),
            Failure::Rpc { method, message } => Self::Rpc { method, message },
            Failure::UnknownApproval(id) => Self::UnknownApproval(id),
            Failure::DecisionNotApplicable(id) => {
                Self::DecisionNotApplicable(id)
            }
        }
    }
}

/// Who waits for a command's answer.
type Reply = oneshot::Sender<Result<Value, Error>>;
/// Who waits for a shutdown's exit.
type Waiter = oneshot::Sender<Exit>;
type Input = codex::Input<Reply, Waiter, std::io::Error>;
type Effect = codex::Effect<Reply, Waiter, std::io::Error>;
type CommandMsg = codex::Command<Reply, Waiter>;

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
    ///
    /// # Errors
    /// Fails when the private home cannot be configured, the process cannot
    /// be started, or it does not complete the handshake.
    pub async fn spawn(
        opts: ServerOptions,
    ) -> Result<(Self, mpsc::Receiver<CodexEvent>), Error> {
        let Started {
            commands,
            events,
            pid,
        } = start(&opts)?;
        let initialize = request(&commands, Call::Initialize).await?;
        let server = Self {
            commands,
            user_agent: user_agent(initialize)?,
            pid,
        };
        Ok((server, events))
    }

    #[must_use]
    pub const fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// `userAgent` from the handshake. Its first segment carries the Codex
    /// version, as in `conductor/0.166.0 (...)`.
    #[must_use]
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    #[must_use]
    pub fn codex_version(&self) -> Option<&str> {
        codex::version(&self.user_agent)
    }

    /// # Errors
    /// Fails when Codex has exited, does not answer in time, rejects the
    /// request or answers with something we cannot read.
    pub async fn list_models(&self) -> Result<proto::Models, Error> {
        Ok(models(request(&self.commands, Call::ModelList).await?)?)
    }

    /// # Errors
    /// Fails when Codex has exited, does not answer in time, rejects the
    /// request or answers with something we cannot read.
    pub async fn start_thread(
        &self,
        opts: ThreadOptions,
    ) -> Result<CodexThread, Error> {
        self.open_thread(opts, None).await
    }

    /// Loads a thread from Codex's own history; works in a new process.
    ///
    /// # Errors
    /// Fails when Codex has exited, does not answer in time, rejects the
    /// request or answers with something we cannot read.
    pub async fn resume_thread(
        &self,
        id: ThreadId,
        opts: ThreadOptions,
    ) -> Result<CodexThread, Error> {
        self.open_thread(opts, Some(id)).await
    }

    /// Resumes `resume` if given, or else starts a thread.
    ///
    /// # Errors
    /// Fails when Codex has exited, does not answer in time, rejects the
    /// request or answers with something we cannot read.
    pub async fn open_thread(
        &self,
        opts: ThreadOptions,
        resume: Option<ThreadId>,
    ) -> Result<CodexThread, Error> {
        let (id, settings) = thread_started(
            request(&self.commands, Call::open(opts, resume)).await?,
        )?;
        Ok(self.thread(id, settings))
    }

    /// Answers an open approval or user-input request exactly once.
    ///
    /// # Errors
    /// Fails when Codex has exited, no approval with `id` is open, or
    /// `decision` does not fit it.
    pub async fn respond(
        &self,
        id: ApprovalId,
        decision: Decision,
    ) -> Result<(), Error> {
        let _ = ask(&self.commands, |reply| CommandMsg::Respond {
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
    /// Fails when the server's actor has already stopped.
    pub async fn shutdown(&self) -> Result<Exit, Error> {
        ask(&self.commands, CommandMsg::Shutdown).await
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
    /// Starts a turn, at `effort` if given: it stays the thread's effort
    /// for later turns. The turn's events follow on the server's stream.
    ///
    /// # Errors
    /// Fails when Codex has exited, does not answer in time, rejects the
    /// request or answers with something we cannot read.
    pub async fn send_turn(
        &self,
        text: String,
        effort: Option<Effort>,
    ) -> Result<TurnId, Error> {
        let call = Call::TurnStart {
            thread: self.id.clone(),
            text,
            effort,
        };
        Ok(turn_id(request(&self.server.commands, call).await?)?)
    }

    /// Discards this turn and every later turn from persisted conversation
    /// history. The thread id stays the same. The caller restores files separately.
    ///
    /// # Errors
    /// Fails when Codex has exited, does not answer in time, rejects the
    /// request or answers with something we cannot read.
    pub async fn revert(&self, before: TurnId) -> Result<(), Error> {
        let call = Call::ThreadRevert {
            thread: self.id.clone(),
            before,
        };
        let _ = request(&self.server.commands, call).await?;
        Ok(())
    }

    /// All native turn ids in chronological order. Fails rather than return
    /// a partial history if the bounded pagination cannot reach its end.
    ///
    /// # Errors
    /// Fails when a page request fails or the history runs past the page
    /// limit.
    pub async fn history_turns(&self) -> Result<Vec<TurnId>, Error> {
        let mut paging = History::start();
        loop {
            paging = match paging {
                Paging::Fetch(history) => self.next_page(history).await?,
                Paging::Done(turns) => break Ok(turns),
                Paging::Limit => break Err(Error::HistoryLimit),
            };
        }
    }

    async fn next_page(&self, history: History) -> Result<Paging, Error> {
        let call = Call::ThreadTurns {
            thread: self.id.clone(),
            cursor: history.cursor(),
        };
        let (turns, cursor) =
            history_page(request(&self.server.commands, call).await?)?;
        Ok(history.add(turns, cursor))
    }

    /// Resolves once Codex accepts; `TurnCompleted` follows as an event.
    ///
    /// # Errors
    /// Fails when Codex has exited, does not answer in time, rejects the
    /// request or answers with something we cannot read.
    pub async fn interrupt(&self, turn: TurnId) -> Result<(), Error> {
        let call = Call::TurnInterrupt {
            thread: self.id.clone(),
            turn,
        };
        let _ = request(&self.server.commands, call).await?;
        Ok(())
    }

    /// Answers one of this thread's open approvals exactly once.
    ///
    /// # Errors
    /// Fails when Codex has exited, no approval with `id` is open, or
    /// `decision` does not fit it.
    pub async fn respond(
        &self,
        id: ApprovalId,
        decision: Decision,
    ) -> Result<(), Error> {
        self.server.respond(id, decision).await
    }
}

/// Sends `call` to the actor and waits for Codex's answer.
async fn request(
    commands: &mpsc::Sender<CommandMsg>,
    call: Call,
) -> Result<Value, Error> {
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

fn command(opts: &ServerOptions) -> Command {
    let mut cmd = install::process::command(
        &opts.codex_path,
        &opts.home,
        codex::app_server(opts.home.kind(), opts.credential.as_ref()),
    );
    cmd.current_dir(&opts.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    install::process::no_window(cmd)
}

/// A private home gets the provider in its `config.toml`.
fn configure(
    home: &Home,
    credential: Option<&CodexCredential>,
) -> Result<(), Error> {
    match home {
        Home::Private(dir) => config::write(dir, credential),
        Home::User => Ok(()),
    }
}

/// A server whose process runs, before it has answered `initialize`.
struct Started {
    commands: mpsc::Sender<CommandMsg>,
    events: mpsc::Receiver<CodexEvent>,
    pid: Option<u32>,
}

/// Configures the home, then starts the process and its actor.
fn start(opts: &ServerOptions) -> Result<Started, Error> {
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
    configure(&opts.home, opts.credential.as_ref())?;
    let mut child = command(opts).spawn()?;
    let pid = child.id();
    let ((stdin, stdout), stderr) = child
        .stdin
        .take()
        .zip(child.stdout.take())
        .zip(child.stderr.take())
        .ok_or_else(|| {
            Error::Command(String::from("codex stdio was not piped"))
        })?;
    let (events_tx, events) = mpsc::channel(4096);
    let (commands_tx, commands) = mpsc::channel(64);
    let actor = Actor {
        server: Server::running(),
        process: Some(Running {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            stdout_log,
            last_stderr: tokio::spawn(log_stderr(stderr)),
        }),
        events: events_tx,
    };
    tokio::spawn(actor.run(commands));
    Ok(Started {
        commands: commands_tx,
        events,
        pid,
    })
}

/// Logs stderr while the server runs, retaining its last nonempty line.
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
    .inspect(|line| tracing::debug!(target: "codex::stderr", "{line}"))
    .filter(|line| !line.trim().is_empty())
    .last()
    .await
}

struct Running {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    /// Gets every raw stdout line.
    stdout_log: Option<std::fs::File>,
    last_stderr: tokio::task::JoinHandle<Option<String>>,
}

impl Running {
    async fn read(&mut self) -> Input {
        let read = self.stdout.next_line().await;
        record(self.stdout_log.as_mut(), &read);
        Input::Read(read.map_err(|error| error.to_string()))
    }

    /// How the process ended, and its last stderr line, which says why.
    async fn stop(self) -> (Exit, Option<String>) {
        let status = install::process::stop(self.child, self.stdin).await;
        let refusal = tokio::time::timeout(STDERR_TIMEOUT, self.last_stderr)
            .await
            .ok()
            .and_then(Result::ok)
            .flatten();
        (status, refusal)
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
    server: Server<Reply>,
    /// The process, until it is stopped.
    process: Option<Running>,
    events: mpsc::Sender<CodexEvent>,
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

    /// Steps the server and performs its effects in order, each effect's
    /// own effects before the next.
    async fn feed(&mut self, input: Input) -> Result<(), Stopped> {
        let mut effects = self.server.step(input);
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
        Ok(self.server.step(Input::Wrote { then, outcome }))
    }

    async fn stop(
        &mut self,
        then: After<Reply, Waiter, std::io::Error>,
    ) -> Result<Vec<Effect>, Stopped> {
        let (exit, refusal) = stop_process(self.process.take()).await;
        Ok(self.server.step(Input::Stopped {
            exit,
            refusal,
            then,
        }))
    }

    async fn emit(&self, event: CodexEvent) -> Result<Vec<Effect>, Stopped> {
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

/// The server only stops a running process.
async fn stop_process(running: Option<Running>) -> (Exit, Option<String>) {
    match running {
        Some(process) => process.stop().await,
        None => (Exit::Unknown(String::from("already stopped")), None),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Home, ServerOptions, command};

    #[test]
    fn app_server_argv() -> Result<(), Box<dyn std::error::Error>> {
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
            .map(|a| a.to_str())
            .collect::<Option<Vec<&str>>>()
            .ok_or("an argument is not UTF-8")?;
        assert_eq!(std.get_program(), "/opt/codex");
        assert_eq!(std.get_current_dir(), Some(Path::new("/tmp")));
        assert_eq!(args, ["app-server", "--listen", "stdio://"]);
        Ok(())
    }
}
