//! One session's actor: it owns the agent, so a slow agent call stalls
//! only its own session.
//!
//! The session's decisions are the state machine in [`host::session`]. The
//! actor turns its orders and its agent's events into an [`Input`], steps
//! the state machine, and performs each [`Effect`]; a [`Call`]'s outcome is
//! the next input.

use std::convert::Infallible;
use std::fmt::Display;
use std::future::{Future, pending, ready};
use std::path::{Path, PathBuf};
use std::pin::Pin;

use agents::{Reply, Step};
use claude::{ClaudeEvent, PermissionMode, PermissionRequestId, claude_answer};
use claude::{SettingSource, VersionPolicy};
use claude_cli::{ClaudeOptions, ClaudeSession};
use codex::{
    ApprovalId, ApprovalPolicy, Sandbox, ThreadId, ThreadOptions, TurnId,
    codex_answer,
};
use codex_cli::CodexThread;
use host::session::{
    Call, Choice, ClaudeResume, Compared, Effect, Input, Launching, Resume,
    Session as Core, Unstarted,
};
use install::Home;
use n0_future::future::race;
use proto::{
    CheckpointId, ClaudeCredential, ClientName, CommandResult, Effort,
    HostPath, Models, Provider, RequestId, SessionId, TaskId, Workspace,
};
use tokio::sync::{mpsc, oneshot};

use crate::Stopped;
use crate::codex_hub::{CodexHub, Routed};
use crate::log::Log;
use crate::workspace;

/// A future the actor runs to its end before it takes the next input.
type Doing<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What a performed effect hands back: the next input, after a call.
type Performed = Result<Option<Input>, Stopped>;

/// How the host runs Claude. Built once in `main`.
#[derive(Debug, Clone)]
pub struct ClaudeLaunch {
    pub claude_path: PathBuf,
    pub home: Home,
    /// The stored credential, which the registry puts in when it starts a
    /// session.
    pub credential: Option<ClaudeCredential>,
}

/// How the host runs Codex: threads on the hub's shared app-server.
#[derive(Debug, Clone)]
pub struct CodexLaunch {
    pub hub: CodexHub,
}

/// What a session task needs to start its agent.
#[derive(Debug, Clone)]
pub enum Launch {
    Claude(ClaudeLaunch),
    Codex(CodexLaunch),
}

impl Launch {
    #[must_use]
    pub const fn provider(&self) -> Provider {
        match self {
            Self::Claude(_) => Provider::Claude,
            Self::Codex(_) => Provider::Codex,
        }
    }
}

/// A handle to one session's task. Clone it freely.
#[derive(Debug, Clone)]
pub struct Session {
    inbox: mpsc::Sender<SessionMsg>,
}

/// A command for one session, already routed to it.
#[derive(Debug)]
pub enum Order {
    Send {
        text: String,
        by: ClientName,
    },
    Interrupt,
    Diff(CheckpointId),
    Undo(CheckpointId, Launch),
    Reply {
        request: RequestId,
        reply: Reply,
        by: ClientName,
    },
    /// Starts the agent with this launch, resuming the conversation if the
    /// session has one: when the session starts, and on `Continue`. The
    /// models are what the provider offers now.
    Start(Launch, Models),
    /// Sets the effort of later turns, if the session's model in these
    /// models takes it. The agent gets it when the next turn starts.
    Effort(Effort, Models),
}

impl Order {
    /// The order as data, and the launch it brings. A Claude fork an undo
    /// makes gets a fresh id.
    fn split(self) -> (host::session::Order, Option<Launch>) {
        use host::session::Order as Core;
        match self {
            Self::Send { text, by } => (Core::Send { text, by }, None),
            Self::Interrupt => (Core::Interrupt, None),
            Self::Diff(checkpoint) => (Core::Diff(checkpoint), None),
            Self::Undo(checkpoint, launch) => (
                Core::Undo {
                    checkpoint,
                    provider: launch.provider(),
                    fork: uuid::Uuid::new_v4().to_string(),
                },
                Some(launch),
            ),
            Self::Reply { request, reply, by } => {
                (Core::Reply { request, reply, by }, None)
            }
            Self::Start(launch, offered) => (
                Core::Start {
                    provider: launch.provider(),
                    offered,
                },
                Some(launch),
            ),
            Self::Effort(effort, offered) => {
                (Core::Effort(effort, offered), None)
            }
        }
    }
}

#[derive(Debug)]
struct SessionMsg {
    order: Order,
    reply: oneshot::Sender<CommandResult>,
}

/// The agent and where its events arrive.
enum Agent {
    Claude {
        session: ClaudeSession,
        events: mpsc::Receiver<ClaudeEvent>,
    },
    Codex {
        thread: CodexThread,
        events: mpsc::Receiver<Routed>,
    },
    /// No agent runs: it exited, the host stopped it, or it never started.
    Down,
}

/// What the actor takes next.
enum Happening {
    Order(SessionMsg),
    Steps(Vec<Step>),
    Input(Input),
    /// Every handle was dropped.
    Closed,
}

struct Actor {
    id: SessionId,
    inbox: mpsc::Receiver<SessionMsg>,
    agent: Agent,
    log: Log,
    /// The launch the order being performed brought.
    launch: Option<Launch>,
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
        prepared: (Launch, Models),
        log: Log,
        history: &[proto::Event],
    ) -> Self {
        let session = Self::restore(id, directory, choice, None, history, log);
        let (reply, _) = oneshot::channel();
        let (launch, offered) = prepared;
        let _ = session.inbox.try_send(SessionMsg {
            order: Order::Start(launch, offered),
            reply,
        });
        session
    }

    pub(crate) fn restore(
        id: SessionId,
        directory: HostPath,
        choice: Choice,
        conversation: Option<String>,
        history: &[proto::Event],
        log: Log,
    ) -> Self {
        let (inbox, messages) = mpsc::channel(16);
        let core = Core::restore(id, directory, choice, conversation, history);
        let actor = Actor {
            id,
            inbox: messages,
            agent: Agent::Down,
            log,
            launch: None,
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
        self.ask(|reply| SessionMsg { order, reply }).await
    }

    async fn ask<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> SessionMsg,
    ) -> Result<T, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.inbox
            .send(build(reply))
            .await
            .map_err(|_| Stopped {})?;
        answer.await.map_err(|_| Stopped {})
    }
}

/// What `launch` offers with the credential in it. Claude lists its models
/// as a session would see them, started in a directory of its own under
/// `data_dir`; Codex asks the hub's app-server.
///
/// # Errors
///
/// Says why when the CLI can't be started or asked.
pub fn list_models(
    launch: Launch,
    data_dir: &Path,
) -> Doing<'static, Result<Models, String>> {
    match launch {
        Launch::Claude(claude) => {
            Box::pin(claude_models(claude, data_dir.join("probe")))
        }
        Launch::Codex(CodexLaunch { hub }) => Box::pin(codex_models(hub)),
    }
}

/// What a Claude session offers, from its `initialize` response: the CLI
/// is started in `directory` and stopped again.
async fn claude_models(
    launch: ClaudeLaunch,
    directory: PathBuf,
) -> Result<Models, String> {
    std::fs::create_dir_all(&directory).map_err(|err| err.to_string())?;
    let (session, _events) = ClaudeSession::spawn(ClaudeOptions {
        claude_path: launch.claude_path,
        home: launch.home,
        credential: launch.credential,
        cwd: directory,
        model: None,
        effort: None,
        permission_mode: PermissionMode::Default,
        resume: claude::Resume::Fresh,
        project_name: None,
        setting_sources: None,
        stdout_log: None,
        versions: VersionPolicy::AllowUntested,
    })
    .await
    .map_err(|err| err.to_string())?;
    let models = session.models().clone();
    let _ = session.shutdown().await;
    Ok(models)
}

async fn codex_models(hub: CodexHub) -> Result<Models, String> {
    hub.models().await.map_err(|err| err.to_string())
}

impl Actor {
    async fn run(mut self, core: Core) -> Result<Infallible, Stopped> {
        let mut now = core;
        loop {
            let happening = self.next().await;
            now = self.on(now, happening).await?;
        }
    }

    /// The next order or agent event, whichever comes first.
    fn next(&mut self) -> impl Future<Output = Happening> + Send + '_ {
        let Self { inbox, agent, .. } = self;
        race(received(inbox), agent.next())
    }

    fn on(
        &mut self,
        core: Core,
        happening: Happening,
    ) -> Doing<'_, Result<Core, Stopped>> {
        match happening {
            Happening::Order(msg) => Box::pin(self.order(core, msg)),
            Happening::Steps(steps) => Box::pin(self.steps(core, steps)),
            Happening::Input(input) => Box::pin(self.drive(core, input)),
            Happening::Closed => Box::pin(ready(Err(Stopped {}))),
        }
    }

    async fn order(
        &mut self,
        core: Core,
        msg: SessionMsg,
    ) -> Result<Core, Stopped> {
        let (order, launch) = msg.order.split();
        self.reply = Some(msg.reply);
        self.launch = launch;
        self.drive(core, Input::Order(order)).await
    }

    /// Takes the steps of one agent event in order.
    async fn steps(
        &mut self,
        core: Core,
        steps: Vec<Step>,
    ) -> Result<Core, Stopped> {
        let mut now = core;
        for step in steps {
            now = self.drive(now, Input::Step(step)).await?;
        }
        Ok(now)
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

    fn perform(&mut self, effect: Effect) -> Doing<'_, Performed> {
        match effect {
            Effect::Emit(kind) => {
                Box::pin(logged(self.log.append(self.id, kind)))
            }
            Effect::Conversation(id) => {
                Box::pin(logged(self.log.conversation(self.id, id)))
            }
            Effect::Barrier => Box::pin(logged(self.log.barrier())),
            Effect::Trace(note) => {
                trace(self.id, note);
                Box::pin(ready(Ok(None)))
            }
            Effect::Release => {
                self.agent = Agent::Down;
                Box::pin(ready(Ok(None)))
            }
            Effect::Reply(result) => {
                answer(self.reply.take(), result);
                Box::pin(ready(Ok(None)))
            }
            Effect::Call(call) => self.call(call),
        }
    }

    fn call(&mut self, call: Call) -> Doing<'_, Performed> {
        match call {
            Call::Record(kind) => Box::pin(self.record(kind)),
            Call::History => Box::pin(self.history()),
            Call::Mint(count) => answered(Input::Minted(mint(count))),
            Call::Exists(path) => {
                answered(Input::Exists(std::fs::read_dir(&path.0).is_ok()))
            }
            Call::Snapshot {
                workspace,
                checkpoint,
                previous,
            } => Box::pin(snapshot(workspace, self.id, checkpoint, previous)),
            Call::Preflight { workspace, commit } => {
                Box::pin(preflight(workspace, commit))
            }
            Call::Restore { workspace, commit } => {
                Box::pin(restore(workspace, commit))
            }
            Call::Diff(compared) => Box::pin(diff(*compared)),
            Call::Start(launching) => Box::pin(self.start(launching)),
            Call::Shutdown => self.shutdown(),
            agent => self.agent.call(agent),
        }
    }

    async fn record(&self, kind: proto::EventKind) -> Performed {
        let event = self.log.record(self.id, kind).await?;
        Ok(Some(Input::Recorded(event.seq)))
    }

    async fn history(&self) -> Performed {
        Ok(Some(Input::History(self.log.history().await?)))
    }

    /// Starts an agent with the order's launch, in place of the one running
    /// if it starts.
    async fn start(&mut self, launching: Launching) -> Performed {
        let started = launch(self.id, self.launch.clone(), launching).await;
        Ok(Some(Input::Started(started.map(
            |(agent, conversation)| {
                self.agent = agent;
                conversation
            },
        ))))
    }

    fn shutdown(&mut self) -> Doing<'static, Performed> {
        match std::mem::replace(&mut self.agent, Agent::Down) {
            Agent::Claude { session, .. } => Box::pin(done(session.shutdown())),
            Agent::Codex { .. } | Agent::Down => answered(Input::Done(Ok(()))),
        }
    }
}

async fn received(inbox: &mut mpsc::Receiver<SessionMsg>) -> Happening {
    inbox
        .recv()
        .await
        .map_or(Happening::Closed, Happening::Order)
}

fn answer(
    reply: Option<oneshot::Sender<CommandResult>>,
    result: CommandResult,
) {
    if let Some(waiting) = reply {
        let _ = waiting.send(result);
    }
}

/// Logs through the `log` facade, which the subscriber forwards: tracing's
/// own macros expand past the shell's complexity limit.
fn trace(session: SessionId, note: &str) {
    log::info!("session {}: {note}", session.0);
}

fn mint(count: usize) -> Vec<TaskId> {
    (0..count)
        .map(|_| TaskId(ulid::Ulid::new().into()))
        .collect()
}

fn answered(input: Input) -> Doing<'static, Performed> {
    Box::pin(ready(Ok(Some(input))))
}

async fn logged(
    appended: impl Future<Output = Result<(), Stopped>>,
) -> Performed {
    appended.await.map(|()| None)
}

/// The outcome of a call that answers nothing else.
async fn done<T, E: Display>(
    called: impl Future<Output = Result<T, E>>,
) -> Performed {
    let outcome = called.await;
    Ok(Some(Input::Done(
        outcome.map(drop).map_err(|err| err.to_string()),
    )))
}

async fn snapshot(
    workspace: Workspace,
    session: SessionId,
    checkpoint: CheckpointId,
    previous: Option<String>,
) -> Performed {
    let taken = workspace::snapshot(
        &workspace,
        session,
        checkpoint,
        previous.as_deref(),
    )
    .await;
    Ok(Some(Input::Snapshotted(
        taken.map_err(|err| err.to_string()),
    )))
}

async fn preflight(workspace: Workspace, commit: String) -> Performed {
    done(workspace::preflight(&workspace, &commit)).await
}

async fn restore(workspace: Workspace, commit: String) -> Performed {
    done(workspace::restore(&workspace, &commit)).await
}

async fn diff(compared: Compared) -> Performed {
    let listed = workspace::diff(
        &compared.workspace,
        &compared.checkpoint,
        &compared.parent,
    )
    .await;
    Ok(Some(Input::Diffed(listed.map_err(|err| err.to_string()))))
}

/// Starts the agent `launching` resumes, with `launch`.
fn launch(
    id: SessionId,
    launch: Option<Launch>,
    launching: Launching,
) -> Doing<'static, Result<(Agent, String), Unstarted>> {
    let Launching {
        directory,
        choice,
        resume,
    } = launching;
    match (launch, resume) {
        (Some(Launch::Claude(claude)), Resume::Claude(from)) => Box::pin(
            start_claude(claude_options(id, claude, &directory, &choice, from)),
        ),
        // Paseo's preset: Codex asks before leaving the workspace.
        (Some(Launch::Codex(CodexLaunch { hub })), Resume::Codex(thread)) => {
            let options = ThreadOptions {
                cwd: directory.0,
                model: choice.model.map(|model| model.0),
                effort: choice.effort,
                approval_policy: ApprovalPolicy::OnRequest,
                sandbox: Sandbox::WorkspaceWrite,
            };
            Box::pin(start_codex(hub, options, thread.map(ThreadId)))
        }
        _ => Box::pin(ready(Err(Unstarted {
            message: String::from("the order brought no launch for the agent"),
            missing: None,
        }))),
    }
}

fn claude_options(
    id: SessionId,
    claude: ClaudeLaunch,
    directory: &HostPath,
    choice: &Choice,
    resume: ClaudeResume,
) -> ClaudeOptions {
    ClaudeOptions {
        claude_path: claude.claude_path,
        home: claude.home,
        credential: claude.credential,
        cwd: PathBuf::from(&directory.0),
        model: choice.model.clone().map(|model| model.0),
        effort: choice.effort,
        permission_mode: PermissionMode::Default,
        resume: claude_resume(resume),
        project_name: Some(ulid::Ulid::from(id.0).to_string()),
        // The user level is the host's private home (§6.2).
        setting_sources: Some(Vec::from([
            SettingSource::User,
            SettingSource::Project,
            SettingSource::Local,
        ])),
        stdout_log: None,
        versions: VersionPolicy::AllowUntested,
    }
}

fn claude_resume(resume: ClaudeResume) -> claude::Resume {
    match resume {
        ClaudeResume::Fresh => claude::Resume::Fresh,
        ClaudeResume::Session(session) => {
            claude::Resume::Session(claude::SessionId(session))
        }
        ClaudeResume::Fork {
            source,
            through,
            target,
        } => claude::Resume::Fork {
            session: claude::SessionId(source),
            through: claude::MessageId(through),
            target: claude::SessionId(target),
        },
    }
}

async fn start_claude(
    options: ClaudeOptions,
) -> Result<(Agent, String), Unstarted> {
    let (session, events) = ClaudeSession::spawn(options)
        .await
        .map_err(|err| unstarted(&err))?;
    let conversation = session.id().0.clone();
    Ok((Agent::Claude { session, events }, conversation))
}

/// Why Claude didn't start, and the session it found missing.
fn unstarted(err: &claude_cli::Error) -> Unstarted {
    let missing = match err {
        claude_cli::Error::MissingSession(session) => Some(session.0.clone()),
        _ => None,
    };
    Unstarted {
        message: err.to_string(),
        missing,
    }
}

async fn start_codex(
    hub: CodexHub,
    options: ThreadOptions,
    resume: Option<ThreadId>,
) -> Result<(Agent, String), Unstarted> {
    let (thread, events) =
        hub.start_thread(options, resume)
            .await
            .map_err(|err| Unstarted {
                message: err.to_string(),
                missing: None,
            })?;
    let conversation = thread.id.0.clone();
    Ok((Agent::Codex { thread, events }, conversation))
}

impl Agent {
    fn next(&mut self) -> Doing<'_, Happening> {
        match self {
            Self::Claude { events, .. } => Box::pin(claude_events(events)),
            Self::Codex { events, .. } => Box::pin(codex_events(events)),
            // With no agent, the session only takes orders.
            Self::Down => Box::pin(pending()),
        }
    }

    fn call(&self, call: Call) -> Doing<'_, Performed> {
        match (self, call) {
            (Self::Claude { session, .. }, Call::SetEffort(effort)) => {
                Box::pin(done(session.set_effort(effort)))
            }
            (Self::Claude { session, .. }, Call::Send { text, .. }) => {
                Box::pin(done(session.send_user(text)))
            }
            (Self::Codex { thread, .. }, Call::Send { text, effort }) => {
                Box::pin(done(thread.send_turn(text, effort)))
            }
            (Self::Claude { session, .. }, Call::Interrupt(_)) => {
                Box::pin(done(session.interrupt()))
            }
            (Self::Codex { thread, .. }, Call::Interrupt(turn)) => {
                Box::pin(done(thread.interrupt(TurnId(turn.0))))
            }
            (
                Self::Claude { session, .. },
                Call::Answer { native, reply, by },
            ) => {
                let decision = claude_answer(reply, &by);
                Box::pin(done(
                    session.respond(PermissionRequestId(native.0), decision),
                ))
            }
            (
                Self::Codex { thread, .. },
                Call::Answer { native, reply, .. },
            ) => Box::pin(done(
                thread.respond(ApprovalId(native.0), codex_answer(reply)),
            )),
            (Self::Codex { thread, .. }, Call::Turns) => {
                Box::pin(turns(thread))
            }
            (Self::Codex { thread, .. }, Call::Revert(turn)) => {
                Box::pin(done(thread.revert(TurnId(turn))))
            }
            (_, other) => answered(not_running(&other)),
        }
    }
}

/// What a call the agent can't take answers: it is not running, or not
/// the provider the call is for.
fn not_running(call: &Call) -> Input {
    let reason = String::from("the agent is not running");
    match call {
        Call::Turns => Input::Turns(Err(reason)),
        _ => Input::Done(Err(reason)),
    }
}

async fn claude_events(events: &mut mpsc::Receiver<ClaudeEvent>) -> Happening {
    events
        .recv()
        .await
        .map_or(Happening::Input(Input::Exited), |event| {
            Happening::Steps(claude::claude_steps(event))
        })
}

async fn codex_events(events: &mut mpsc::Receiver<Routed>) -> Happening {
    events
        .recv()
        .await
        .map_or(Happening::Input(Input::Exited), routed)
}

fn routed(routed: Routed) -> Happening {
    match routed {
        Routed::Codex(event) => Happening::Steps(codex::codex_steps(event)),
        Routed::Stopped { reason } => Happening::Input(Input::Stopped(reason)),
    }
}

async fn turns(thread: &CodexThread) -> Performed {
    let listed = thread.history_turns().await;
    Ok(Some(Input::Turns(
        listed
            .map(|turns| turns.into_iter().map(|turn| turn.0).collect())
            .map_err(|err| err.to_string()),
    )))
}
