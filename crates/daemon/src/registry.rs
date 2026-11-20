//! The registry's actor: the one place a `StartSession` and a `Continue`
//! are checked, a session is found by its id, the host's instances
//! change, and an instance signs in from the app.
//!
//! Its decisions are `host::registry`'s and `host::sign_in`'s; the actor
//! keeps the store, the sessions' tasks, the instances' refreshes and
//! starts and the sign-ins' runs, and performs what the cores decide.

use std::convert::Infallible;
use std::path::Path;

use agents::runtime::Command;
use host::registry::{
    Change, Job, Launch, Listing, Refreshed, Started, Starting,
};
use host::restore::{Created, Recovery, Step, events_of};
use host::serve::{Configure, SignInOrder};
use host::sign_in::{Effect as SignInEffect, Key, SignIns};
use proto::{
    Catalog, Checkpoint, CheckpointId, ClientName, CommandResult, Credential,
    Effort, Event, EventKind, HistoryAnchor, HostPath, InstanceId, Instances,
    ModelId, Models, Provider, Rejection, SessionId, SignIn, UndoAvailability,
    Workspace,
};
use runtime::{Runtime, Starter};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinSet;

use crate::Stopped;
use crate::credentials::{Store, Stored};
use crate::home::{HomeClone, Placement};
use crate::log::Log;
use crate::session::{Order, Session, Told};
use crate::sign_in::{Report, Runs};
use crate::start::Providers;

/// A subscriber this far behind the sign-ins is dropped and resubscribes.
const SIGN_IN_BUFFER: usize = 64;

/// The sign-ins running now, and the steps they take from then on.
pub type Watch = (Vec<SignIn>, broadcast::Receiver<SignIn>);

pub type Launcher = host::registry::Launcher<Starter>;

type Reply = oneshot::Sender<CommandResult>;
type Core = host::registry::Registry<Starter, Session, Reply>;
type Effect = host::registry::Effect<Starter, Reply>;

#[derive(Debug, Clone)]
pub struct Registry {
    inbox: mpsc::Sender<RegistryMsg>,
}

enum RegistryMsg {
    Refreshed(Refreshed),
    Started(Started<Starter>),
    Start {
        instance: InstanceId,
        placed: Placement,
        model: Option<ModelId>,
        effort: Option<Effort>,
        by: ClientName,
        reply: Reply,
    },
    Continue {
        session: SessionId,
        reply: oneshot::Sender<Result<(Session, Models), Rejection>>,
    },
    Find {
        session: SessionId,
        reply: oneshot::Sender<Option<Session>>,
    },
    Instances {
        reply: oneshot::Sender<(Instances, Catalog)>,
    },
    Configure {
        configure: Configure,
        reply: Reply,
    },
    SignIn {
        order: SignInOrder,
        reply: Reply,
    },
    Watch {
        reply: oneshot::Sender<Watch>,
    },
    Run(Key, Report),
}

struct RegistryActor {
    log: Log,
    store: Store,
    core: Core,
    providers: Providers,
    home: HomeClone,
    refreshes: JoinSet<Refreshed>,
    starts: JoinSet<Started<Starter>>,
    sign_ins: SignIns,
    runs: Runs,
    live: broadcast::Sender<SignIn>,
}

impl Registry {
    pub async fn spawn(
        log: Log,
        store: Store,
        stored: &Stored,
        providers: Providers,
        home: HomeClone,
    ) -> Result<Self, Stopped> {
        let core = Core::new(stored, providers.catalog());
        let mut actor = RegistryActor::new(log, store, core, providers, home);
        for starting in actor.core.boot() {
            let started = start(&actor.providers, starting).await;
            let effects = actor.core.started(started);
            actor.perform(effects);
        }
        actor.restore().await?;
        Ok(actor.spawn())
    }

    /// Checks the directory and the instance, logs `SessionCreated` and
    /// answers `Started` at once. The agent starts inside the session's own
    /// task, so a slow start never blocks the registry. A session on an
    /// issue is refused while the instance has one on it, works on a
    /// branch cut from the issue's, and gets the issue's brief as its first
    /// message.
    ///
    /// # Errors
    ///
    /// Fails if the registry has stopped.
    pub async fn start(
        &self,
        instance: InstanceId,
        placed: Placement,
        choice: (Option<ModelId>, Option<Effort>),
        by: ClientName,
    ) -> Result<CommandResult, Stopped> {
        let (model, effort) = choice;
        self.ask(|reply| RegistryMsg::Start {
            instance,
            placed,
            model,
            effort,
            by,
            reply,
        })
        .await
    }

    /// The session, how its instance runs now and what it offers, for an
    /// order that may start the agent or is checked against what the
    /// instance offers: `Continue`, `UndoTurn` and `SetEffort`. The
    /// session starts its agent itself, so a slow start never blocks the
    /// registry.
    ///
    /// # Errors
    ///
    /// Fails if the registry has stopped.
    pub async fn relaunch(
        &self,
        session: SessionId,
    ) -> Result<Result<(Session, Models), Rejection>, Stopped> {
        self.ask(|reply| RegistryMsg::Continue { session, reply })
            .await
    }

    /// The session with this id, if the registry has it.
    ///
    /// # Errors
    ///
    /// Fails if the registry has stopped.
    pub async fn find(
        &self,
        session: SessionId,
    ) -> Result<Option<Session>, Stopped> {
        self.ask(|reply| RegistryMsg::Find { session, reply }).await
    }

    /// What `Welcome` tells a client the host can run.
    ///
    /// # Errors
    ///
    /// Fails if the registry has stopped.
    pub async fn instances(&self) -> Result<(Instances, Catalog), Stopped> {
        self.ask(|reply| RegistryMsg::Instances { reply }).await
    }

    /// Changes the host's instances and answers with what it runs then:
    /// once a changed credential has refreshed its instance, and once an
    /// added instance's CLI has started.
    ///
    /// # Errors
    ///
    /// Fails if the registry has stopped.
    pub async fn configure(
        &self,
        configure: Configure,
    ) -> Result<CommandResult, Stopped> {
        self.ask(|reply| RegistryMsg::Configure { configure, reply })
            .await
    }

    /// Starts, answers or cancels an instance's sign-in.
    ///
    /// # Errors
    ///
    /// Fails if the registry has stopped.
    pub async fn sign_in(
        &self,
        order: SignInOrder,
    ) -> Result<CommandResult, Stopped> {
        self.ask(|reply| RegistryMsg::SignIn { order, reply }).await
    }

    /// The sign-ins running now and every step after, taken together so
    /// none falls between.
    ///
    /// # Errors
    ///
    /// Fails if the registry has stopped.
    pub async fn watch(&self) -> Result<Watch, Stopped> {
        self.ask(|reply| RegistryMsg::Watch { reply }).await
    }

    async fn ask<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> RegistryMsg,
    ) -> Result<T, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.inbox
            .send(build(reply))
            .await
            .map_err(|_| Stopped {})?;
        answer.await.map_err(|_| Stopped {})
    }
}

impl RegistryActor {
    fn new(
        log: Log,
        store: Store,
        core: Core,
        providers: Providers,
        home: HomeClone,
    ) -> Self {
        Self {
            log,
            store,
            core,
            providers,
            home,
            refreshes: JoinSet::new(),
            starts: JoinSet::new(),
            sign_ins: SignIns::default(),
            runs: Runs::default(),
            live: broadcast::channel(SIGN_IN_BUFFER).0,
        }
    }

    fn spawn(self) -> Registry {
        let (inbox, messages) = mpsc::channel(16);
        tokio::spawn(self.run(messages));
        Registry { inbox }
    }

    async fn run(
        mut self,
        mut messages: mpsc::Receiver<RegistryMsg>,
    ) -> Result<Infallible, Stopped> {
        loop {
            let msg = tokio::select! {
                msg = messages.recv() => msg.ok_or(Stopped {})?,
                Some(Ok(result)) = self.refreshes.join_next() => RegistryMsg::Refreshed(result),
                Some(Ok(result)) = self.starts.join_next() => RegistryMsg::Started(result),
                Some((key, report)) = self.runs.next() => RegistryMsg::Run(key, report),
            };
            self.on(msg).await?;
        }
    }

    async fn on(&mut self, msg: RegistryMsg) -> Result<(), Stopped> {
        match msg {
            RegistryMsg::Refreshed(result) => {
                let effects = self.core.refreshed(result);
                self.perform(effects);
            }
            RegistryMsg::Started(result) => {
                let effects = self.core.started(result);
                self.perform(effects);
            }
            RegistryMsg::Start {
                instance,
                placed,
                model,
                effort,
                by,
                reply,
            } => {
                let result =
                    self.start(instance, placed, (model, effort), by).await?;
                let _ = reply.send(result);
            }
            RegistryMsg::Continue { session, reply } => {
                let _ = reply.send(self.core.relaunch(session));
            }
            RegistryMsg::Find { session, reply } => {
                let _ = reply.send(self.core.find(session));
            }
            RegistryMsg::Instances { reply } => {
                let _ = reply
                    .send((self.core.instances(), self.core.catalog().clone()));
            }
            RegistryMsg::Configure { configure, reply } => {
                let effects = self.configure(configure, reply).await?;
                self.perform(effects);
            }
            RegistryMsg::SignIn { order, reply } => {
                let (result, effects) = self.order(order);
                self.signs_in(effects).await;
                let _ = reply.send(result);
            }
            RegistryMsg::Watch { reply } => {
                let _ = reply
                    .send((self.sign_ins.running(), self.live.subscribe()));
            }
            RegistryMsg::Run(key, report) => {
                let effects = self.reported(&key, report);
                self.signs_in(effects).await;
            }
        }
        Ok(())
    }

    fn order(
        &mut self,
        order: SignInOrder,
    ) -> (CommandResult, Vec<SignInEffect>) {
        match order {
            SignInOrder::Start(instance) => {
                let settings = self.core.settings(&instance);
                self.sign_ins.start(instance, settings)
            }
            SignInOrder::Submit(instance, code) => {
                self.sign_ins.submit(instance, code)
            }
            SignInOrder::Cancel(instance) => self.sign_ins.cancel(instance),
        }
    }

    /// Hands the sign-ins' core what a run reported.
    fn reported(&mut self, key: &Key, report: Report) -> Vec<SignInEffect> {
        match report {
            Report::Spawned(spawned) => {
                let flow = self.runs.started(key, spawned);
                self.sign_ins.spawned(key, flow)
            }
            Report::Printed(line) => self.sign_ins.printed(key, &line),
            Report::Exited(code, status) => {
                self.sign_ins.exited(key, code, &status)
            }
            Report::Expired => self.sign_ins.expired(key),
            Report::Checked(code, account) => {
                self.sign_ins.checked(key, &code, account)
            }
            Report::Listed(result) => self.sign_ins.listed(key, result),
        }
    }

    async fn signs_in(&mut self, effects: Vec<SignInEffect>) {
        for effect in effects {
            self.sign_in(effect).await;
        }
    }

    async fn sign_in(&mut self, effect: SignInEffect) {
        match effect {
            SignInEffect::Spawn { key, settings } => {
                self.runs.spawn(key, self.providers.clone(), settings);
            }
            SignInEffect::Write(key, code) => {
                self.runs.write(&key, &code).await;
            }
            SignInEffect::Kill(key) => self.runs.kill(&key),
            SignInEffect::Check(key) => self.runs.check(key),
            SignInEffect::Refresh(key, account) => {
                let (reply, answer) = oneshot::channel();
                let effects =
                    self.core.signed_in(key.instance.clone(), account, reply);
                self.perform(effects);
                self.runs.listen(key, answer);
            }
            SignInEffect::Publish(step) => {
                crate::log_note(&host::sign_in::note(&step));
                let _ = self.live.send(step);
            }
        }
    }

    async fn configure(
        &mut self,
        configure: Configure,
        reply: Reply,
    ) -> Result<Vec<Effect>, Stopped> {
        Ok(match configure {
            Configure::SetCredential {
                provider,
                credential,
            } => self.core.set_credential(provider, credential, reply),
            Configure::Remove(instance) => {
                let history = self.log.history().await?;
                let running = host::registry::running(&history);
                self.core.remove_instance(instance, &running, reply)
            }
        })
    }

    /// Performs the effects in order, each effect's own effects before the
    /// next.
    fn perform(&mut self, mut effects: Vec<Effect>) {
        effects.reverse();
        while let Some(effect) = effects.pop() {
            let more = self.effect(effect);
            effects.extend(more.into_iter().rev());
        }
    }

    fn effect(&mut self, effect: Effect) -> Vec<Effect> {
        match effect {
            Effect::Reply(reply, result) => {
                let _ = reply.send(result);
                Vec::new()
            }
            Effect::MoveAside { instance, reply } => {
                let moved = self
                    .store
                    .move_aside()
                    .map(|aside| {
                        tracing::warn!(aside = %aside.display(), "moved the unreadable credential store aside");
                    })
                    .map_err(|err| format!("{err:#}"));
                self.core.moved_aside(instance, reply, moved)
            }
            Effect::Save {
                roster,
                change,
                reply,
            } => {
                let moved = match &change {
                    Change::SignedIn { from, to } => {
                        crate::start::move_instance(self.store.dir(), from, to)
                    }
                    Change::Credential(_)
                    | Change::Added(_)
                    | Change::Removed(_) => Ok(()),
                };
                let saved = moved
                    .and_then(|()| self.store.save(&roster))
                    .map_err(|err| format!("{err:#}"));
                self.core.saved(roster, change, reply, saved)
            }
            Effect::Refresh(job) => {
                self.refreshes.spawn(refresh(job));
                Vec::new()
            }
            Effect::Start(starting) => {
                let providers = self.providers.clone();
                self.starts
                    .spawn(async move { start(&providers, starting).await });
                Vec::new()
            }
            Effect::Log(note) => {
                crate::log_note(&note);
                Vec::new()
            }
        }
    }

    async fn restore(&mut self) -> Result<(), Stopped> {
        let history = self.log.history().await?;
        for created in host::restore::created(&history) {
            self.restore_session(created, &history).await?;
        }
        Ok(())
    }

    /// Recovers the session from the restart, then keeps it, its agent
    /// not running.
    async fn restore_session(
        &mut self,
        created: Created,
        history: &[Event],
    ) -> Result<(), Stopped> {
        let Created {
            session: id,
            instance,
            provider,
            directory,
            choice,
        } = created;
        let recovery = Recovery::of(choice, &events_of(id, history));
        let conversation = self.conversation(id, recovery.talked).await?;
        recover(&self.log, id, recovery.steps()).await?;
        let recovered = events_of(id, &self.log.history().await?);
        let runtime = self
            .core
            .launcher(&instance)
            .map_or_else(Runtime::refusing, |start| start.start());
        let told = self.told(provider);
        let session = Session::restore(
            id,
            directory,
            recovery.choice,
            told,
            (runtime, conversation),
            &recovered,
            self.log.clone(),
        );
        self.core.admit(id, instance, session);
        Ok(())
    }

    fn told(&self, provider: Provider) -> Told {
        Told {
            home: self.home.clone(),
            instructions_file: self.providers.instructions_file(provider),
        }
    }

    /// The conversation the host saved for a session a turn started in.
    async fn conversation(
        &self,
        id: SessionId,
        talked: bool,
    ) -> Result<Option<String>, Stopped> {
        if talked {
            self.log.saved_conversation(id).await
        } else {
            Ok(None)
        }
    }

    async fn start(
        &mut self,
        instance: InstanceId,
        placed: Placement,
        asked: (Option<ModelId>, Option<Effort>),
        by: ClientName,
    ) -> Result<CommandResult, Stopped> {
        if let Some(session) = self.taken(&placed, &instance).await? {
            return Ok(CommandResult::Rejected(Rejection::IssueTaken {
                session,
            }));
        }
        // `read_dir` fails for a missing path and for a file alike.
        let exists = std::fs::read_dir(&placed.directory.0).is_ok();
        let (model, effort) = asked;
        match self.core.start(&instance, exists, model, effort) {
            Err(rejection) => Ok(CommandResult::Rejected(rejection)),
            Ok(launch) => self.create(instance, placed, launch, by).await,
        }
    }

    /// Logs the issue the new session works on, if any.
    async fn attach(
        &self,
        id: SessionId,
        on: Option<&home::issue::Start>,
    ) -> Result<(), Stopped> {
        match on {
            Some(start) => {
                let issue = start.issue;
                self.log.append(id, EventKind::OnIssue { issue }).await
            }
            None => Ok(()),
        }
    }

    /// The session the instance has on the placement's issue already.
    async fn taken(
        &self,
        placed: &Placement,
        instance: &InstanceId,
    ) -> Result<Option<SessionId>, Stopped> {
        let Some(on) = &placed.on else {
            return Ok(None);
        };
        let history = self.log.history().await?;
        Ok(host::registry::on_issue(&history, on.issue, instance))
    }

    /// Makes the session's workspace and first checkpoint, logs them, and
    /// starts the session.
    async fn create(
        &mut self,
        instance: InstanceId,
        placed: Placement,
        launch: Launch<Starter>,
        by: ClientName,
    ) -> Result<CommandResult, Stopped> {
        let id = SessionId(u128::from(ulid::Ulid::new()));
        let Placement { directory, on } = placed;
        let base = on.as_ref().map(|start| start.branch.as_str());
        match prepare(self.store.dir(), &directory, id, base).await {
            Err(rejection) => Ok(CommandResult::Rejected(rejection)),
            Ok((workspace, commit)) => {
                let Launch {
                    start,
                    choice,
                    offered,
                    provider,
                } = launch;
                let created = EventKind::SessionCreated {
                    instance: instance.clone(),
                    provider,
                    directory: directory.clone(),
                    by: by.clone(),
                    model: choice.model.clone(),
                    effort: choice.effort,
                };
                self.log.append(id, created).await?;
                self.attach(id, on.as_ref()).await?;
                let workspace_event = self
                    .log
                    .record(id, EventKind::WorkspaceCreated { workspace })
                    .await?;
                let checkpoint = initial_checkpoint(commit);
                let checkpoint_event = self
                    .log
                    .record(id, EventKind::CheckpointCreated { checkpoint })
                    .await?;
                let told = self.told(provider);
                let session = Session::spawn(
                    id,
                    directory,
                    choice,
                    told,
                    (start.start(), offered),
                    self.log.clone(),
                    &[workspace_event, checkpoint_event],
                );
                brief((&session, &self.log, id), on, by);
                self.core.admit(id, instance, session);
                Ok(CommandResult::Started { session: id })
            }
        }
    }
}

/// The instance's agents take the credential, and then it lists what it
/// offers with it, if it then signs in.
async fn refresh(job: Job<Starter>) -> Refreshed {
    Refreshed {
        id: job.id,
        instance: job.instance,
        listing: listing(job.start, job.credential, job.signs_in).await,
    }
}

async fn start(providers: &Providers, starting: Starting) -> Started<Starter> {
    let Starting {
        id,
        instance,
        settings,
    } = starting;
    let launcher = providers.launch(&instance, settings).await;
    Started {
        id,
        instance,
        launcher,
    }
}

async fn listing(
    start: Starter,
    credential: Option<Credential>,
    signs_in: Result<(), String>,
) -> Result<Listing, String> {
    let mut runtime = start.start();
    let named = runtime
        .ask(Command::Credential(credential))
        .await
        .configured()?;
    signs_in?;
    let models = runtime.ask(Command::Models).await.models()?;
    Ok(Listing { models, named })
}

/// Sends a session on an issue the issue's brief, once its agent has
/// started, in a task of its own so the registry doesn't wait for the
/// agent.
fn brief(
    (session, log, id): (&Session, &Log, SessionId),
    on: Option<home::issue::Start>,
    by: ClientName,
) {
    let Some(start) = on else {
        return;
    };
    let (briefed, logged) = (session.clone(), log.clone());
    let text = start.brief;
    tokio::spawn(async move {
        let sent = briefed.order(Order::Send { text, by }).await;
        if let Some(message) = host::registry::unbriefed(sent.ok().as_ref()) {
            tracing::warn!(%message, "an issue's brief was not sent");
            let _ = logged.append(id, EventKind::Error { message }).await;
        }
    });
}

/// The session's workspace, on a branch cut from `base` or else the
/// checkout's `HEAD`, and the commit of its first checkpoint.
async fn prepare(
    data_dir: &Path,
    directory: &HostPath,
    id: SessionId,
    base: Option<&str>,
) -> Result<(Workspace, String), Rejection> {
    let workspace = crate::workspace::create(data_dir, directory, id, base)
        .await
        .map_err(host::registry::unprepared)?;
    let commit =
        crate::workspace::snapshot(&workspace, id, CheckpointId(0), None)
            .await
            .map_err(|err| Rejection::WorkspaceFailure {
                reason: err.to_string(),
            })?;
    Ok((workspace, commit))
}

fn initial_checkpoint(commit: String) -> Checkpoint {
    Checkpoint {
        id: CheckpointId(0),
        parent: None,
        turn_start: None,
        commit,
        history: HistoryAnchor::Fresh,
        undo: UndoAvailability::Unavailable {
            reason: String::from("initial checkpoint has no turn"),
        },
    }
}

/// Performs what a restart does to the session.
async fn recover(
    log: &Log,
    id: SessionId,
    steps: Vec<Step>,
) -> Result<(), Stopped> {
    for step in steps {
        match step {
            Step::Append(kind) => log.append(id, kind).await?,
            Step::Barrier => log.barrier().await?,
            Step::Snapshot(turn) => {
                let snapshot = crate::workspace::snapshot(
                    &turn.workspace,
                    id,
                    turn.reserved,
                    turn.base.as_deref(),
                )
                .await
                .map_err(|err| err.to_string());
                log.append(id, turn.snapshotted(snapshot)).await?;
            }
        }
    }
    Ok(())
}
