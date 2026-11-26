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
use host::restore::{Created, Recovery, Step};

use agents::{ClientName, Credential, Effort, ModelId, Models, Provider};
use home::HomeRefusal;
use host::log::Event;
use host::registry::{
    Catalog, InstanceCommand, InstanceId, InstanceRefusal, InstanceResult,
    Instances, Roster,
};
use host::session::{
    EventKind, SessionId, SessionRefusal, SessionResult, StartAt,
};
use host::sign_in::{CredentialTarget, Effect as SignInEffect, Key, SignIns};
use host::sign_in::{SignIn, SignInCommand, SignInRefusal, SignInResult};
use link::CommandId;
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

const SIGN_IN_BUFFER: usize = 64;

pub struct Watch {
    pub running: Vec<SignIn>,
    pub sign_ins: broadcast::Receiver<SignIn>,
    pub offers: watch::Receiver<Instances>,
}

pub type Launcher = host::registry::Launcher<Instance>;

struct Reply {
    removal: Option<CommandId>,
    credential: Option<CredentialLease>,
    channel: oneshot::Sender<InstanceResult>,
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
    workspaces: crate::Workspaces,
    inbox: mpsc::Sender<RegistryMsg>,
    home: HomeClone,
}

enum RegistryMsg {
    Refreshed(Refreshed),
    Started(Started<Instance>),
    Start {
        request: StartRequest,
        placed: Box<Result<Placement, HomeRefusal>>,
        reply: StartReply,
    },
    BranchReady(Box<BranchReady>),
    Continue {
        session: SessionId,
        reply: oneshot::Sender<Result<Relaunch, SessionRefusal>>,
    },
    Find {
        session: SessionId,
        reply: oneshot::Sender<Option<Session>>,
    },
    Instances {
        reply: oneshot::Sender<(Instances, Catalog)>,
    },
    Configure {
        configure: InstanceCommand,
        id: CommandId,
        reply: oneshot::Sender<InstanceResult>,
    },
    SignIn {
        command: SignInCommand,
        reply: oneshot::Sender<SignInResult>,
    },
    Watch {
        reply: oneshot::Sender<Watch>,
    },
    Run(Key, Report),
}

struct RegistryActor {
    workspaces: crate::Workspaces,
    log: Log,
    store: Store,
    core: Core,
    providers: Providers,
    home: HomeClone,
    refreshes: JoinSet<Refreshed>,
    starts: JoinSet<Started<Instance>>,
    refresh_jobs: BTreeMap<Id, (u64, InstanceId)>,
    start_jobs: BTreeMap<Id, (u64, InstanceId)>,
    relaunching: BTreeMap<SessionId, Weak<()>>,
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
        workspaces: crate::Workspaces,
    ) -> Result<(Self, watch::Receiver<bool>), Stopped> {
        let mut core = Core::new(stored, providers.catalog());
        let starts = core.boot();
        let (ready, initialized) = watch::channel(false);
        let mut actor = RegistryActor::new(
            log, store, core, providers, home, ready, workspaces,
        );
        actor.perform_depth_first(
            starts.into_iter().map(Effect::Start).collect(),
        );
        actor.restore().await?;
        Ok((actor.spawn(), initialized))
    }

    pub async fn start(
        &self,
        instance: InstanceId,
        at: StartAt,
        choice: (Option<ModelId>, Option<Effort>),
        by: ClientName,
    ) -> Result<SessionResult, Stopped> {
        let placed = self.home.place(at).await?;
        if let Err(reason) =
            self.home.check_workspaces(&placed, &self.workspaces).await
        {
            return Ok(SessionResult::Refused(
                SessionRefusal::WorkspaceFailure { reason },
            ));
        }
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

    pub async fn relaunch(
        &self,
        session: SessionId,
    ) -> Result<Result<Relaunch, SessionRefusal>, Stopped> {
        self.ask(|reply| RegistryMsg::Continue { session, reply })
            .await
    }

    pub async fn find(
        &self,
        session: SessionId,
    ) -> Result<Option<Session>, Stopped> {
        self.ask(|reply| RegistryMsg::Find { session, reply }).await
    }

    pub async fn instances(&self) -> Result<(Instances, Catalog), Stopped> {
        self.ask(|reply| RegistryMsg::Instances { reply }).await
    }

    pub async fn configure(
        &self,
        configure: InstanceCommand,
        id: CommandId,
    ) -> Result<InstanceResult, Stopped> {
        self.ask(|reply| RegistryMsg::Configure {
            configure,
            id,
            reply,
        })
        .await
    }

    pub async fn sign_in(
        &self,
        command: SignInCommand,
    ) -> Result<SignInResult, Stopped> {
        self.ask(|reply| RegistryMsg::SignIn { command, reply })
            .await
    }

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
        workspaces: crate::Workspaces,
    ) -> Self {
        Self {
            workspaces,
            log,
            store,
            offers: watch::channel(core.instances()).0,
            core,
            providers,
            home,
            refreshes: JoinSet::new(),
            starts: JoinSet::new(),
            refresh_jobs: BTreeMap::new(),
            start_jobs: BTreeMap::new(),
            relaunching: BTreeMap::new(),
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
        let workspaces = self.workspaces.clone();
        tokio::spawn(self.run(messages));
        Registry {
            workspaces,
            inbox,
            home,
        }
    }

    async fn run(
        mut self,
        mut messages: mpsc::Receiver<RegistryMsg>,
    ) -> Result<Infallible, Stopped> {
        loop {
            let msg = tokio::select! {
                msg = messages.recv() => msg.ok_or(Stopped {})?,
                Some(result) = self.refreshes.join_next_with_id() => {
                    match complete_job(&mut self.refresh_jobs, result, |id, instance, reason| Refreshed {
                        id, instance, configured: Err(reason),
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
            RegistryMsg::Refreshed(result) => {
                let effects = self.core.refreshed(result);
                self.perform_depth_first(effects);
            }
            RegistryMsg::Started(result) => {
                let effects = self.core.started(result);
                self.perform_depth_first(effects);
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
                let _ = reply.send(self.core.relaunch(session).map(
                    |(handle, adapter, models)| {
                        let reservation = self
                            .relaunching
                            .get(&session)
                            .and_then(Weak::upgrade)
                            .unwrap_or_default();
                        self.relaunching
                            .insert(session, Arc::downgrade(&reservation));
                        (handle, adapter, models, reservation)
                    },
                ));
            }
            RegistryMsg::Find { session, reply } => {
                let _ = reply.send(self.core.find(session));
            }
            RegistryMsg::Instances { reply } => {
                let _ = reply
                    .send((self.core.instances(), self.core.catalog().clone()));
            }
            RegistryMsg::Configure {
                configure,
                id,
                reply,
            } => {
                let effects = self.configure(configure, id, reply).await?;
                self.perform_depth_first(effects);
            }
            RegistryMsg::SignIn { command, reply } => {
                let (result, effects) = self.sign_in_command(command).await?;
                self.signs_in(effects).await?;
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
                self.signs_in(effects).await?;
            }
        }
        Ok(())
    }

    async fn sign_in_command(
        &mut self,
        command: SignInCommand,
    ) -> Result<(SignInResult, Vec<SignInEffect>), Stopped> {
        Ok(match command {
            SignInCommand::Start { instance } => {
                let running = self.running_sessions().await?;
                self.start_sign_in(instance, &running)
            }
            SignInCommand::StartCredential {
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
                    Err(refusal) => {
                        (SignInResult::Refused(refusal), Vec::new())
                    }
                }
            }
            SignInCommand::SubmitCode { instance, code } => {
                self.sign_ins.submit(instance, code)
            }
            SignInCommand::Cancel { instance } => {
                self.sign_ins.cancel(instance)
            }
        })
    }

    fn start_sign_in(
        &mut self,
        instance: InstanceId,
        running: &BTreeSet<SessionId>,
    ) -> (SignInResult, Vec<SignInEffect>) {
        match self.prepare_sign_in(&instance, running) {
            Ok(Some(settings)) => self.sign_ins.start(instance, Some(settings)),
            Ok(None) => (SignInResult::Accepted, Vec::new()),
            Err(refusal) => (SignInResult::Refused(refusal), Vec::new()),
        }
    }

    fn prepare_sign_in(
        &mut self,
        instance: &InstanceId,
        running: &BTreeSet<SessionId>,
    ) -> Result<Option<host::registry::Instance>, SignInRefusal> {
        if self.sign_ins.is_running(instance) {
            return Err(SignInRefusal::SignInRunning);
        }
        let recovered = self
            .store
            .resume_adoption(|account| {
                self.core
                    .instance_in_use(&[&account.from, &account.to], running)
                    .is_none()
            })
            .map_err(|err| SignInRefusal::CredentialNotSaved {
                reason: format!("{err:#}"),
            })?;
        if recovered.map_or(Ok(false), |adoption| {
            self.resume_sign_in(instance, adoption)
        })? {
            return Ok(None);
        }
        self.core
            .settings(instance)
            .ok_or(SignInRefusal::UnknownInstance)?;
        let lease = self.store.signing(instance).map_err(|err| {
            SignInRefusal::InstanceUnavailable {
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
    ) -> Result<CredentialTarget, SignInRefusal> {
        if self.sign_ins.is_running(source) {
            return Err(SignInRefusal::SignInRunning);
        }
        let target = CredentialTarget::requested(
            self.core.catalog(),
            source,
            kind,
            label,
        )?;
        if let Some(sessions) =
            self.core.instance_in_use(&[&target.destination], running)
        {
            return Err(SignInRefusal::InstanceInUse { sessions });
        }
        self.core
            .settings(source)
            .ok_or(SignInRefusal::UnknownInstance)?;
        let lease = self.store.signing(source).map_err(|err| {
            SignInRefusal::InstanceUnavailable {
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
            channel,
        };
        let admitted = self.admit_credential(&target, &credential, &running);
        let result = match admitted {
            Ok(claim) => {
                reply.credential = Some(claim);
                self.providers
                    .share_credential(target.destination.clone(), &credential)
                    .await
            }
            Err(refusal) => Err(refusal),
        };
        let effects = match result {
            Ok(()) => self.core.set_credential(
                target.destination.provider,
                credential,
                reply,
            ),
            Err(reason) => {
                vec![Effect::Reply(reply, InstanceResult::Refused(reason))]
            }
        };
        self.perform_depth_first(effects);
        self.runs.listen(key, answer);
        Ok(())
    }

    fn admit_credential(
        &self,
        target: &CredentialTarget,
        credential: &Credential,
        running: &BTreeSet<SessionId>,
    ) -> Result<CredentialLease, InstanceRefusal> {
        if !target.accepts(self.core.catalog(), credential) {
            return Err(InstanceRefusal::InvalidCredential {
                reason: String::from(
                    "the provider returned an invalid credential",
                ),
            });
        }
        if let Some(sessions) =
            self.core.instance_in_use(&[&target.destination], running)
        {
            return Err(InstanceRefusal::InstanceInUse { sessions });
        }
        self.store
            .claim_credential(target.destination.clone())
            .map_err(|err| InstanceRefusal::CredentialNotSaved {
                reason: format!("{err:#}"),
            })
    }

    fn resume_sign_in(
        &mut self,
        requested: &InstanceId,
        adoption: RecoveredAdoption,
    ) -> Result<bool, SignInRefusal> {
        let RecoveredAdoption { account, roster } = adoption;
        let run = self.sign_ins.resume(requested, &account)?;
        let (channel, answer) = oneshot::channel();
        let reply = Reply {
            removal: None,
            credential: None,
            channel,
        };
        let effects =
            self.core
                .saved(roster, Change::SignedIn(account), reply, Ok(()));
        self.perform_depth_first(effects);
        let resumed = run.is_some();
        if let Some(key) = run {
            self.runs.listen(key, answer);
        }
        Ok(resumed)
    }

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
            Report::Listed(result) => {
                self.sign_ins.listed(key, result, &self.offers.borrow())
            }
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
                    channel,
                };
                let running = self.running_sessions().await?;
                let effects = self.core.signed_in(
                    key.instance.clone(),
                    account,
                    &running,
                    reply,
                );
                self.perform_depth_first(effects);
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
        configure: InstanceCommand,
        id: CommandId,
        channel: oneshot::Sender<InstanceResult>,
    ) -> Result<Vec<Effect>, Stopped> {
        let removal =
            matches!(configure, InstanceCommand::Remove { .. }).then_some(id);
        let reply = Reply {
            removal,
            credential: None,
            channel,
        };
        Ok(match configure {
            InstanceCommand::SetCredential {
                provider,
                credential,
            } => self.core.set_credential(provider, credential, reply),
            InstanceCommand::Remove { instance } => {
                if self.store.removal(id) {
                    return Ok(vec![Effect::Reply(
                        reply,
                        InstanceResult::Accepted,
                    )]);
                }
                let running = self.running_sessions().await?;
                self.core.remove_instance(instance, &running, reply)
            }
        })
    }

    fn perform_depth_first(&mut self, mut effects: Vec<Effect>) {
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
                let _ = reply.channel.send(result);
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
                self.refresh_jobs.insert(task.id(), generation);
                Vec::new()
            }
            Effect::Start(starting) => {
                let providers = self.providers.clone();
                let generation = (starting.id, starting.instance.clone());
                let task = self
                    .starts
                    .spawn(async move { start(&providers, starting).await });
                self.start_jobs.insert(task.id(), generation);
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
                    reply.removal,
                )
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
        for (created, events) in host::restore::histories(history) {
            self.restore_session(created, events).await?;
        }
        Ok(())
    }

    async fn restore_session(
        &mut self,
        created: Created,
        mut events: Vec<Event>,
    ) -> Result<(), Stopped> {
        let Created {
            session: id,
            instance,
            directory,
            choice,
        } = created;
        let recovery = Recovery::of(choice, &events);
        let conversation = self.conversation(id, recovery.talked).await?;
        self.recover(id, recovery.steps, &mut events).await?;
        let told = self.told(&instance.provider);
        let session = Session::restore(
            id,
            directory,
            recovery.choice,
            told,
            (None, conversation),
            &events,
            self.log.clone(),
        );
        self.core.admit(id, instance, session);
        Ok(())
    }

    async fn recover(
        &self,
        id: SessionId,
        steps: Vec<Step>,
        events: &mut Vec<Event>,
    ) -> Result<(), Stopped> {
        for step in steps {
            let kind = match step {
                Step::Append(kind) => kind,
                Step::Barrier => {
                    self.log.barrier().await?;
                    continue;
                }
                Step::Snapshot(turn) => {
                    let snapshot = self
                        .workspaces
                        .snapshot(&turn.workspace, id, turn.attempt.clone())
                        .await
                        .map_err(|err| err.to_string());
                    turn.snapshotted(snapshot)
                }
            };
            events.push(self.log.record(id, kind).await?);
        }
        Ok(())
    }

    fn told(&self, provider: &Provider) -> Told {
        Told {
            workspaces: self.workspaces.clone(),
            home: self.home.clone(),
            instructions_file: self.providers.instructions_file(provider),
        }
    }

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
        placed: Result<Placement, HomeRefusal>,
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
                    let workspace = self
                        .workspaces
                        .create(&directory, id, base.as_deref())
                        .await;
                    StartReport::Workspace(id, workspace)
                }
                StartEffect::Snapshot { session, workspace } => {
                    StartReport::Snapshot(
                        self.workspaces
                            .snapshot(
                                &workspace,
                                session,
                                vcs::command::SnapshotAttempt(String::from(
                                    "initial",
                                )),
                            )
                            .await,
                    )
                }
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
                        self.told(&provider),
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
        [workspace, snapshot]: [EventKind; 2],
    ) -> Result<[Event; 2], Stopped> {
        for event in before {
            self.log.append(id, event).await?;
        }
        let workspace_event = self.log.record(id, workspace).await?;
        let snapshot_event = self.log.record(id, snapshot).await?;
        Ok([workspace_event, snapshot_event])
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

#[cfg(test)]
mod tests;
