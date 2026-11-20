//! The registry's actor: the one place a `StartSession` and a `Continue`
//! are checked, a session is found by its id, the host's instances
//! change, and an instance signs in from the app.
//!
//! Its decisions are `host::registry`'s and `host::sign_in`'s; the actor
//! keeps the store, the sessions' tasks, the instances' refreshes and
//! starts and the sign-ins' runs, and performs what the cores decide.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::ops::ControlFlow::{Break, Continue};

use anyhow::Context;
use host::registry::{
    Change, Job, Refreshed, StartDecision, StartEffect, StartEnd, StartOutcome,
    StartReport, StartRequest, StartSession, StartStep, Started, Starting,
    brief_sent,
};
use host::restore::{Created, Recovery, Step, events_of};
use host::serve::{Configure, SignInOrder};
use host::sign_in::{Effect as SignInEffect, Key, SignIns};
use proto::{
    Catalog, ClientName, CommandResult, Effort, Event, EventKind, InstanceId,
    Instances, ModelId, Models, Provider, Rejection, Roster, SessionId, SignIn,
    SignInStep, StartAt,
};
use runtime::Instance;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinSet;

use crate::Stopped;
use crate::credentials::{RecoveredAdoption, SigningLease, Store, Stored};
use crate::home::{HomeClone, Placement};
use crate::log::Log;
use crate::session::{Order, Session, Told};
use crate::sign_in::{Report, Runs};
use crate::start::Providers;

/// A subscriber this far behind the sign-ins is dropped and resubscribes.
const SIGN_IN_BUFFER: usize = 64;

pub struct Watch {
    pub running: Vec<SignIn>,
    pub sign_ins: broadcast::Receiver<SignIn>,
    pub offers: watch::Receiver<Instances>,
}

pub type Launcher = host::registry::Launcher<Instance>;

type Reply = oneshot::Sender<CommandResult>;
type Core = host::registry::Registry<Instance, Session, Reply>;
type Effect = host::registry::Effect<Instance, Reply>;
type StartReply = oneshot::Sender<StartOutcome>;

struct BranchReady {
    next: StartSession<Instance, Session>,
    report: StartReport<Session>,
    reply: StartReply,
}

#[derive(Debug, Clone)]
pub struct Registry {
    inbox: mpsc::Sender<RegistryMsg>,
    home: HomeClone,
}

enum RegistryMsg {
    Refreshed(Refreshed),
    Started(Started<Instance>),
    Start {
        request: StartRequest,
        placed: Box<Result<Placement, Rejection>>,
        reply: StartReply,
    },
    BranchReady(Box<BranchReady>),
    Continue {
        session: SessionId,
        reply: oneshot::Sender<Result<(Session, Instance, Models), Rejection>>,
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
    starts: JoinSet<Started<Instance>>,
    branches: JoinSet<BranchReady>,
    sign_ins: SignIns,
    leases: BTreeMap<InstanceId, SigningLease>,
    runs: Runs,
    live: broadcast::Sender<SignIn>,
    offers: watch::Sender<Instances>,
    initialized: Option<watch::Sender<bool>>,
}

impl Registry {
    pub async fn spawn(
        log: Log,
        store: Store,
        stored: &Stored,
        providers: Providers,
        home: HomeClone,
    ) -> Result<(Self, watch::Receiver<bool>), Stopped> {
        let mut core = Core::new(stored, providers.catalog());
        let starts = core.boot();
        let (ready, initialized) = watch::channel(false);
        let mut actor =
            RegistryActor::new(log, store, core, providers, home, ready);
        actor.perform(starts.into_iter().map(Effect::Start).collect());
        actor.restore().await?;
        Ok((actor.spawn(), initialized))
    }

    /// The caller holds the session command lease. It keeps competing
    /// starts out until the opening records are committed, including while
    /// the registry waits for a branch alongside its other requests.
    ///
    /// # Errors
    ///
    /// Fails if the registry has stopped.
    pub async fn start(
        &self,
        instance: InstanceId,
        at: StartAt,
        choice: (Option<ModelId>, Option<Effort>),
        by: ClientName,
    ) -> Result<CommandResult, Stopped> {
        let placed = self.home.place(at).await?;
        let (model, effort) = choice;
        let request = StartRequest {
            instance,
            model,
            effort,
            by,
        };
        let started = self
            .ask(|reply| RegistryMsg::Start {
                request,
                placed: Box::new(placed),
                reply,
            })
            .await?;
        if let Some((issue, by)) = started.progress {
            self.home.started(issue, by).await?;
        }
        Ok(started.result)
    }

    /// The session, its current instance adapter and its offer, for an
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
    ) -> Result<Result<(Session, Instance, Models), Rejection>, Stopped> {
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

    /// Captures the sign-ins, offers and their live updates in one registry turn.
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
        initialized: watch::Sender<bool>,
    ) -> Self {
        Self {
            log,
            store,
            offers: watch::channel(core.instances()).0,
            core,
            providers,
            home,
            refreshes: JoinSet::new(),
            starts: JoinSet::new(),
            branches: JoinSet::new(),
            sign_ins: SignIns::default(),
            leases: BTreeMap::new(),
            runs: Runs::default(),
            initialized: Some(initialized),
            live: broadcast::channel(SIGN_IN_BUFFER).0,
        }
    }

    fn spawn(self) -> Registry {
        let (inbox, messages) = mpsc::channel(16);
        let home = self.home.clone();
        tokio::spawn(self.run(messages));
        Registry { inbox, home }
    }

    async fn run(
        mut self,
        mut messages: mpsc::Receiver<RegistryMsg>,
    ) -> Result<Infallible, Stopped> {
        loop {
            let msg = tokio::select! {
                msg = messages.recv() => msg.ok_or(Stopped {})?,
                Some(Ok(result)) = self.refreshes.join_next() => RegistryMsg::Refreshed(result),
                Some(result) = self.starts.join_next() => RegistryMsg::Started(result.map_err(|_| Stopped {})?),
                Some(Ok(result)) = self.branches.join_next() => RegistryMsg::BranchReady(Box::new(result)),
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
                request,
                placed,
                reply,
            } => self.start(request, *placed, reply).await?,
            RegistryMsg::BranchReady(ready) => {
                let BranchReady {
                    next,
                    report,
                    reply,
                } = *ready;
                let decision = next.report(&mut self.core, report);
                self.run_start(decision, reply).await?;
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
                let _ = reply.send(Watch {
                    running: self.sign_ins.running(),
                    sign_ins: self.live.subscribe(),
                    offers: self.offers.subscribe(),
                });
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
            SignInOrder::Start(instance) => self.start_sign_in(instance),
            SignInOrder::Submit(instance, code) => {
                self.sign_ins.submit(instance, code)
            }
            SignInOrder::Cancel(instance) => self.sign_ins.cancel(instance),
        }
    }

    fn start_sign_in(
        &mut self,
        instance: InstanceId,
    ) -> (CommandResult, Vec<SignInEffect>) {
        match self.prepare_sign_in(&instance) {
            Ok(Some(settings)) => self.sign_ins.start(instance, Some(settings)),
            Ok(None) => (CommandResult::Accepted, Vec::new()),
            Err(rejection) => (CommandResult::Rejected(rejection), Vec::new()),
        }
    }

    fn prepare_sign_in(
        &mut self,
        instance: &InstanceId,
    ) -> Result<Option<proto::Instance>, Rejection> {
        if self.sign_ins.is_running(instance) {
            return Err(Rejection::SignInRunning);
        }
        let recovered = self.store.resume_adoption().map_err(|err| {
            Rejection::CredentialNotSaved {
                reason: format!("{err:#}"),
            }
        })?;
        if recovered.map_or(Ok(false), |adoption| {
            self.resume_sign_in(instance, adoption)
        })? {
            return Ok(None);
        }
        let settings = self
            .core
            .settings(instance)
            .ok_or(Rejection::UnknownInstance)?;
        let lease = self.store.signing(instance).map_err(|err| {
            Rejection::InstanceUnavailable {
                reason: format!("{err:#}"),
            }
        })?;
        self.leases.insert(instance.clone(), lease);
        Ok(Some(settings))
    }

    fn resume_sign_in(
        &mut self,
        requested: &InstanceId,
        adoption: RecoveredAdoption,
    ) -> Result<bool, Rejection> {
        let RecoveredAdoption { from, to, roster } = adoption;
        let run = (from == *requested)
            .then(|| self.sign_ins.resume(from.clone()))
            .transpose()?;
        let (reply, answer) = oneshot::channel();
        let effects = self.core.saved(
            roster,
            Change::SignedIn { from, to },
            reply,
            Ok(()),
        );
        self.perform(effects);
        let resumed = run.is_some();
        if let Some(key) = run {
            self.runs.listen(key, answer);
        }
        Ok(resumed)
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
            Report::Checked(result) => self.sign_ins.checked(key, result),
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
                let lease = self.leases.get(&key.instance).cloned();
                self.runs
                    .spawn(key, self.providers.clone(), settings, lease);
            }
            SignInEffect::Write(key, code) => {
                self.runs.write(&key, &code).await;
            }
            SignInEffect::Kill(key) => self.runs.kill(&key),
            SignInEffect::Check(key, login) => {
                let lease = self.leases.get(&key.instance).cloned();
                self.runs.check(key, login, lease);
            }
            SignInEffect::Refresh(key, account) => {
                let (reply, answer) = oneshot::channel();
                let effects =
                    self.core.signed_in(key.instance.clone(), account, reply);
                self.perform(effects);
                self.runs.listen(key, answer);
            }
            SignInEffect::Publish(step) => self.publish_sign_in(step),
        }
    }

    fn publish_sign_in(&mut self, step: SignIn) {
        if matches!(
            step.step,
            SignInStep::SignedIn { .. }
                | SignInStep::Failed { .. }
                | SignInStep::Cancelled
        ) {
            self.leases.remove(&step.instance);
        }
        crate::log_note(&host::sign_in::note(&step));
        let _ = self.live.send(step);
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
        self.publish_offers();
    }

    fn publish_offers(&mut self) {
        let current = self.core.instances();
        if *self.offers.borrow() != current {
            self.offers.send_replace(current);
        }
        if let Some(ready) =
            self.initialized.take_if(|_| self.core.offers_settled())
        {
            ready.send_replace(true);
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
            } => self.save(roster, change, reply),
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

    fn save(
        &mut self,
        roster: Roster,
        change: Change,
        reply: Reply,
    ) -> Vec<Effect> {
        let saved = match &change {
            Change::SignedIn { from, to } => self
                .leases
                .get(from)
                .context("the sign-in no longer has its private home")
                .and_then(|lease| self.store.adopt(lease, to)),
            Change::Credential(_) | Change::Added(_) | Change::Removed(_) => {
                self.store.save_change(&roster, &change)
            }
        };
        match saved {
            Ok(current) => self.core.saved(current, change, reply, Ok(())),
            Err(err) => {
                self.core
                    .saved(roster, change, reply, Err(format!("{err:#}")))
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
        let told = self.told(provider);
        let session = Session::restore(
            id,
            directory,
            recovery.choice,
            told,
            (None, conversation),
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
        request: StartRequest,
        placed: Result<Placement, Rejection>,
        reply: StartReply,
    ) -> Result<(), Stopped> {
        let history = if placed.as_ref().is_ok_and(|at| at.on.is_some()) {
            self.log.history().await?
        } else {
            Vec::new()
        };
        self.run_start(request.placed(placed, &history), reply)
            .await
    }

    fn ready_branch(
        &mut self,
        branch: home::issue::Start,
        next: StartSession<Instance, Session>,
        reply: StartReply,
    ) {
        let home = self.home.clone();
        self.branches.spawn(async move {
            let report = home
                .ready(branch)
                .await
                .map_or(StartReport::Stopped, StartReport::Branch);
            BranchReady {
                next,
                report,
                reply,
            }
        });
    }

    async fn run_start(
        &mut self,
        mut decision: StartDecision<Instance, Session>,
        reply: StartReply,
    ) -> Result<(), Stopped> {
        loop {
            let StartStep { effect, next } = match decision {
                Continue(step) => step,
                Break(StartEnd::Finished(outcome)) => {
                    let _ = reply.send(outcome);
                    return Ok(());
                }
                Break(StartEnd::Stopped) => return Err(Stopped {}),
            };
            let report = match effect {
                StartEffect::ReadyBranch(branch) => {
                    self.ready_branch(branch, next, reply);
                    return Ok(());
                }
                StartEffect::CheckDirectory(path) => {
                    StartReport::Directory(std::fs::read_dir(path.0).is_ok())
                }
                StartEffect::CreateWorkspace { directory, base } => {
                    let id = SessionId(u128::from(ulid::Ulid::new()));
                    let workspace = crate::workspace::create(
                        self.store.dir(),
                        &directory,
                        id,
                        base.as_deref(),
                    )
                    .await;
                    StartReport::Workspace(id, workspace)
                }
                StartEffect::Snapshot {
                    session,
                    workspace,
                    id,
                } => StartReport::Snapshot(
                    crate::workspace::snapshot(&workspace, session, id, None)
                        .await,
                ),
                StartEffect::Record {
                    session,
                    before,
                    seed,
                } => self
                    .record_opening(session, before, *seed)
                    .await
                    .map_or(StartReport::Stopped, |events| {
                        StartReport::Recorded(Box::new(events))
                    }),
                StartEffect::Spawn {
                    session,
                    directory,
                    choice,
                    provider,
                    agent,
                    history,
                } => {
                    let (starter, offered) = agent;
                    StartReport::Spawned(Session::spawn(
                        session,
                        directory,
                        choice,
                        self.told(provider),
                        (starter.start(), offered),
                        self.log.clone(),
                        history.as_ref(),
                    ))
                }
                StartEffect::Brief {
                    session,
                    handle,
                    text,
                    by,
                } => {
                    brief_issue(handle, self.log.clone(), session, text, by);
                    StartReport::BriefQueued
                }
            };
            decision = next.report(&mut self.core, report);
        }
    }

    async fn record_opening(
        &self,
        id: SessionId,
        before: Vec<EventKind>,
        [workspace, checkpoint]: [EventKind; 2],
    ) -> Result<[Event; 2], Stopped> {
        for event in before {
            self.log.append(id, event).await?;
        }
        let workspace_event = self.log.record(id, workspace).await?;
        let checkpoint_event = self.log.record(id, checkpoint).await?;
        Ok([workspace_event, checkpoint_event])
    }
}

async fn refresh(job: Job<Instance>) -> Refreshed {
    Refreshed {
        id: job.id,
        instance: job.instance_id,
        configured: job.adapter.configure(job.credential).await,
    }
}

async fn start(providers: &Providers, starting: Starting) -> Started<Instance> {
    let Starting {
        mode,
        id,
        instance,
        settings,
    } = starting;
    let launcher = providers.launch(&instance, settings, mode).await;
    Started {
        id,
        instance,
        launcher,
    }
}

/// Sends a session on an issue the issue's brief, once its agent has
/// started, in a task of its own so the registry doesn't wait for the
/// agent.
fn brief_issue(
    session: Session,
    log: Log,
    id: SessionId,
    text: String,
    by: ClientName,
) {
    tokio::spawn(async move {
        let sent = session.order(Order::Send { text, by }).await;
        if let Some(event) = brief_sent(sent.ok().as_ref()) {
            tracing::warn!(?event, "an issue's brief was not sent");
            let _ = log.append(id, event).await;
        }
    });
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
