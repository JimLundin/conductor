//! The handle a session holds on its agent, whichever provider runs it: a
//! channel pair that speaks the `agents::runtime` protocol. Each provider's
//! shell (`claude-cli`, `codex-cli`) runs a task on the other end, and
//! hands the host a [`Plugin`] whose [`Starter`] starts its agents.

pub mod agent;
mod plugin;

pub use plugin::{Launch, Plugin, Settings};

use std::fmt;
use std::sync::Arc;

use agents::Note;
use agents::runtime::{Command, Event, Outcome};
use proto::Models;
use tokio::sync::mpsc;

/// One agent as its session sees it: where the session sends commands, and
/// where the agent's steps and outcomes arrive. Dropping it ends the
/// agent's task.
#[derive(Debug)]
pub struct Runtime {
    pub commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
}

/// The agent's end of a [`Runtime`].
#[derive(Debug)]
struct Plug {
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
}

/// A runtime and the plug its agent's task takes.
#[must_use]
fn pair() -> (Runtime, Plug) {
    let (commands, received) = mpsc::channel(16);
    let (reported, events) = mpsc::channel(1024);
    (
        Runtime { commands, events },
        Plug {
            commands: received,
            events: reported,
        },
    )
}

/// Why a command fails once the agent's task has gone.
const GONE: &str = "the agent's runtime has stopped";

impl Runtime {
    /// A runtime whose every command fails for `reason`: the host can't run
    /// its provider.
    #[must_use]
    pub fn refusing(reason: String) -> Self {
        let (runtime, plug) = pair();
        tokio::spawn(refuse(plug, reason));
        runtime
    }

    /// Sends `command` and waits for its outcome, passing over the steps
    /// before it.
    pub async fn ask(&mut self, command: Command) -> Outcome {
        let unanswered = command.failed(GONE);
        let _ = self.commands.send(command).await;
        loop {
            match self.events.recv().await {
                Some(Event::Outcome(outcome)) => break outcome,
                Some(Event::Step(_)) => {}
                None => break unanswered,
            }
        }
    }
}

async fn refuse(mut plug: Plug, reason: String) {
    while let Some(command) = plug.commands.recv().await {
        let refused = Event::Outcome(command.failed(&reason));
        let _ = plug.events.send(refused).await;
    }
}

/// Starts one of a provider's agents. Clone it freely.
#[derive(Clone)]
pub struct Starter(Arc<dyn Fn() -> Runtime + Send + Sync>);

impl Starter {
    pub fn new(start: impl Fn() -> Runtime + Send + Sync + 'static) -> Self {
        Self(Arc::new(start))
    }

    #[must_use]
    pub fn start(&self) -> Runtime {
        (self.0)()
    }

    /// What the provider offers with its credential now.
    ///
    /// # Errors
    /// Why it listed none.
    pub async fn models(&self) -> Result<Models, String> {
        self.start().ask(Command::Models).await.models()
    }
}

impl fmt::Debug for Starter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Starter")
    }
}

/// Logs what an agent's core decided to report.
pub fn log(note: &Note) {
    match note {
        Note::Unreadable { provider, error } => {
            tracing::error!(target: "agent", ?provider, %error, "reading stdout failed");
        }
        Note::NotJson {
            provider,
            error,
            line,
        } => {
            tracing::warn!(target: "agent", ?provider, %error, %line, "printed a line that is not JSON");
        }
        Note::Unmapped { provider, frame } => {
            tracing::trace!(target: "agent", ?provider, %frame, "unmapped frame");
        }
        Note::UnmappedOnThread {
            provider,
            thread,
            frame,
        } => {
            tracing::trace!(target: "agent", ?provider, %thread, %frame, "unmapped thread frame");
        }
        Note::ThreadSettings {
            provider,
            thread,
            settings,
        } => {
            tracing::debug!(target: "agent", ?provider, %thread, %settings, "thread settings");
        }
        Note::UnknownResponse { provider, request } => {
            tracing::warn!(target: "agent", ?provider, %request, "response for unknown request");
        }
        Note::Unsupported { provider, request } => {
            tracing::warn!(target: "agent", ?provider, %request, "refused a request conductor does not handle");
        }
        Note::Unwritten {
            provider,
            writing,
            error,
        } => {
            tracing::error!(target: "agent", ?provider, %writing, %error, "write failed");
        }
        Note::SecretDeclined { provider } => {
            tracing::info!(target: "agent", ?provider, "declined a question that asks for a secret answer");
        }
        Note::Answering {
            provider,
            approval,
            thread,
        } => {
            tracing::debug!(target: "agent", ?provider, %approval, %thread, "answering");
        }
        Note::Untested {
            provider,
            found,
            supported,
        } => {
            tracing::warn!(target: "agent", ?provider, %found, %supported, "running an untested version");
        }
        Note::Unversioned { provider } => {
            tracing::warn!(target: "agent", ?provider, "running a CLI that did not report its version");
        }
        Note::GatewayExposed { provider } => {
            tracing::warn!(target: "agent", ?provider, "the CLI's commands can read the gateway token; install bubblewrap and socat to sandbox them");
        }
    }
}
