//! The handle a session holds on its agent, whichever provider runs it: a
//! channel pair that speaks the `agents::runtime` protocol. Each provider's
//! shell (`claude-cli`, `codex-cli`) runs a task on the other end, and
//! hands the host a [`Starter`] for its agents.

use std::fmt;
use std::sync::Arc;

use agents::runtime::{Command, Event, Outcome};
use agents::{Level, Note};
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
pub struct Plug {
    pub commands: mpsc::Receiver<Command>,
    pub events: mpsc::Sender<Event>,
}

/// A runtime and the plug its agent's task takes.
#[must_use]
pub fn pair() -> (Runtime, Plug) {
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

/// Logs what a core decided to report.
pub fn log(note: &Note) {
    let message = &note.message;
    match note.level {
        Level::Error => tracing::error!("{message}"),
        Level::Warn => tracing::warn!("{message}"),
        Level::Info => tracing::info!("{message}"),
        Level::Debug => tracing::debug!("{message}"),
        Level::Trace => tracing::trace!("{message}"),
    }
}
