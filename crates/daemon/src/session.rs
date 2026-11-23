//! One session's actor: it owns the session's agent, so a slow agent call
//! stalls only its own session.
//!
//! The session's decisions are the state machine in [`host::session`]. The
//! actor turns its orders and its agent's events into an [`Input`], steps
//! the state machine, and performs each [`Effect`]; a [`Call`]'s outcome is
//! the next input. The agent is a [`Runtime`], whichever provider runs it.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::path::Path;
use std::time::Duration;

use agents::Step;
use agents::runtime::{Command, Event, Outcome, Start};
use host::finish::{Effect as FinishEffect, Finish, Input as FinishInput};
pub use host::session::Order;
use host::session::{Call, Choice, Compared, Effect, Input, Session as Core};
use proto::{
    ClientName, EventKind, HostPath, HostSeq, Models, SessionId, SessionResult,
    Workspace,
};
use runtime::{Instance, Runtime};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::Instant;

use crate::Stopped;
use crate::home::HomeClone;
use crate::log::{Log, Subscription};
use crate::workspace;

/// What a performed effect hands back: the next input, after a call.
type Performed = Result<Option<Input>, Stopped>;

/// The home and the instruction file already read by this session's CLI.
#[derive(Debug, Clone)]
pub struct Told {
    pub home: HomeClone,
    pub instructions_file: Option<String>,
}

/// A handle to one session's task. Clone it freely.
#[derive(Debug, Clone)]
pub struct Session {
    inbox: mpsc::Sender<SessionMsg>,
}

#[derive(Debug)]
struct SessionMsg {
    request: Request,
    reply: oneshot::Sender<SessionResult>,
}

#[derive(Debug)]
enum Request {
    Order(Order),
    Relaunch(Order, Instance, std::sync::Arc<()>),
    Finish(ClientName),
}

#[derive(Default)]
struct FinishRun {
    subscription: Option<Subscription>,
    until: Option<Instant>,
    landing: Option<(HostSeq, Option<u64>)>,
}

/// What the actor takes next.
enum Happening {
    Request(SessionMsg),
    Agent(Event),
    /// Every handle was dropped, or the agent's runtime has gone.
    Closed,
}

struct Actor {
    id: SessionId,
    inbox: mpsc::Receiver<SessionMsg>,
    runtime: Option<Runtime>,
    told: Told,
    /// Where the session was created, which places it in its project.
    created: HostPath,
    /// The agent's steps that arrived while the actor waited on a call;
    /// it takes them once it has taken the call's outcome.
    held: VecDeque<Step>,
    log: Log,
    /// Where the order being performed is answered.
    reply: Option<oneshot::Sender<SessionResult>>,
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
        let restored = (Some(runtime), None);
        let session =
            Self::restore(id, directory, choice, told, restored, history, log);
        let (reply, _) = oneshot::channel();
        let _ = session.inbox.try_send(SessionMsg {
            request: Request::Order(Order::Start { offered }),
            reply,
        });
        session
    }

    /// Restores the conversation from the log. A session recovered during
    /// bootstrap takes its runtime from its first accepted relaunch.
    pub(crate) fn restore(
        id: SessionId,
        directory: HostPath,
        choice: Choice,
        told: Told,
        agent: (Option<Runtime>, Option<String>),
        history: &[proto::Event],
        log: Log,
    ) -> Self {
        let (runtime, conversation) = agent;
        let (inbox, messages) = mpsc::channel(16);
        let created = directory.clone();
        let core = Core::restore(id, directory, choice, conversation, history);
        let actor = Actor {
            id,
            inbox: messages,
            runtime,
            told,
            created,
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
    pub async fn order(&self, order: Order) -> Result<SessionResult, Stopped> {
        self.request(Request::Order(order)).await
    }

    pub(crate) async fn relaunch(
        &self,
        order: Order,
        adapter: Instance,
        reservation: std::sync::Arc<()>,
    ) -> Result<SessionResult, Stopped> {
        self.request(Request::Relaunch(order, adapter, reservation))
            .await
    }

    pub(crate) async fn finish(
        &self,
        by: ClientName,
    ) -> Result<SessionResult, Stopped> {
        self.request(Request::Finish(by)).await
    }

    async fn request(
        &self,
        request: Request,
    ) -> Result<SessionResult, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.inbox
            .send(SessionMsg { request, reply })
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
                msg = self.inbox.recv() => msg.map_or(Happening::Closed, Happening::Request),
                event = next_event(&mut self.runtime) => event.map_or(Happening::Closed, Happening::Agent),
            };
            now = match happening {
                Happening::Request(msg) => {
                    self.handle_request(now, msg).await?
                }
                other => self.on(now, other).await?,
            };
        }
    }

    async fn handle_request(
        &mut self,
        core: Core,
        msg: SessionMsg,
    ) -> Result<Core, Stopped> {
        match msg {
            SessionMsg {
                request: Request::Finish(by),
                reply,
            } => {
                self.reply = Some(reply);
                self.finish(core, by).await.inspect_err(|_| {
                    tracing::warn!(session = ?self.id, "finishing stopped with the host");
                })
            }
            other => self.on(core, Happening::Request(other)).await,
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
            Happening::Request(SessionMsg { request, reply }) => {
                let order = match request {
                    Request::Order(order) => order,
                    Request::Relaunch(order, adapter, reservation) => {
                        self.runtime.get_or_insert_with(|| adapter.start());
                        self.reply = Some(reply);
                        let input = Input::Order(order);
                        let result = self.react(core, input).await;
                        drop(reservation);
                        return result;
                    }
                    Request::Finish(_) => {
                        return Err(Stopped {});
                    }
                };
                self.reply = Some(reply);
                Input::Order(order)
            }
            Happening::Agent(Event::Step(step)) => Input::Step(step),
            Happening::Agent(Event::Outcome(outcome)) => Input::Agent(outcome),
            Happening::Closed => return Err(Stopped {}),
        };
        self.react(core, input).await
    }

    async fn react(
        &mut self,
        core: Core,
        input: Input,
    ) -> Result<Core, Stopped> {
        let mut now = self.drive(core, input).await?;
        while let Some(step) = self.held.pop_front() {
            now = self.drive(now, Input::Step(step)).await?;
        }
        Ok(now)
    }

    async fn finish(
        &mut self,
        core: Core,
        by: ClientName,
    ) -> Result<Core, Stopped> {
        let (mut finish, first) = Finish::new(self.id, by);
        let mut effects = VecDeque::from([first]);
        let mut now = core;
        let mut watch = FinishRun::default();
        while let Some(effect) = effects.pop_front() {
            let (stepped, input) =
                self.finish_effect(now, effect, &mut watch).await?;
            now = stepped;
            effects.extend(
                input.into_iter().flat_map(|outcome| finish.step(outcome)),
            );
        }
        Ok(now)
    }

    async fn finish_effect(
        &mut self,
        core: Core,
        effect: FinishEffect,
        watch: &mut FinishRun,
    ) -> Result<(Core, Option<FinishInput>), Stopped> {
        match effect {
            FinishEffect::Order(order) => {
                let (reply, mut answer) = oneshot::channel();
                self.reply = Some(reply);
                let stepped = self.react(core, Input::Order(order)).await?;
                Ok((
                    stepped,
                    Some(FinishInput::Ordered(answer.try_recv().ok())),
                ))
            }
            FinishEffect::NextEvent => {
                let (stepped, input) = self.summary_event(core, watch).await?;
                Ok((stepped, Some(input)))
            }
            other => Ok((core, self.perform_finish(other, watch).await?)),
        }
    }

    async fn perform_finish(
        &mut self,
        effect: FinishEffect,
        watch: &mut FinishRun,
    ) -> Result<Option<FinishInput>, Stopped> {
        let input = match effect {
            FinishEffect::History => {
                FinishInput::History(self.log.finish_history(self.id).await?)
            }
            FinishEffect::Emit(kind) => {
                self.log.append(self.id, kind).await?;
                return Ok(None);
            }
            FinishEffect::Reply(answer) => {
                self.answer(answer);
                return Ok(None);
            }
            FinishEffect::Watch { after, seconds } => {
                watch.until =
                    Some(Instant::now() + Duration::from_secs(seconds));
                watch.subscribe(&self.log, after).await?
            }
            FinishEffect::Replay(after) => {
                watch.subscribe(&self.log, after).await?
            }
            FinishEffect::ReadHome => FinishInput::Home(self.told.home.shown()),
            FinishEffect::Ready { workspace, branch } => FinishInput::Ready(
                self.ready_landing(&workspace, &branch, watch).await?,
            ),
            FinishEffect::Land { workspace, branch } => {
                let (attempt, completed) = watch.landing.ok_or(Stopped {})?;
                let landed = match completed {
                    Some(commits) => Ok(commits),
                    None => workspace::land(&workspace, &branch, attempt).await,
                };
                FinishInput::Landed(landed)
            }
            FinishEffect::Push { workspace, branch } => FinishInput::Pushed(
                workspace::push_branch(&workspace, &branch).await,
            ),
            FinishEffect::Note {
                issue,
                summary,
                by,
                commits,
            } => {
                let note_id = watch.landing.map(|(attempt, _)| {
                    let namespace = uuid::Uuid::from_u128(self.id.0);
                    let name = format!("conductor finish note {}", attempt.0);
                    home::issue::EditId(
                        uuid::Uuid::new_v5(&namespace, name.as_bytes())
                            .as_u128(),
                    )
                });
                self.told
                    .home
                    .finished(issue, summary, by, commits, note_id)
                    .await?;
                FinishInput::Noted
            }
            FinishEffect::Order(_) | FinishEffect::NextEvent => {
                return Err(Stopped {});
            }
        };
        Ok(Some(input))
    }

    async fn ready_landing(
        &self,
        workspace: &Workspace,
        branch: &str,
        run: &mut FinishRun,
    ) -> Result<Result<(), String>, Stopped> {
        self.log.barrier().await?;
        let history = self.log.finish_history(self.id).await?;
        let attempt = history
            .iter()
            .rev()
            .find(|event| matches!(event.kind, EventKind::FinishStarted))
            .map(|event| event.seq)
            .ok_or(Stopped {})?;
        match workspace::landing_receipt(workspace, branch, attempt).await {
            Ok(completed) => {
                run.landing = Some((attempt, completed));
                if completed.is_some() {
                    return Ok(Ok(()));
                }
            }
            Err(reason) => return Ok(Err(reason)),
        }
        Ok(crate::home::ready_issue_branch(
            Path::new(&workspace.repository.0),
            branch,
        )
        .await)
    }

    // A concurrent command can stage summary events in its transaction.
    // Keep taking orders so its receipt can publish those events.
    async fn summary_event(
        &mut self,
        mut core: Core,
        watch: &mut FinishRun,
    ) -> Result<(Core, FinishInput), Stopped> {
        loop {
            let subscription = watch.subscription.as_mut().ok_or(Stopped {})?;
            let until = watch.until.ok_or(Stopped {})?;
            let happening = tokio::select! {
                biased;
                () = tokio::time::sleep_until(until) => return Ok((core, FinishInput::TimedOut)),
                next = subscription.live.recv() => return Ok((core, watched(next)?)),
                msg = self.inbox.recv() => msg.map_or(Happening::Closed, Happening::Request),
                event = next_event(&mut self.runtime) => event.map_or(Happening::Closed, Happening::Agent),
            };
            match happening {
                Happening::Request(SessionMsg {
                    request: Request::Finish(_),
                    reply,
                }) => {
                    self.reply = Some(reply);
                    let history = self.log.finish_history(self.id).await?;
                    return Ok((core, FinishInput::RequestedAgain(history)));
                }
                other => core = self.on(core, other).await?,
            }
        }
    }

    fn answer(&mut self, result: SessionResult) {
        if let Some(waiting) = self.reply.take() {
            let _ = waiting.send(result);
        }
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
                self.answer(result);
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
            Call::Snapshot(workspace) => snapshot(workspace, self.id).await,
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
        let instructed = self.instructed(command);
        let unanswered = instructed.failed("the agent's runtime has stopped");
        let Some(runtime) = &self.runtime else {
            return unanswered;
        };
        let _ = runtime.commands.send(instructed).await;
        self.outcome().await.unwrap_or(unanswered)
    }

    fn instructed(&self, command: Command) -> Command {
        match command {
            Command::Start(start) => Command::Start(Start {
                instructions: self.told.instructions_file.as_deref().and_then(
                    |file| {
                        self.told.home.instructions(
                            &self.created,
                            &start.directory,
                            file,
                        )
                    },
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
            match self.runtime.as_mut()?.events.recv().await? {
                Event::Outcome(outcome) => break Some(outcome),
                Event::Step(step) => self.held.push_back(step),
            }
        }
    }
}

async fn next_event(runtime: &mut Option<Runtime>) -> Option<Event> {
    match runtime {
        Some(agent) => agent.events.recv().await,
        None => std::future::pending().await,
    }
}

impl FinishRun {
    async fn subscribe(
        &mut self,
        log: &Log,
        after: HostSeq,
    ) -> Result<FinishInput, Stopped> {
        let until = self.until.ok_or(Stopped {})?;
        let Ok(result) =
            tokio::time::timeout_at(until, log.subscribe(after)).await
        else {
            return Ok(FinishInput::TimedOut);
        };
        let mut subscription = result?;
        let replay = std::mem::take(&mut subscription.replay);
        self.subscription = Some(subscription);
        Ok(FinishInput::Events(replay))
    }
}

fn watched(
    next: Result<proto::Event, broadcast::error::RecvError>,
) -> Result<FinishInput, Stopped> {
    match next {
        Ok(event) => Ok(FinishInput::Events(Vec::from([event]))),
        Err(broadcast::error::RecvError::Lagged(_)) => Ok(FinishInput::Lagged),
        Err(broadcast::error::RecvError::Closed) => Err(Stopped {}),
    }
}

async fn snapshot(workspace: Workspace, session: SessionId) -> Input {
    let taken = workspace::snapshot(&workspace, session).await;
    Input::Snapshotted(taken.map_err(|err| err.to_string()))
}

async fn diff(compared: Compared) -> Input {
    let listed = workspace::diff(
        &compared.workspace,
        compared.snapshot,
        &compared.commit,
        &compared.previous,
    )
    .await;
    Input::Diffed(listed.map_err(|err| err.to_string()))
}
