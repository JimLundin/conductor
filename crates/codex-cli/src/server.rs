//! The Codex handle and handshake over the shared line-JSON process runner.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use codex::CodexCredential;
use proto::Effort;
use serde_json::Value;

use agents::Exit;
use install::Home;
use install::line_json::{self, Handle, Options, StderrPolicy};
use tokio::process::Command;
use tokio::sync::mpsc;

use super::config;
use codex::{
    ApprovalId, Call, CallRequest, CodexEvent, Decision, Failure, History,
    Paging, Problem, Server, ThreadId, ThreadOptions, TurnId, history_page,
    models, thread_started, turn_id, user_agent,
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
            Failure::Protocol(Problem::Rpc { method, message }) => {
                Self::Rpc { method, message }
            }
            Failure::Protocol(Problem::UnknownApproval(id)) => {
                Self::UnknownApproval(id)
            }
            Failure::Protocol(Problem::DecisionNotApplicable(id)) => {
                Self::DecisionNotApplicable(id)
            }
        }
    }
}

type CommandMsg = CallRequest<line_json::Reply<Error>>;

/// The host's app-server. Cloning yields another handle to the same process.
#[derive(Clone)]
pub struct CodexServer {
    commands: Handle<CommandMsg>,
    user_agent: String,
}

/// A thread on a [`CodexServer`]. Its events arrive on the server's event
/// stream, tagged with [`CodexThread::id`].
#[derive(Clone)]
pub struct CodexThread {
    pub id: ThreadId,
    #[cfg(all(test, unix))]
    settings: codex::ThreadSettings,
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
        config::write(&opts.home, opts.credential.as_ref())?;
        Self::open(opts).await
    }

    pub async fn open(
        opts: ServerOptions,
    ) -> Result<(Self, mpsc::Receiver<CodexEvent>), Error> {
        let line_json::Started {
            handle: commands,
            events,
            ..
        } = start(&opts)?;
        let initialize = request(&commands, Call::Initialize).await?;
        let server = Self {
            commands,
            user_agent: user_agent(initialize)?,
        };
        Ok((server, events))
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
        #[cfg(not(all(test, unix)))]
        let _ = settings;
        Ok(CodexThread {
            id,
            #[cfg(all(test, unix))]
            settings,
            server: self.clone(),
        })
    }

    /// Answers an open approval or user-input request exactly once.
    ///
    /// # Errors
    /// Fails when Codex has exited, no approval with `id` is open, or
    /// `decision` does not fit it.
    async fn respond(
        &self,
        id: ApprovalId,
        decision: Decision,
    ) -> Result<(), Error> {
        let _ = self
            .commands
            .ask(|reply| CommandMsg::Respond {
                id,
                decision,
                reply,
            })
            .await
            .map_err(|_| Error::Exited)??;
        Ok(())
    }

    /// Closes stdin, waits briefly for a clean exit, then kills.
    ///
    /// # Errors
    /// Fails when the server's actor has already stopped.
    pub async fn shutdown(&self) -> Result<Exit, Error> {
        self.commands.shutdown().await.map_err(|_| Error::Exited)
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
    commands: &Handle<CommandMsg>,
    call: Call,
) -> Result<Value, Error> {
    let method = String::from(call.method());
    tokio::time::timeout(
        REQUEST_TIMEOUT,
        commands.ask(|reply| CommandMsg::Request { call, reply }),
    )
    .await
    .map_err(|_| Error::Timeout(method))?
    .map_err(|_| Error::Exited)?
}

fn command(opts: &ServerOptions) -> Command {
    let mut cmd = install::process::command(
        &opts.codex_path,
        &opts.home,
        codex::app_server(opts.home.kind(), opts.credential.as_ref()),
    );
    cmd.current_dir(&opts.cwd);
    cmd
}

fn start(
    opts: &ServerOptions,
) -> Result<line_json::Started<CommandMsg, CodexEvent>, Error> {
    let mut driver = Server::default();
    Ok(line_json::start(
        command(opts),
        Options {
            stdout_log: None,
            stderr: StderrPolicy::LastNonempty(STDERR_TIMEOUT),
            log_stderr: |line| tracing::debug!(target: "codex::stderr", "{line}"),
            log_note: runtime::log,
            commands: 64,
            events: 4096,
        },
        move |input| driver.step(input),
    )?)
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

#[cfg(test)]
#[cfg(unix)]
mod replay;
