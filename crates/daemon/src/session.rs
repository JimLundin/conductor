//! One session's actor: it owns the session's agent, so a slow agent call
//! stalls only its own session.
//!
//! The session's decisions are the state machine in [`host::session`]. The
//! actor turns its orders and its agent's events into an [`Input`], steps
//! the state machine, and performs each [`Effect`]; a [`Call`]'s outcome is
//! the next input. The agent is a [`Runtime`], whichever provider runs it.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::fmt::{self, Display};
use std::sync::Arc;

use agents::Step;
use agents::runtime::{Command, Event, Outcome, Start};
pub use host::serve::Order;
use host::session::{Call, Choice, Compared, Effect, Input, Session as Core};
use proto::{
    CheckpointId, CommandResult, HostPath, Models, Provider, SessionId, TaskId,
    Workspace,
};
use runtime::Runtime;
use tokio::sync::{mpsc, oneshot};

use crate::Stopped;
use crate::log::Log;
use crate::workspace;

/// What a performed effect hands back: the next input, after a call.
type Performed = Result<Option<Input>, Stopped>;

type Compose = dyn Fn(&HostPath, Provider) -> Option<String> + Send + Sync;

/// What the host tells a session's agent beside its CLI's own
/// instructions, for a session created in a directory with a provider's
/// CLI (docs/home.md §5).
#[derive(Clone)]
pub struct Instructor(Arc<Compose>);

impl Instructor {
    pub fn new(
        compose: impl Fn(&HostPath, Provider) -> Option<String>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self(Arc::new(compose))
    }
}

impl fmt::Debug for Instructor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Instructor")
    }
}

/// Where a session was created and which provider runs it, and how the
/// host composes its instructions from those each time its agent starts,
/// so they follow the home as it is then.
#[derive(Debug, Clone)]
pub struct Told {
    pub instructor: Instructor,
    pub directory: HostPath,
    pub provider: Provider,
}

/// A handle to one session's task. Clone it freely.
#[derive(Debug, Clone)]
pub struct Session {
    inbox: mpsc::Sender<SessionMsg>,
}

#[derive(Debug)]
struct SessionMsg {
    order: Order,
    reply: oneshot::Sender<CommandResult>,
}

/// What the actor takes next.
enum Happening {
    Order(SessionMsg),
    Agent(Event),
    /// Every handle was dropped, or the agent's runtime has gone.
    Closed,
}

struct Actor {
    id: SessionId,
    inbox: mpsc::Receiver<SessionMsg>,
    runtime: Runtime,
    told: Told,
    /// The agent's steps that arrived while the actor waited on a call;
    /// it takes them once it has taken the call's outcome.
    held: VecDeque<Step>,
    log: Log,
    /// Where the order being performed is answered.
    reply: Option<oneshot::Sender<CommandResult>>,
}

impl Session {
    /// Starts the session's task, which starts the agent in `directory` and
    /// logs `SessionReady` or `SessionClosed`.
    #[must_use]
    pub fn spawn(
        id: SessionId,
        directory: HostPath,
        choice: Choice,
        told: Told,
        agent: (Runtime, Models),
        log: Log,
        history: &[proto::Event],
    ) -> Self {
        let (runtime, offered) = agent;
        let restored = (runtime, None);
        let session =
            Self::restore(id, directory, choice, told, restored, history, log);
        let (reply, _) = oneshot::channel();
        let _ = session.inbox.try_send(SessionMsg {
            order: Order::Start(offered),
            reply,
        });
        session
    }

    /// The session `history` logged, with its agent's runtime and the
    /// conversation the host saved for it, and no agent running.
    pub(crate) fn restore(
        id: SessionId,
        directory: HostPath,
        choice: Choice,
        told: Told,
        agent: (Runtime, Option<String>),
        history: &[proto::Event],
        log: Log,
    ) -> Self {
        let (runtime, conversation) = agent;
        let (inbox, messages) = mpsc::channel(16);
        let core = Core::restore(id, directory, choice, conversation, history);
        let actor = Actor {
            id,
            inbox: messages,
            runtime,
            told,
            held: VecDeque::new(),
            log,
            reply: None,
        };
        tokio::spawn(actor.run(core));
        Self { inbox }
    }

    /// Hands the session an order and waits for its answer.
    ///
    /// # Errors
    ///
    /// Fails if the session has stopped.
    pub async fn order(&self, order: Order) -> Result<CommandResult, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.inbox
            .send(SessionMsg { order, reply })
            .await
            .map_err(|_| Stopped {})?;
        answer.await.map_err(|_| Stopped {})
    }
}

impl Actor {
    async fn run(mut self, core: Core) -> Result<Infallible, Stopped> {
        let mut now = core;
        loop {
            let happening = tokio::select! {
                biased;
                msg = self.inbox.recv() => msg.map_or(Happening::Closed, Happening::Order),
                event = self.runtime.events.recv() => event.map_or(Happening::Closed, Happening::Agent),
            };
            now = self.on(now, happening).await?;
        }
    }

    /// Steps the session with what happened, then with each step the agent
    /// reported while the session waited on it.
    async fn on(
        &mut self,
        core: Core,
        happening: Happening,
    ) -> Result<Core, Stopped> {
        let input = match happening {
            Happening::Order(msg) => self.take(msg),
            Happening::Agent(Event::Step(step)) => Input::Step(step),
            Happening::Agent(Event::Outcome(outcome)) => Input::Agent(outcome),
            Happening::Closed => return Err(Stopped {}),
        };
        let mut now = self.drive(core, input).await?;
        while let Some(step) = self.held.pop_front() {
            now = self.drive(now, Input::Step(step)).await?;
        }
        Ok(now)
    }

    fn take(&mut self, msg: SessionMsg) -> Input {
        self.reply = Some(msg.reply);
        // A conversation an undo forks gets a fresh id.
        Input::Order(msg.order.session(uuid::Uuid::new_v4().to_string()))
    }

    /// Steps the session with `input`, and again with each call's outcome,
    /// until it waits for nothing.
    async fn drive(
        &mut self,
        core: Core,
        input: Input,
    ) -> Result<Core, Stopped> {
        let mut now = core;
        let mut next = Some(input);
        while let Some(input) = next {
            let (stepped, effects) = now.step(input);
            now = stepped;
            next = self.perform_all(effects).await?;
        }
        Ok(now)
    }

    async fn perform_all(&mut self, effects: Vec<Effect>) -> Performed {
        let mut next = None;
        for effect in effects {
            next = self.perform(effect).await?;
        }
        Ok(next)
    }

    async fn perform(&mut self, effect: Effect) -> Performed {
        match effect {
            Effect::Emit(kind) => {
                self.log.append(self.id, kind).await.map(|()| None)
            }
            Effect::Conversation(id) => {
                self.log.conversation(self.id, id).await.map(|()| None)
            }
            Effect::Barrier => self.log.barrier().await.map(|()| None),
            Effect::Trace(note) => {
                let session = ulid::Ulid::from(self.id.0);
                tracing::info!(%session, "{note}");
                Ok(None)
            }
            Effect::Reply(result) => {
                if let Some(waiting) = self.reply.take() {
                    let _ = waiting.send(result);
                }
                Ok(None)
            }
            Effect::Call(call) => self.call(call).await,
        }
    }

    async fn call(&mut self, call: Call) -> Performed {
        let input = match call {
            Call::Record(kind) => {
                Input::Recorded(self.log.record(self.id, kind).await?.seq)
            }
            Call::History => Input::History(self.log.history().await?),
            Call::Mint(count) => Input::Minted(mint(count)),
            Call::Exists(path) => {
                Input::Exists(std::fs::read_dir(&path.0).is_ok())
            }
            Call::Snapshot {
                workspace,
                checkpoint,
                previous,
            } => snapshot(workspace, self.id, checkpoint, previous).await,
            Call::Preflight { workspace, commit } => {
                done(workspace::preflight(&workspace, &commit).await)
            }
            Call::Restore { workspace, commit } => {
                done(workspace::restore(&workspace, &commit).await)
            }
            Call::Diff(compared) => diff(*compared).await,
            Call::Remove(workspace) => Input::Removed(
                workspace::remove(&workspace)
                    .await
                    .map_err(|err| err.to_string()),
            ),
            Call::Agent(command) => Input::Agent(self.ask(command).await),
        };
        Ok(Some(input))
    }

    /// Sends the agent `command` and waits for its outcome. A start takes
    /// the session's instructions as the home has them now.
    async fn ask(&mut self, command: Command) -> Outcome {
        let told = self.told(command);
        let unanswered = told.failed("the agent's runtime has stopped");
        let _ = self.runtime.commands.send(told).await;
        self.outcome().await.unwrap_or(unanswered)
    }

    fn told(&self, command: Command) -> Command {
        match command {
            Command::Start(start) => Command::Start(Start {
                instructions: (self.told.instructor.0)(
                    &self.told.directory,
                    self.told.provider,
                ),
                ..start
            }),
            other => other,
        }
    }

    /// The outcome the agent reports next. The steps it reports before it
    /// wait until the session has taken the outcome.
    async fn outcome(&mut self) -> Option<Outcome> {
        loop {
            match self.runtime.events.recv().await? {
                Event::Outcome(outcome) => break Some(outcome),
                Event::Step(step) => self.held.push_back(step),
            }
        }
    }
}

fn mint(count: usize) -> Vec<TaskId> {
    (0..count)
        .map(|_| TaskId(ulid::Ulid::new().into()))
        .collect()
}

/// The outcome of a call that answers nothing else.
fn done<T, E: Display>(called: Result<T, E>) -> Input {
    Input::Done(called.map(drop).map_err(|err| err.to_string()))
}

async fn snapshot(
    workspace: Workspace,
    session: SessionId,
    checkpoint: CheckpointId,
    previous: Option<String>,
) -> Input {
    let taken = workspace::snapshot(
        &workspace,
        session,
        checkpoint,
        previous.as_deref(),
    )
    .await;
    Input::Snapshotted(taken.map_err(|err| err.to_string()))
}

async fn diff(compared: Compared) -> Input {
    let listed = workspace::diff(
        &compared.workspace,
        &compared.checkpoint,
        &compared.parent,
    )
    .await;
    Input::Diffed(listed.map_err(|err| err.to_string()))
}
