//! The registry's actor: the one place a `StartSession` and a `Continue`
//! are checked, a session is found by its id, the host's instances
//! change, and an instance signs in from the app.
//!
//! Its decisions are `host::registry`'s and `host::sign_in`'s; the actor
//! keeps the store, the sessions' tasks, the instances' refreshes and
//! starts and the sign-ins' runs, and performs what the cores decide.

use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::ops::ControlFlow::{Break, Continue};
use std::sync::{Arc, Weak};

use anyhow::Context;
use host::registry::{
    Change, Job, Refreshed, StartDecision, StartEffect, StartEnd, StartOutcome,
    StartReport, StartRequest, StartSession, StartStep, Started, Starting,
    brief_sent,
};
use host::restore::{Created, Recovery, Step, events_of};
use host::serve::{Configure, SignInOrder};
use host::sign_in::{CredentialTarget, Effect as SignInEffect, Key, SignIns};
use proto::{
    Catalog, ClientName, CommandId, CommandResult, Credential, Effort, Event,
    EventKind, InstanceId, Instances, ModelId, Models, Provider, Rejection,
    Roster, SessionId, SignIn, StartAt,
};
use runtime::Instance;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::{Id, JoinSet};

use crate::Stopped;
use crate::credentials::{
    CredentialLease, RecoveredAdoption, SigningLease, Store, Stored,
};
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
    pub configuration: watch::Receiver<Option<proto::fleet::Application>>,
}

pub type Launcher = host::registry::Launcher<Instance>;

struct Reply {
    removal: Option<CommandId>,
    credential: Option<CredentialLease>,
    application: bool,
    channel: Option<oneshot::Sender<CommandResult>>,
}
type Core = host::registry::Registry<Instance, Session, Reply>;
type Effect = host::registry::Effect<Instance, Reply>;
pub type Relaunch = (Session, Instance, Models, Arc<()>);
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
        reply: oneshot::Sender<Result<Relaunch, Rejection>>,
    },
    Find {
        session: SessionId,
        reply: oneshot::Sender<Option<Session>>,
    },
    MoveInstance {
        source: InstanceId,
        incoming: bool,
        reply: oneshot::Sender<
            Result<(InstanceId, proto::Instance, Providers), String>,
        >,
    },
    Arrive {
        session: SessionId,
        id: proto::movement::MoveId,
        reply: oneshot::Sender<()>,
    },
    Instances {
        reply: oneshot::Sender<(Instances, Catalog)>,
    },
    ReconfigureFleet {
        providers: BTreeSet<Provider>,
        reply: oneshot::Sender<bool>,
    },
    Configure {
        application: bool,
        configure: Configure,
        id: CommandId,
        reply: oneshot::Sender<CommandResult>,
    },
    SignIn {
        order: SignInOrder,
        reply: oneshot::Sender<CommandResult>,
    },
    Watch {
        reply: oneshot::Sender<Watch>,
    },
    Run(Key, Report),
    ForOrder(SessionId, bool, oneshot::Sender<Result<Session, Rejection>>),
    Tick(u64),
    Desire(u64, Roster, BTreeSet<InstanceId>, oneshot::Sender<()>),
    Adopted(u64, oneshot::Sender<()>),
    Retired(InstanceId, u64, agents::Retirement, bool),
}

struct RegistryActor {
    log: Log,
    store: Store,
    core: Core,
    providers: Providers,
    home: HomeClone,
    retirements: JoinSet<(InstanceId, u64, agents::Retirement, bool)>,
    refreshes: JoinSet<Refreshed>,
    starts: JoinSet<Started<Instance>>,
    refresh_jobs: BTreeMap<Id, (u64, InstanceId)>,
    start_jobs: BTreeMap<Id, (u64, InstanceId)>,
    start_controls: BTreeMap<
        InstanceId,
        (u64, crate::start::Control, tokio::task::AbortHandle),
    >,
    refresh_controls: BTreeMap<InstanceId, tokio::task::AbortHandle>,
    relaunching: BTreeMap<SessionId, Weak<()>>,
    arrivals: BTreeMap<SessionId, proto::movement::MoveId>,
    branches: JoinSet<BranchReady>,
    sign_ins: SignIns,
    leases: BTreeMap<InstanceId, SigningLease>,
    runs: Runs,
    live: broadcast::Sender<SignIn>,
    offers: watch::Sender<Instances>,
    configuration: watch::Sender<Option<proto::fleet::Application>>,
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
        if let Some(fleet) = providers.membership() {
            let desired = fleet.desired().await?;
            core.desire(desired.version, &desired.credentials, &desired.owned);
        }
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
    ) -> Result<Result<Relaunch, Rejection>, Stopped> {
        self.ask(|reply| RegistryMsg::Continue { session, reply })
            .await
    }

    /// The session with this id, if the registry has it.
    ///
    /// # Errors
    ///
    /// Fails if the registry has stopped.
    pub(crate) async fn for_order(
        &self,
        session: SessionId,
        requires_instance: bool,
    ) -> Result<Result<Session, Rejection>, Stopped> {
        self.ask(|reply| {
            RegistryMsg::ForOrder(session, requires_instance, reply)
        })
        .await
    }

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
    pub(crate) async fn move_instance(
        &self,
        source: InstanceId,
        incoming: bool,
    ) -> Result<(InstanceId, proto::Instance, Providers), String> {
        self.ask(|reply| RegistryMsg::MoveInstance {
            source,
            incoming,
            reply,
        })
        .await
        .map_err(|err| err.to_string())?
    }

    pub(crate) async fn arrived(
        &self,
        session: SessionId,
        id: proto::movement::MoveId,
    ) -> Result<(), Stopped> {
        self.ask(|reply| RegistryMsg::Arrive { session, id, reply })
            .await
    }

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
        id: CommandId,
    ) -> Result<CommandResult, Stopped> {
        self.ask(|reply| RegistryMsg::Configure {
            application: false,
            configure,
            id,
            reply,
        })
        .await
    }

    pub(crate) async fn reconfigure_fleet(
        &self,
        providers: BTreeSet<Provider>,
    ) -> Result<bool, Stopped> {
        self.ask(|reply| RegistryMsg::ReconfigureFleet { providers, reply })
            .await
    }

    pub(crate) async fn apply(
        &self,
        configure: Configure,
        id: CommandId,
    ) -> Result<CommandResult, Stopped> {
        self.ask(|reply| RegistryMsg::Configure {
            application: true,
            configure,
            id,
            reply,
        })
        .await
    }

    pub(crate) async fn desire(
        &self,
        version: u64,
        desired: Roster,
        known: BTreeSet<InstanceId>,
    ) -> Result<(), Stopped> {
        self.ask(|reply| RegistryMsg::Desire(version, desired, known, reply))
            .await
    }

    pub(crate) async fn adopted(&self, version: u64) -> Result<(), Stopped> {
        self.ask(|reply| RegistryMsg::Adopted(version, reply)).await
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
        crate::ask(&self.inbox, build).await
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
            configuration: watch::channel(core.application()).0,
            core,
            providers,
            home,
            retirements: JoinSet::new(),
            refreshes: JoinSet::new(),
            starts: JoinSet::new(),
            refresh_jobs: BTreeMap::new(),
            start_jobs: BTreeMap::new(),
            start_controls: BTreeMap::new(),
            refresh_controls: BTreeMap::new(),
            relaunching: BTreeMap::new(),
            arrivals: BTreeMap::new(),
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
        let began = tokio::time::Instant::now();
        let mut clock =
            tokio::time::interval(std::time::Duration::from_millis(250));
        loop {
            let msg = tokio::select! {
                _ = clock.tick() => RegistryMsg::Tick(began.elapsed().as_secs()),
                Some(Ok((instance, id, stage, success))) = self.retirements.join_next() => RegistryMsg::Retired(instance, id, stage, success),
                msg = messages.recv() => msg.ok_or(Stopped {})?,
                Some(result) = self.refreshes.join_next_with_id() => {
                    match complete_job(&mut self.refresh_jobs, result, |id, instance, reason| Refreshed {
                        id, instance, configured: Err(agents::ConfigurationFailure::Temporary(reason)),
                    }) {
                        Some(refreshed) => RegistryMsg::Refreshed(refreshed),
                        None => continue,
                    }
                },
                Some(result) = self.starts.join_next_with_id() => {
                    match complete_job(&mut self.start_jobs, result, |id, instance, reason| Started {
                        id, instance, launcher: Launcher::Unavailable(reason),
                    }) {
                        Some(started) => RegistryMsg::Started(started),
                        None => continue,
                    }
                },
                Some(Ok(result)) = self.branches.join_next() => RegistryMsg::BranchReady(Box::new(result)),
                Some((key, report)) = self.runs.next() => RegistryMsg::Run(key, report),
            };
            self.on(msg).await?;
        }
    }

    async fn on(&mut self, msg: RegistryMsg) -> Result<(), Stopped> {
        match msg {
            RegistryMsg::Tick(now) => {
                let effects = self.core.tick(now);
                self.perform(effects);
            }
            RegistryMsg::ForOrder(session, required, reply) => {
                let _ = reply.send(self.core.for_order(session, required));
            }
            RegistryMsg::Desire(version, desired, known, reply) => {
                let effects = self.core.desire(version, &desired, &known);
                self.perform(effects);
                let _ = reply.send(());
            }
            RegistryMsg::Adopted(version, reply) => {
                self.core.adopted(version);
                self.publish_offers();
                let _ = reply.send(());
            }
            RegistryMsg::Retired(instance, id, stage, success) => {
                let effects = self.core.retired(&instance, id, stage, success);
                self.perform(effects);
            }
            RegistryMsg::Refreshed(result) => {
                let effects = self.core.refreshed(result);
                self.perform(effects);
            }
            RegistryMsg::Started(result) => self.started(result),
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
                let _ = reply.send(self.reserve_relaunch(session));
            }
            RegistryMsg::Find { session, reply } => {
                let _ = reply.send(self.core.find(session));
            }
            RegistryMsg::MoveInstance {
                source,
                incoming,
                reply,
            } => {
                let _ = reply.send(self.move_instance(source, incoming));
            }
            RegistryMsg::Arrive { session, id, reply } => {
                self.register_arrival(session, id).await?;
                let _ = reply.send(());
            }
            RegistryMsg::Instances { reply } => {
                let _ = reply
                    .send((self.core.instances(), self.core.catalog().clone()));
            }
            RegistryMsg::ReconfigureFleet { providers, reply } => {
                self.reconfigure(providers).await?;
                let _ = reply.send(true);
            }
            RegistryMsg::Configure {
                application,
                configure,
                id,
                reply,
            } => {
                let effects =
                    self.configure(configure, id, reply, application).await?;
                self.perform(effects);
            }
            RegistryMsg::SignIn { order, reply } => {
                let (result, effects) = self.order(order).await?;
                self.signs_in(effects).await?;
                let _ = reply.send(result);
            }
            RegistryMsg::Watch { reply } => {
                let _ = reply.send(Watch {
                    running: self.sign_ins.running(),
                    sign_ins: self.live.subscribe(),
                    offers: self.offers.subscribe(),
                    configuration: self.configuration.subscribe(),
                });
            }
            RegistryMsg::Run(key, report) => {
                let effects = self.reported(&key, report);
                self.signs_in(effects).await?;
            }
        }
        Ok(())
    }

    fn started(&mut self, result: Started<Instance>) {
        if self
            .start_controls
            .get(&result.instance)
            .is_some_and(|(id, _, _)| *id == result.id)
        {
            self.start_controls.remove(&result.instance);
        }
        let effects = self.core.started(result);
        self.perform(effects);
    }

    fn reserve_relaunch(
        &mut self,
        session: SessionId,
    ) -> Result<Relaunch, Rejection> {
        self.core
            .relaunch(session)
            .map(|(handle, adapter, models)| {
                let reservation = self
                    .relaunching
                    .get(&session)
                    .and_then(Weak::upgrade)
                    .unwrap_or_default();
                self.relaunching
                    .insert(session, Arc::downgrade(&reservation));
                (handle, adapter, models, reservation)
            })
    }

    async fn reconfigure(
        &mut self,
        providers: BTreeSet<Provider>,
    ) -> Result<(), Stopped> {
        for sign_in in self.sign_ins.running() {
            if providers.contains(&sign_in.instance.provider) {
                let effects = self.sign_ins.retire(sign_in.instance);
                self.signs_in(effects).await?;
            }
        }
        let effects = self.core.reconfigure(&providers);
        self.perform(effects);
        Ok(())
    }

    async fn order(
        &mut self,
        order: SignInOrder,
    ) -> Result<(CommandResult, Vec<SignInEffect>), Stopped> {
        Ok(match order {
            SignInOrder::Start(instance) => {
                let running = self.running_sessions().await?;
                self.start_sign_in(instance, &running)
            }
            SignInOrder::Credential {
                instance,
                kind,
                label,
            } => {
                let running = self.running_sessions().await?;
                match self.prepare_credential_sign_in(
                    &instance, kind, &label, &running,
                ) {
                    Ok(target) => {
                        self.sign_ins.start_credential(instance, target)
                    }
                    Err(rejection) => {
                        (CommandResult::Rejected(rejection), Vec::new())
                    }
                }
            }
            SignInOrder::Submit(instance, code) => {
                self.sign_ins.submit(instance, code)
            }
            SignInOrder::Cancel(instance) => self.sign_ins.cancel(instance),
        })
    }

    fn start_sign_in(
        &mut self,
        instance: InstanceId,
        running: &BTreeSet<SessionId>,
    ) -> (CommandResult, Vec<SignInEffect>) {
        match self.prepare_sign_in(&instance, running) {
            Ok(Some(settings)) => self.sign_ins.start(instance, Some(settings)),
            Ok(None) => (CommandResult::Accepted, Vec::new()),
            Err(rejection) => (CommandResult::Rejected(rejection), Vec::new()),
        }
    }

    fn prepare_sign_in(
        &mut self,
        instance: &InstanceId,
        running: &BTreeSet<SessionId>,
    ) -> Result<Option<proto::Instance>, Rejection> {
        if self.sign_ins.is_running(instance) {
            return Err(Rejection::SignInRunning);
        }
        let recovered = self
            .store
            .resume_adoption(|account| {
                self.core
                    .instance_in_use(&[&account.from, &account.to], running)
                    .is_none()
            })
            .map_err(|err| Rejection::CredentialNotSaved {
                reason: format!("{err:#}"),
            })?;
        if recovered.map_or(Ok(false), |adoption| {
            self.resume_sign_in(instance, adoption)
        })? {
            return Ok(None);
        }
        self.core
            .settings(instance)
            .ok_or(Rejection::UnknownInstance)?;
        let lease = self.store.signing(instance).map_err(|err| {
            Rejection::InstanceUnavailable {
                reason: format!("{err:#}"),
            }
        })?;
        let settings = lease.settings.clone();
        self.leases.insert(instance.clone(), lease);
        Ok(Some(settings))
    }

    fn prepare_credential_sign_in(
        &mut self,
        source: &InstanceId,
        kind: String,
        label: &str,
        running: &BTreeSet<SessionId>,
    ) -> Result<CredentialTarget, Rejection> {
        if self.sign_ins.is_running(source) {
            return Err(Rejection::SignInRunning);
        }
        let target = CredentialTarget::requested(
            self.core.catalog(),
            source,
            kind,
            label,
        )?;
        if let Some(reason) =
            self.core.instance_in_use(&[&target.destination], running)
        {
            return Err(reason);
        }
        self.core
            .settings(source)
            .ok_or(Rejection::UnknownInstance)?;
        let lease = self.store.signing(source).map_err(|err| {
            Rejection::InstanceUnavailable {
                reason: format!("{err:#}"),
            }
        })?;
        self.leases.insert(source.clone(), lease);
        Ok(target)
    }

    async fn save_credential_sign_in(
        &mut self,
        key: Key,
        target: CredentialTarget,
        credential: Credential,
    ) -> Result<(), Stopped> {
        let running = self.running_sessions().await?;
        let (channel, answer) = oneshot::channel();
        let mut reply = Reply {
            removal: None,
            credential: None,
            application: false,
            channel: Some(channel),
        };
        let admitted = self.admit_credential(&target, &credential, &running);
        let result = match admitted {
            Ok(claim) => {
                reply.credential = Some(claim);
                self.providers
                    .share_credential(target.destination.clone(), &credential)
                    .await?
            }
            Err(rejection) => Some(CommandResult::Rejected(rejection)),
        };
        let effects = match result {
            None => self.core.set_credential(
                target.destination.provider,
                credential,
                reply,
            ),
            Some(outcome) => vec![Effect::Reply(reply, outcome)],
        };
        self.perform(effects);
        self.runs.listen(key, answer);
        Ok(())
    }

    fn admit_credential(
        &self,
        target: &CredentialTarget,
        credential: &Credential,
        running: &BTreeSet<SessionId>,
    ) -> Result<CredentialLease, Rejection> {
        if !target.accepts(self.core.catalog(), credential) {
            return Err(Rejection::InvalidCredential {
                reason: String::from(
                    "the provider returned an invalid credential",
                ),
            });
        }
        if let Some(reason) =
            self.core.instance_in_use(&[&target.destination], running)
        {
            return Err(reason);
        }
        self.store
            .claim_credential(target.destination.clone())
            .map_err(|err| Rejection::CredentialNotSaved {
                reason: format!("{err:#}"),
            })
    }

    fn resume_sign_in(
        &mut self,
        requested: &InstanceId,
        adoption: RecoveredAdoption,
    ) -> Result<bool, Rejection> {
        let RecoveredAdoption { account, roster } = adoption;
        let run = self.sign_ins.resume(requested, &account)?;
        let (channel, answer) = oneshot::channel();
        let reply = Reply {
            removal: None,
            credential: None,
            application: false,
            channel: Some(channel),
        };
        let effects =
            self.core
                .saved(roster, Change::SignedIn(account), reply, Ok(()));
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
            Report::Exited(code, status) => self
                .runs
                .exited(key, code, status)
                .map_or_else(Vec::new, |(exit_code, exit_status)| {
                    self.sign_ins.exited(key, exit_code, &exit_status)
                }),
            Report::Expired => self.sign_ins.expired(key),
            Report::Checked(result) => self.sign_ins.checked(key, result),
            Report::Written(result) => match self.runs.written(key, result) {
                Ok(Some((code, status))) => {
                    self.sign_ins.exited(key, code, &status)
                }
                Ok(None) => Vec::new(),
                Err(reason) => self.sign_ins.write_failed(key, reason),
            },
            Report::Listed(result) => self.sign_ins.listed(key, result),
            Report::CredentialLink(url) => {
                self.sign_ins.credential_link(key, url)
            }
            Report::CredentialCreated(result) => {
                self.runs.kill(key);
                self.sign_ins.credential_created(key, result)
            }
        }
    }

    async fn signs_in(
        &mut self,
        effects: Vec<SignInEffect>,
    ) -> Result<(), Stopped> {
        for effect in effects {
            self.sign_in(effect).await?;
        }
        Ok(())
    }

    async fn sign_in(&mut self, effect: SignInEffect) -> Result<(), Stopped> {
        match effect {
            SignInEffect::Spawn { key, settings } => {
                let lease = self.leases.get(&key.instance).cloned();
                self.runs
                    .spawn(key, self.providers.clone(), settings, lease);
            }
            SignInEffect::SpawnCredential(key, target) => {
                if let Some(lease) = self.leases.get(&key.instance) {
                    self.runs.spawn_credential(
                        key,
                        target,
                        self.providers.clone(),
                        lease.settings.clone(),
                    );
                }
            }
            SignInEffect::StoreCredential(key, target, credential) => {
                self.save_credential_sign_in(key, target, credential)
                    .await?;
            }
            SignInEffect::Write(key, code) => {
                self.runs.write(key, code);
            }
            SignInEffect::Kill(key) => self.runs.kill(&key),
            SignInEffect::Check(key, login) => {
                let lease = self.leases.get(&key.instance).cloned();
                self.runs.check(key, login, lease);
            }
            SignInEffect::Refresh(key, account) => {
                let (channel, answer) = oneshot::channel();
                let reply = Reply {
                    removal: None,
                    credential: None,
                    application: false,
                    channel: Some(channel),
                };
                let running = self.running_sessions().await?;
                let effects = self.core.signed_in(
                    key.instance.clone(),
                    account,
                    &running,
                    reply,
                );
                self.perform(effects);
                self.runs.listen(key, answer);
            }
            SignInEffect::Publish(step) => self.publish_sign_in(step),
        }
        Ok(())
    }

    fn publish_sign_in(&mut self, step: SignIn) {
        if let Some(instance) = host::sign_in::release(&step) {
            self.leases.remove(instance);
        }
        crate::log_note(&host::sign_in::note(&step));
        let _ = self.live.send(step);
    }

    async fn running_sessions(
        &mut self,
    ) -> Result<BTreeSet<SessionId>, Stopped> {
        self.relaunching
            .retain(|_, reservation| reservation.strong_count() > 0);
        let mut running: BTreeSet<_> =
            self.relaunching.keys().copied().collect();
        running.extend(self.log.running_sessions().await?);
        Ok(running)
    }

    async fn configure(
        &mut self,
        configure: Configure,
        id: CommandId,
        channel: oneshot::Sender<CommandResult>,
        application: bool,
    ) -> Result<Vec<Effect>, Stopped> {
        if application {
            self.retire_sign_in(configure.instance(self.core.catalog()))
                .await?;
        }
        let removal = configure.removal().map(|_| id);
        let reply = Reply {
            removal,
            credential: None,
            application,
            channel: Some(channel),
        };
        Ok(match configure {
            Configure::Retry(instance) => {
                self.core.retry_instance(instance, reply)
            }
            Configure::SetCredential {
                provider,
                credential,
            } => self.core.set_credential(provider, credential, reply),
            Configure::Remove(instance) => {
                self.remove_instance(instance, id, reply)
            }
        })
    }

    async fn retire_sign_in(
        &mut self,
        target: Option<InstanceId>,
    ) -> Result<(), Stopped> {
        if let Some(instance) = target {
            let effects = self.sign_ins.retire(instance);
            self.signs_in(effects).await?;
        }
        Ok(())
    }

    fn remove_instance(
        &mut self,
        instance: InstanceId,
        id: CommandId,
        reply: Reply,
    ) -> Vec<Effect> {
        match self.store.removal(id) {
            Some(instances) => {
                vec![Effect::Reply(reply, CommandResult::Instances(instances))]
            }
            None => self.core.remove_instance(instance, reply),
        }
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
        self.publish_configuration();
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

    fn publish_configuration(&self) {
        let configuration = self.core.application();
        if *self.configuration.borrow() != configuration {
            self.configuration.send_replace(configuration);
        }
    }

    fn effect(&mut self, effect: Effect) -> Vec<Effect> {
        match effect {
            Effect::Reply(reply, result) => {
                if let Some(channel) = reply.channel {
                    let _ = channel.send(result);
                }
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
                let generation = (job.id, job.instance_id.clone());
                let task = self.refreshes.spawn(refresh(job));
                self.refresh_jobs.insert(task.id(), generation.clone());
                self.refresh_controls.insert(generation.1, task);
                Vec::new()
            }
            Effect::Start(starting) => {
                let providers = self.providers.clone();
                let generation = (starting.id, starting.instance.clone());
                let control = crate::start::Control::default();
                let observed = control.clone();
                let task = self.starts.spawn(async move {
                    start(&providers, starting, &control).await
                });
                self.start_jobs.insert(task.id(), generation.clone());
                self.start_controls
                    .insert(generation.1, (generation.0, observed, task));
                Vec::new()
            }
            Effect::Retire {
                instance,
                id,
                adapter,
                stage,
            } => {
                self.retire(instance, id, adapter, stage);
                Vec::new()
            }
            Effect::Log(note) => {
                crate::log_note(&note);
                Vec::new()
            }
        }
    }

    fn cancel_jobs(&mut self, instance: &InstanceId) -> Option<Instance> {
        if let Some(task) = self.refresh_controls.remove(instance) {
            task.abort();
        }
        self.start_controls
            .remove(instance)
            .and_then(|(_, control, task)| {
                let adapter = control.cancel();
                task.abort();
                adapter
            })
    }

    fn retire(
        &mut self,
        instance: InstanceId,
        id: u64,
        adapter: Option<Instance>,
        stage: agents::Retirement,
    ) {
        let starting = if self.core.retirement_is_current(&instance, id) {
            self.cancel_jobs(&instance)
        } else {
            None
        };
        let retiring = adapter.or(starting);
        if let Some(adapter) = &retiring {
            self.core.retiring_adapter(&instance, id, adapter.clone());
        }
        self.retirements.spawn(async move {
            let success = terminate(retiring, stage).await;
            (instance, id, stage, success)
        });
    }

    fn adopt_saved(
        &mut self,
        current: Roster,
        change: Change,
        mut reply: Reply,
    ) -> Vec<Effect> {
        if reply.application {
            if let Some(channel) = reply.channel.take() {
                let _ = channel.send(CommandResult::Instances(
                    self.core.instances_of(&current),
                ));
            }
            self.core.adopted_change(current, change, reply)
        } else {
            self.core.saved(current, change, reply, Ok(()))
        }
    }

    fn save(
        &mut self,
        roster: Roster,
        change: Change,
        reply: Reply,
    ) -> Vec<Effect> {
        let saved = match &change {
            Change::SignedIn(adoption) => self
                .leases
                .get(&adoption.from)
                .context("the sign-in no longer has its private home")
                .and_then(|lease| self.store.adopt(lease, adoption.clone())),
            Change::Credential(_) | Change::Added(_) | Change::Removed(_) => {
                self.store.save_change(
                    &roster,
                    &change,
                    reply.credential.as_ref(),
                    reply.removal.map(|id| {
                        (id, |current: &Roster| self.core.instances_of(current))
                    }),
                )
            }
        };
        match saved {
            Ok(current) => self.adopt_saved(current, change, reply),
            Err(err) => {
                self.core
                    .saved(roster, change, reply, Err(format!("{err:#}")))
            }
        }
    }

    fn move_instance(
        &self,
        source: InstanceId,
        incoming: bool,
    ) -> Result<(InstanceId, proto::Instance, Providers), String> {
        let id = if incoming {
            self.core.move_instance(&source)?
        } else {
            source
        };
        let settings = self.core.settings(&id).ok_or_else(|| {
            String::from("the source provider account is absent")
        })?;
        Ok((id, settings, self.providers.clone()))
    }

    async fn register_arrival(
        &mut self,
        session: SessionId,
        id: proto::movement::MoveId,
    ) -> Result<(), Stopped> {
        if self.arrivals.get(&session) == Some(&id) {
            return Ok(());
        }
        let history = self.log.finish_history(session).await?;
        if let Some(created) =
            host::restore::created(&history).into_iter().next()
        {
            self.restore_session(created, &history).await?;
        }
        Ok(())
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
        let events = events_of(id, history);
        let recovery = Recovery::of(choice, &events);
        let conversation = self.conversation(id, recovery.talked).await?;
        recover(&self.log, id, recovery.steps()).await?;
        if host::restore::finish_interrupted(&events) {
            self.log.append(id, EventKind::FinishInterrupted).await?;
        }
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
        if let Some(arrived) = host::movement::arrival_id(&recovered) {
            self.arrivals.insert(id, arrived);
        }
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

fn complete_job<T>(
    jobs: &mut BTreeMap<Id, (u64, InstanceId)>,
    result: Result<(Id, T), tokio::task::JoinError>,
    failed: impl FnOnce(u64, InstanceId, String) -> T,
) -> Option<T> {
    match result {
        Ok((task, completed)) => {
            jobs.remove(&task);
            Some(completed)
        }
        Err(err) => jobs.remove(&err.id()).map(|(id, instance)| {
            let reason = if err.is_cancelled() {
                "the instance job was cancelled"
            } else {
                "the instance job failed"
            };
            failed(id, instance, String::from(reason))
        }),
    }
}

async fn refresh(job: Job<Instance>) -> Refreshed {
    Refreshed {
        id: job.id,
        instance: job.instance_id,
        configured: job.adapter.configure(job.credential).await,
    }
}

async fn start(
    providers: &Providers,
    starting: Starting,
    control: &crate::start::Control,
) -> Started<Instance> {
    let Starting {
        mode,
        id,
        instance,
        settings,
    } = starting;
    let launcher = providers.launch(&instance, settings, mode, control).await;
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

#[cfg(test)]
mod tests;

async fn terminate(
    retiring: Option<Instance>,
    stage: agents::Retirement,
) -> bool {
    match retiring {
        Some(adapter) => tokio::time::timeout(
            std::time::Duration::from_secs(4),
            adapter.retire(stage),
        )
        .await
        .is_ok_and(|result| result.is_ok()),
        None => true,
    }
}
