//! The registry's decisions about sessions and instances.
//!
//! They are whether a session's start or a `Continue` may go ahead and why
//! not, the model and effort a session starts with, which session an id
//! names, whether each instance is available, how an instance comes to be
//! and is removed, and how a credential change is stored and the instance
//! refreshed.
//!
//! An instance is one account of a provider, keyed by the provider and the
//! account, which the provider names. A host may have several of one
//! provider, each with its own credential, its own private home and its
//! own CLI's agents. An instance comes to be when its account is known: a
//! credential names it in its form, and a sign-in when the CLI reports who
//! it signed in as. Until then a sign-in runs in its provider's own
//! instance ([`Account::Own`]), which every provider has ([`defaults`]):
//! once the CLI names the account, the own instance's home becomes that
//! account's instance and the provider gets a new own one.
//!
//! `daemon` runs it: it owns the store, the sessions' tasks and the
//! instances' agents. It asks the registry, performs each [`Effect`] in
//! order, and reports how each went. The registry holds each provider
//! instance `S`, each session's handle `H` and each answer's reply `R` as
//! opaque values; it knows no provider by name.
//!
//! A credential change refreshes its instance, which applies the
//! credential and discovers its models. Until that ends
//! the instance is unavailable. One refresh per instance runs at a time;
//! changes made meanwhile coalesce into one more, and every change waiting
//! gets the answer of the last. An added instance is unavailable until its
//! CLI has started.

use agents::Configuration;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;

use crate::log::Event;
use crate::session::{SessionId, SessionRefusal, SessionResult};
use agents::{Account, Auth, Credential, Effort, ModelId, Models, Provider};
use home::IssueId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Note;
use crate::assignment::assigned;
use crate::prelude::*;
use crate::session::{Choice, choose};
use crate::start::Mode;
use crate::workspace::GitError;

mod instance;
pub use instance::{
    Availability, Catalog, Cli, Instance, InstanceCommand, InstanceId,
    InstanceRefusal, InstanceResult, InstanceStatus, Instances, ProviderNote,
    Roster,
};

/// What a CLI signs in with when the host stores no credential for its
/// instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnSignIn {
    /// The private home is signed in, from the app or with `conductord
    /// login <instance>`.
    SignedIn,
    /// A CLI given on the command line, signed in however it is.
    Unmanaged,
    /// Nothing: the instance is unavailable until it gets a credential.
    Missing,
}

impl OwnSignIn {
    fn auth(self) -> Result<Auth, String> {
        match self {
            Self::SignedIn => Ok(Auth::SignedIn),
            Self::Unmanaged => Ok(Auth::Unmanaged),
            Self::Missing => Err(String::from(NO_CREDENTIAL)),
        }
    }
}

const NO_CREDENTIAL: &str = "no credential configured";
const REFRESHING: &str = "refreshing the instance's models";
const STARTING: &str = "starting the instance's CLI";

/// A session defaults to the credential's model, then the configured
/// model, then the CLI's default.
pub(crate) fn offered(
    configured: Configuration,
    chosen: Option<&ModelId>,
) -> Result<Models, String> {
    let mut models = configured.models?;
    models.default = configured
        .named
        .or_else(|| chosen.cloned())
        .or(models.default);
    Ok(models)
}

/// How the host runs an instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launcher<S> {
    /// The CLI runs, `start` starts its agents, and it signs in as `own`
    /// says when the host stores no credential for it.
    Running {
        start: S,
        own: OwnSignIn,
        chosen: Option<ModelId>,
        models: Result<Models, String>,
    },
    /// The host can't run this instance's sessions, for this reason.
    Unavailable(String),
}

/// What the store holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stored {
    Opened(Roster),
    /// There is no store yet: the host is fresh.
    Fresh,
    /// The file exists and doesn't open, for this reason. The host runs
    /// no instance until a Clear moves it aside.
    Unreadable(String),
}

impl Stored {
    /// The instances the host has: the stored ones, with each provider's
    /// own one. A store that doesn't open shows the own ones, unavailable,
    /// so removing one of them can move it aside.
    #[must_use]
    pub fn roster(&self, catalog: &Catalog) -> Roster {
        let mut roster = defaults(catalog);
        if let Self::Opened(stored) = self {
            roster.0.extend(stored.0.clone());
        }
        roster
    }
}

/// What a fresh host has: each provider's own instance, with no
/// credential.
#[must_use]
pub fn defaults(catalog: &Catalog) -> Roster {
    Roster(
        catalog
            .0
            .keys()
            .map(|provider| {
                (InstanceId::own(provider.clone()), Instance::default())
            })
            .collect(),
    )
}

/// The name of an instance's private home and work dir.
///
/// That is its provider's for the own instance, and otherwise the
/// provider's with the start of a hash of the account, which is safe in a
/// path whatever the account is.
#[must_use]
pub fn dir(instance: &InstanceId) -> String {
    let provider = instance.provider.name();
    match &instance.account {
        Account::Own => String::from(provider),
        Account::Named(account) => {
            let hash = Sha256::digest(account.as_bytes());
            let hex = data_encoding::HEXLOWER.encode(&hash);
            format!("{provider}-{}", hex.get(..16).unwrap_or(&hex))
        }
    }
}

/// The instance of `provider` a person names on the host's command line:
/// the one of `account`, or without one the provider's own.
///
/// # Errors
///
/// The roster has no such instance; the error lists the accounts it has.
pub fn picked(
    roster: &Roster,
    provider: &Provider,
    account: Option<&str>,
) -> Result<InstanceId, String> {
    let id = sign_in_instance(provider.clone(), account);
    if roster.of(&id).is_some() {
        return Ok(id);
    }
    let known = roster
        .0
        .keys()
        .filter(|known| &known.provider == provider)
        .filter_map(|known| match &known.account {
            Account::Named(named) => Some(named.clone()),
            Account::Own => None,
        })
        .collect::<Vec<String>>();
    Err(if known.is_empty() {
        format!("this host has no instance {id}")
    } else {
        format!(
            "this host has no instance {id}; its {} accounts are {}",
            provider.name(),
            known.join(", ")
        )
    })
}

/// Which instance a terminal sign-in requests.
#[must_use]
pub fn sign_in_instance(
    provider: Provider,
    account: Option<&str>,
) -> InstanceId {
    match account {
        Some(named) => InstanceId::named(provider, named),
        None => InstanceId::own(provider),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountAdoption {
    pub from: InstanceId,
    pub to: InstanceId,
}

pub enum AdoptionReplay {
    Install(Roster),
    Committed(Roster),
}

impl AccountAdoption {
    /// A new account named by a confirmed sign-in.
    #[must_use]
    pub fn named(from: &InstanceId, reported: Option<Account>) -> Option<Self> {
        let account = reported.filter(|named| named != &from.account)?;
        Some(Self {
            from: from.clone(),
            to: InstanceId {
                provider: from.provider.clone(),
                account,
            },
        })
    }

    /// The result of retrying the source instance's sign-in.
    #[must_use]
    pub fn destination_for(
        &self,
        requested: &InstanceId,
    ) -> Option<&InstanceId> {
        (&self.from == requested).then_some(&self.to)
    }

    /// The adopted account takes the source's settings and signs in through
    /// its private home. The provider's own instance remains available.
    #[must_use]
    pub fn plan(&self, roster: &Roster) -> Option<Roster> {
        if self.from.provider != self.to.provider || self.from == self.to {
            return None;
        }
        let settings = roster.of(&self.from)?;
        let mut next = roster.clone();
        if self.from.account != Account::Own {
            next.0.remove(&self.from);
        }
        next.0.insert(
            self.to.clone(),
            Instance {
                credential: None,
                ..settings.clone()
            },
        );
        Some(next)
    }

    /// Resolves where to resume an adoption against the current roster.
    /// # Errors
    /// Neither a source nor a destination instance remains.
    pub fn replay(
        &self,
        roster: Roster,
    ) -> Result<AdoptionReplay, &'static str> {
        if let Some(next) = self.plan(&roster) {
            return Ok(AdoptionReplay::Install(next));
        }
        if roster.of(&self.to).is_some() {
            return Ok(AdoptionReplay::Committed(roster));
        }
        Err("the adoption has neither a source nor a destination instance")
    }
}

/// The sessions whose agents run, or start, after `history`.
#[must_use]
pub fn running(history: &[Event]) -> BTreeSet<SessionId> {
    let mut running = BTreeSet::new();
    for event in history {
        match crate::restore::agent_activity(&event.kind) {
            Some(true) => {
                running.insert(event.session);
            }
            Some(false) => {
                running.remove(&event.session);
            }
            None => {}
        }
    }
    running
}

/// The session `instance` has on `issue` after `history`, if it has one
/// it hasn't removed: an instance works on an issue in one session.
#[must_use]
fn on_issue(
    history: &[Event],
    issue: IssueId,
    instance: &InstanceId,
) -> Option<SessionId> {
    assigned(history)
        .into_iter()
        .find(|(_, assigned)| assigned.is(issue, instance))
        .map(|(session, _)| session)
}

/// What the session's log says when the issue's brief, its first
/// message, didn't reach the agent: `None` when it did.
#[must_use]
fn unbriefed(answer: Option<&SessionResult>) -> Option<String> {
    let why = match answer {
        Some(SessionResult::Accepted) => return None,
        Some(SessionResult::Refused(refusal)) => refusal.label(),
        None => String::from("the session stopped"),
        Some(other) => format!("the session answered {other:?}"),
    };
    Some(format!(
        "The issue's brief was not sent: {why}. Send it again from the issue."
    ))
}

/// A scheduled configuration of one provider instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job<S> {
    pub id: u64,
    pub instance_id: InstanceId,
    pub adapter: S,
    pub credential: Option<Credential>,
}

/// How the refresh `id` of `instance` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refreshed {
    pub id: u64,
    pub instance: InstanceId,
    pub configured: Result<Configuration, String>,
}

/// The settings and discovery policy for one instance launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Starting {
    pub mode: Mode,
    pub id: u64,
    pub instance: InstanceId,
    pub settings: Instance,
}

/// How starting `id` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started<S> {
    pub id: u64,
    pub instance: InstanceId,
    pub launcher: Launcher<S>,
}

/// What a store save changes, which says what follows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Credential(InstanceId),
    Added(InstanceId),
    /// A sign-in of `from` named `to`'s account. The shell commits
    /// ownership of the private directories with the roster change.
    SignedIn(AccountAdoption),
    Removed(InstanceId),
}

impl Change {
    /// Applies a credential or removal to the current roster, using the
    /// planned roster for the credential's form values.
    #[must_use]
    pub fn apply(&self, current: &mut Roster, planned: &Roster) -> bool {
        match self {
            Self::Credential(instance) => {
                let Some(settings) = current.0.get_mut(instance) else {
                    return false;
                };
                settings.credential =
                    planned.of(instance).and_then(|s| s.credential.clone());
            }
            Self::Added(instance) => {
                let Some(settings) = planned.of(instance) else {
                    return false;
                };
                let entry = current
                    .0
                    .entry(instance.clone())
                    .or_insert_with(|| settings.clone());
                entry.credential.clone_from(&settings.credential);
            }
            Self::Removed(instance) => {
                current.0.remove(instance);
            }
            Self::SignedIn(_) => return false,
        }
        true
    }
}

/// What the shell does next, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect<S, R> {
    Reply(R, InstanceResult),
    /// Move the store that doesn't open aside, and report
    /// [`Registry::moved_aside`].
    MoveAside {
        instance: InstanceId,
        reply: R,
    },
    /// Store `roster` in place of what the store holds, and report
    /// [`Registry::saved`].
    Save {
        roster: Roster,
        change: Change,
        reply: R,
    },
    /// Run the refresh, and report [`Registry::refreshed`].
    Refresh(Job<S>),
    /// Start the instance's CLI, and report [`Registry::started`].
    Start(Starting),
    Log(Note),
}

/// A session that may start: its instance's starter, the model and effort
/// it runs with, and what the instance offers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Launch<S> {
    start: S,
    choice: Choice,
    offered: Models,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Refresh<S, R> {
    /// The latest job, which runs once the one running has ended unless it
    /// is that one, and the changes waiting for it.
    Pending {
        active: u64,
        latest: Job<S>,
        replies: Vec<R>,
    },
    /// Changes arriving during launch need one live refresh, even if the
    /// last request repeats the credential the boot probe used.
    Starting {
        id: u64,
        refresh_requested: bool,
        replies: Vec<R>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry<S, H, R> {
    roster: Roster,
    /// Why the store doesn't open, while it doesn't.
    unreadable: Option<String>,
    launchers: BTreeMap<InstanceId, Launcher<S>>,
    /// How each provider presents itself, and the credentials it takes.
    catalog: Catalog,
    /// An instance with none is idle.
    refreshes: BTreeMap<InstanceId, Refresh<S, R>>,
    sessions: BTreeMap<SessionId, (InstanceId, H)>,
    jobs: u64,
}

impl<S: Clone, H: Clone, R> Registry<S, H, R> {
    /// A registry with no session yet. `stored` is what the store held
    /// when it opened.
    #[must_use]
    pub fn new(stored: &Stored, catalog: Catalog) -> Self {
        let unreadable = match stored {
            Stored::Unreadable(reason) => Some(reason.clone()),
            Stored::Opened(_) | Stored::Fresh => None,
        };
        Self {
            roster: stored.roster(&catalog),
            unreadable,
            launchers: BTreeMap::new(),
            catalog,
            refreshes: BTreeMap::new(),
            sessions: BTreeMap::new(),
            jobs: 0,
        }
    }

    /// Starts the instances already in the roster.
    pub fn boot(&mut self) -> Vec<Starting> {
        let roster = self.roster.0.clone();
        roster
            .into_iter()
            .map(|(instance, settings)| {
                self.starting(instance, settings, Vec::new(), Mode::Boot)
            })
            .collect()
    }

    #[must_use]
    pub const fn offers_settled(&self) -> bool {
        self.refreshes.is_empty()
    }

    /// What `Welcome` tells a client of each provider that never changes.
    #[must_use]
    pub const fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// What `Welcome` tells a client the host can run.
    #[must_use]
    pub fn instances(&self) -> Instances {
        let roster = &self.roster;
        Instances(
            roster
                .0
                .keys()
                .map(|id| {
                    (
                        id.clone(),
                        InstanceStatus {
                            availability: self.availability(
                                id,
                                roster.of(id).and_then(|settings| {
                                    settings.credential.as_ref()
                                }),
                            ),
                        },
                    )
                })
                .collect(),
        )
    }

    /// Whether a session may start in a directory that `exists`, on
    /// `instance` with the model and effort asked for.
    ///
    /// # Errors
    ///
    /// Why it may not: no such directory, no such instance, the instance
    /// is unavailable, or it doesn't offer the model or effort.
    fn launch_session(
        &self,
        instance: &InstanceId,
        exists: bool,
        model: Option<ModelId>,
        effort: Option<Effort>,
    ) -> Result<Launch<S>, SessionRefusal> {
        if !exists {
            return Err(SessionRefusal::NoSuchDirectory);
        }
        if self.roster.of(instance).is_none() {
            return Err(SessionRefusal::UnknownInstance);
        }
        let (start, _, offered) = self.ready(instance).map_err(unavailable)?;
        let choice = choose(&offered, model, effort)
            .map_err(|reason| SessionRefusal::NotOffered { reason })?;
        Ok(Launch {
            start,
            choice,
            offered,
        })
    }

    /// Keeps a session that started or was restored.
    pub fn admit(
        &mut self,
        session: SessionId,
        instance: InstanceId,
        handle: H,
    ) {
        self.sessions.insert(session, (instance, handle));
    }

    /// The session and what its instance offers, for an order that may
    /// start its agent.
    ///
    /// # Errors
    ///
    /// No such session, or its instance is unavailable or gone.
    pub fn relaunch(
        &self,
        session: SessionId,
    ) -> Result<(H, S, Models), SessionRefusal> {
        let (instance, handle) = self
            .sessions
            .get(&session)
            .ok_or(SessionRefusal::UnknownSession)?;
        self.ready(instance)
            .map(|(adapter, _, models)| (handle.clone(), adapter, models))
            .map_err(unavailable)
    }

    #[must_use]
    pub fn find(&self, session: SessionId) -> Option<H> {
        self.sessions
            .get(&session)
            .map(|(_, handle)| handle.clone())
    }

    /// How the host runs `instance`, if it has it: its CLI, model and
    /// credential.
    #[must_use]
    pub fn settings(&self, instance: &InstanceId) -> Option<Instance> {
        self.roster.of(instance).cloned()
    }

    /// The instance's home passed its CLI's sign-in check after a sign-in
    /// from the app, and the CLI says it is `account`'s. On the instance's
    /// own account, or none, its CLI signs in on its own from now on, and
    /// it lists what it offers then. On another, the home becomes that
    /// account's instance, whose CLI then starts. `reply` gets what the
    /// host runs once it has.
    pub fn signed_in(
        &mut self,
        instance: InstanceId,
        account: Option<Account>,
        running: &BTreeSet<SessionId>,
        reply: R,
    ) -> Vec<Effect<S, R>> {
        if let Some((adoption, roster)) =
            AccountAdoption::named(&instance, account).and_then(|adoption| {
                adoption.plan(&self.roster).map(|roster| (adoption, roster))
            })
        {
            if let Some(sessions) =
                self.instance_in_use(&[&adoption.from, &adoption.to], running)
            {
                return Vec::from([refuse(
                    reply,
                    InstanceRefusal::InstanceInUse { sessions },
                )]);
            }
            return self.save(roster, Change::SignedIn(adoption), reply);
        }
        if let Some(Launcher::Running { own, .. }) =
            self.launchers.get_mut(&instance)
        {
            *own = OwnSignIn::SignedIn;
        }
        self.schedule(instance, Vec::from([reply]))
    }

    /// Stores a credential of `provider`. The account its form names is
    /// the instance's: one the host has takes it and refreshes, and
    /// otherwise the instance is added and its CLI started. A store that
    /// doesn't open is left alone, since saving would overwrite the other
    /// instances.
    pub fn set_credential(
        &mut self,
        provider: Provider,
        credential: Credential,
        reply: R,
    ) -> Vec<Effect<S, R>> {
        let named = self
            .catalog
            .of(&provider)
            .ok_or_else(|| format!("this host doesn't run {}", provider.name()))
            .and_then(|info| info.account(&credential));
        let instance = match named {
            Ok(account) => InstanceId { provider, account },
            Err(reason) => {
                return Vec::from([refuse(
                    reply,
                    InstanceRefusal::InvalidCredential { reason },
                )]);
            }
        };
        let mut roster = self.roster.clone();
        let change = if let Some(changed) = roster.0.get_mut(&instance) {
            changed.credential = Some(credential);
            Change::Credential(instance)
        } else {
            roster.0.insert(
                instance.clone(),
                Instance {
                    credential: Some(credential),
                    ..Instance::default()
                },
            );
            Change::Added(instance)
        };
        self.save(roster, change, reply)
    }

    /// Removes an instance on which none of the `running` sessions runs.
    /// Its closed sessions stay, and can't continue. A provider's own
    /// instance stays; a store that doesn't open is moved aside first,
    /// which is how removing one recovers it.
    pub fn remove_instance(
        &mut self,
        instance: InstanceId,
        running: &BTreeSet<SessionId>,
        reply: R,
    ) -> Vec<Effect<S, R>> {
        if self.roster.of(&instance).is_none() {
            return Vec::from([refuse(
                reply,
                InstanceRefusal::UnknownInstance,
            )]);
        }
        if self.unreadable.is_some() {
            return Vec::from([Effect::MoveAside { instance, reply }]);
        }
        if instance.account == Account::Own {
            return Vec::from([Self::answer(reply)]);
        }
        if let Some(sessions) = self.instance_in_use(&[&instance], running) {
            return Vec::from([refuse(
                reply,
                InstanceRefusal::InstanceInUse { sessions },
            )]);
        }
        let mut roster = self.roster.clone();
        roster.0.remove(&instance);
        self.save(roster, Change::Removed(instance), reply)
    }

    #[must_use]
    pub fn instance_in_use(
        &self,
        instances: &[&InstanceId],
        running: &BTreeSet<SessionId>,
    ) -> Option<u64> {
        let busy = self
            .sessions
            .iter()
            .filter(|(id, (on, _))| {
                instances.contains(&on) && running.contains(id)
            })
            .count();
        (busy > 0).then(|| u64::try_from(busy).unwrap_or(u64::MAX))
    }

    /// How the [`Effect::MoveAside`] went.
    pub fn moved_aside(
        &mut self,
        instance: InstanceId,
        reply: R,
        moved: Result<(), String>,
    ) -> Vec<Effect<S, R>> {
        match moved {
            Ok(()) => {
                self.unreadable = None;
                self.remove_instance(instance, &BTreeSet::new(), reply)
            }
            Err(reason) => Vec::from([not_saved(reply, reason)]),
        }
    }

    /// How the [`Effect::Save`] went.
    pub fn saved(
        &mut self,
        roster: Roster,
        change: Change,
        reply: R,
        saved: Result<(), String>,
    ) -> Vec<Effect<S, R>> {
        if let Err(reason) = saved {
            return Vec::from([not_saved(reply, reason)]);
        }
        self.roster = roster;
        match change {
            Change::Credential(instance) => {
                let note = Note::CredentialStored {
                    instance: instance.clone(),
                };
                logged(note, self.schedule(instance, Vec::from([reply])))
            }
            Change::Added(instance) => {
                let note = Note::InstanceAdded {
                    instance: instance.clone(),
                };
                logged(note, self.launch(instance, Vec::from([reply])))
            }
            Change::SignedIn(AccountAdoption { from, to }) => {
                let mut waiting = self.forget(&to);
                waiting.push(reply);
                let note = Note::InstanceAdded {
                    instance: to.clone(),
                };
                let mut effects = logged(note, self.launch(to, waiting));
                let left = self.forget(&from);
                if from.account == Account::Own {
                    effects.extend(self.launch(from, left));
                } else {
                    effects.extend(Self::answers(left));
                }
                effects
            }
            Change::Removed(instance) => {
                let mut waiting = self.forget(&instance);
                waiting.push(reply);
                logged(
                    Note::InstanceRemoved { instance },
                    Self::answers(waiting),
                )
            }
        }
    }

    /// How an [`Effect::Refresh`] went. A result for a job since
    /// superseded is dropped, and the latest job runs.
    pub fn refreshed(&mut self, refreshed: Refreshed) -> Vec<Effect<S, R>> {
        let Refreshed {
            id,
            instance,
            configured,
        } = refreshed;
        match self.refreshes.remove(&instance) {
            Some(Refresh::Pending {
                active,
                latest,
                replies,
            }) if active == id && latest.id == id => {
                let logged = self.configured(&instance, configured);
                logged.into_iter().chain(Self::answers(replies)).collect()
            }
            Some(Refresh::Pending {
                active,
                latest,
                replies,
            }) if active == id => {
                let job = latest.clone();
                self.refreshes.insert(
                    instance,
                    Refresh::Pending {
                        active: latest.id,
                        latest,
                        replies,
                    },
                );
                Vec::from([Effect::Refresh(job)])
            }
            settled => {
                self.refreshes
                    .extend(settled.map(|refresh| (instance, refresh)));
                Vec::new()
            }
        }
    }

    /// How an [`Effect::Start`] went. The instance runs as the launcher
    /// says; if its credential changed while its CLI started, it
    /// refreshes with the current one. A start for an instance since
    /// removed is dropped.
    pub fn started(&mut self, started: Started<S>) -> Vec<Effect<S, R>> {
        let Started {
            id,
            instance,
            launcher,
        } = started;
        match self.refreshes.remove(&instance) {
            Some(Refresh::Starting {
                id: current,
                refresh_requested,
                replies,
            }) if current == id => {
                self.launchers.insert(instance.clone(), launcher);
                if refresh_requested {
                    self.schedule(instance, replies)
                } else {
                    Self::answers(replies)
                }
            }
            other => {
                self.refreshes
                    .extend(other.map(|refresh| (instance, refresh)));
                Vec::new()
            }
        }
    }

    /// A Save of `roster`, unless the store doesn't open.
    fn save(
        &self,
        roster: Roster,
        change: Change,
        reply: R,
    ) -> Vec<Effect<S, R>> {
        Vec::from([match &self.unreadable {
            Some(reason) => not_saved(reply, reason.clone()),
            None => Effect::Save {
                roster,
                change,
                reply,
            },
        }])
    }

    /// Drops how the host ran `instance`, and hands back the changes that
    /// waited for it.
    fn forget(&mut self, instance: &InstanceId) -> Vec<R> {
        self.launchers.remove(instance);
        match self.refreshes.remove(instance) {
            Some(
                Refresh::Pending { replies, .. }
                | Refresh::Starting { replies, .. },
            ) => replies,
            None => Vec::new(),
        }
    }

    /// Starts an instance's CLI, in its home as it is now, and answers
    /// `replies` once it has.
    fn launch(
        &mut self,
        instance: InstanceId,
        replies: Vec<R>,
    ) -> Vec<Effect<S, R>> {
        let Some(settings) = self.roster.of(&instance).cloned() else {
            return Self::answers(replies);
        };
        Vec::from([Effect::Start(self.starting(
            instance,
            settings,
            replies,
            Mode::Change,
        ))])
    }

    fn starting(
        &mut self,
        instance: InstanceId,
        settings: Instance,
        replies: Vec<R>,
        mode: Mode,
    ) -> Starting {
        self.jobs = self.jobs.saturating_add(1);
        self.refreshes.insert(
            instance.clone(),
            Refresh::Starting {
                id: self.jobs,
                refresh_requested: false,
                replies,
            },
        );
        Starting {
            mode,
            id: self.jobs,
            instance,
            settings,
        }
    }

    fn configured(
        &mut self,
        instance: &InstanceId,
        result: Result<Configuration, String>,
    ) -> Option<Effect<S, R>> {
        let Launcher::Running { chosen, models, .. } =
            self.launchers.get_mut(instance)?
        else {
            return None;
        };
        *models = result.and_then(|config| offered(config, chosen.as_ref()));
        models.as_ref().err().map(|reason| {
            Effect::Log(Note::RefreshFailed {
                instance: instance.clone(),
                reason: reason.clone(),
            })
        })
    }

    /// Refreshes `instance` with its credential, or, while a refresh
    /// runs, makes it the next. An instance whose CLI is starting takes
    /// it once started, and one the host can't run answers at once.
    fn schedule(
        &mut self,
        instance: InstanceId,
        asked: Vec<R>,
    ) -> Vec<Effect<S, R>> {
        if let Some(Refresh::Starting {
            refresh_requested,
            replies,
            ..
        }) = self.refreshes.get_mut(&instance)
        {
            *refresh_requested = true;
            replies.extend(asked);
            return Vec::new();
        }
        let Some(Launcher::Running { start, .. }) =
            self.launchers.get(&instance)
        else {
            return Self::answers(asked);
        };
        let credential = self.credential(&instance);
        self.jobs = self.jobs.saturating_add(1);
        let latest = Job {
            id: self.jobs,
            instance_id: instance.clone(),
            adapter: start.clone(),
            credential,
        };
        let (active, replies, run) = match self.refreshes.remove(&instance) {
            Some(Refresh::Pending {
                active,
                mut replies,
                ..
            }) => {
                replies.extend(asked);
                (active, replies, None)
            }
            Some(Refresh::Starting { .. }) | None => {
                (latest.id, asked, Some(Effect::Refresh(latest.clone())))
            }
        };
        self.refreshes.insert(
            instance,
            Refresh::Pending {
                active,
                latest,
                replies,
            },
        );
        run.into_iter().collect()
    }

    const fn answer(reply: R) -> Effect<S, R> {
        Effect::Reply(reply, InstanceResult::Accepted)
    }

    fn answers(replies: Vec<R>) -> Vec<Effect<S, R>> {
        replies.into_iter().map(Self::answer).collect()
    }

    fn credential(&self, instance: &InstanceId) -> Option<Credential> {
        self.roster
            .of(instance)
            .and_then(|known| known.credential.clone())
    }

    fn availability(
        &self,
        instance: &InstanceId,
        credential: Option<&Credential>,
    ) -> Availability {
        match self.ready_with(instance, credential) {
            Ok((_, auth, models)) => Availability::Available { auth, models },
            Err(reason) => Availability::Unavailable { reason },
        }
    }

    /// How `instance`'s agents start and how it signs in, or why it
    /// can't. A stored credential comes first, then the CLI's own sign-in.
    /// A store that doesn't open may hold a credential, so it never falls
    /// back to the sign-in.
    fn ready(
        &self,
        instance: &InstanceId,
    ) -> Result<(S, Auth, Models), String> {
        self.ready_with(
            instance,
            self.roster
                .of(instance)
                .and_then(|settings| settings.credential.as_ref()),
        )
    }

    fn ready_with(
        &self,
        instance: &InstanceId,
        credential: Option<&Credential>,
    ) -> Result<(S, Auth, Models), String> {
        match self.refreshes.get(instance) {
            None => Ok(()),
            Some(
                Refresh::Pending { .. }
                | Refresh::Starting {
                    refresh_requested: true,
                    ..
                },
            ) => Err(String::from(REFRESHING)),
            Some(Refresh::Starting { .. }) => Err(String::from(STARTING)),
        }?;
        match (self.launcher_of(instance)?, &self.unreadable) {
            (Launcher::Unavailable(reason), _)
            | (Launcher::Running { .. }, Some(reason)) => Err(reason.clone()),
            (
                Launcher::Running {
                    start, own, models, ..
                },
                None,
            ) => {
                let offered = models.clone()?;
                let auth = self.signs_in(instance, credential, *own)?;
                Ok((start.clone(), auth, offered))
            }
        }
    }

    /// How `instance` signs in with `credential`, or that it can't.
    fn signs_in(
        &self,
        instance: &InstanceId,
        credential: Option<&Credential>,
        own: OwnSignIn,
    ) -> Result<Auth, String> {
        let info = self.catalog.of(&instance.provider);
        credential.zip(info).map_or_else(
            || match (&instance.account, own) {
                // Only a sign-in names an account without a credential, so
                // a CLI signed in however it is was signed in as it.
                (Account::Named(_), OwnSignIn::Unmanaged) => Ok(Auth::SignedIn),
                (Account::Own | Account::Named(_), _) => own.auth(),
            },
            |(stored, described)| Ok(described.auth(stored)),
        )
    }

    fn launcher_of(
        &self,
        instance: &InstanceId,
    ) -> Result<&Launcher<S>, String> {
        self.launchers
            .get(instance)
            .ok_or_else(|| format!("this host has no instance {instance}"))
    }
}

/// Why a session's workspace could not be made.
#[must_use]
fn unprepared(error: GitError) -> SessionRefusal {
    match error {
        GitError::NotRepository => SessionRefusal::NotGitRepository,
        GitError::Failed(reason) => SessionRefusal::WorkspaceFailure { reason },
    }
}

const fn unavailable(reason: String) -> SessionRefusal {
    SessionRefusal::InstanceUnavailable { reason }
}

const fn refuse<S, R>(reply: R, refusal: InstanceRefusal) -> Effect<S, R> {
    Effect::Reply(reply, InstanceResult::Refused(refusal))
}

const fn not_saved<S, R>(reply: R, reason: String) -> Effect<S, R> {
    refuse(reply, InstanceRefusal::CredentialNotSaved { reason })
}

fn logged<S, R>(note: Note, then: Vec<Effect<S, R>>) -> Vec<Effect<S, R>> {
    core::iter::once(Effect::Log(note)).chain(then).collect()
}

mod start;
pub use start::{
    InHome, Placement, StartDecision, StartEffect, StartEnd, StartOutcome,
    StartReport, StartRequest, StartSession, StartStep, brief_sent, place,
    place_in_home, place_without_home,
};

#[cfg(test)]
mod tests;
