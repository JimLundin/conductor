//! The registry's decisions about sessions and instances.
//!
//! They are whether a `StartSession` or a `Continue` may go ahead and why
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
//! order, and reports how each went. The registry holds each instance's
//! starter `S`, each session's handle `H` and each answer's reply `R` as
//! opaque values; it knows no provider by name.
//!
//! A credential change refreshes its instance: the instance's agents take
//! the new credential and list what they offer with it. Until that ends
//! the instance is unavailable. One refresh per instance runs at a time;
//! changes made meanwhile coalesce into one more, and every change waiting
//! gets the answer of the last. An added instance is unavailable until its
//! CLI has started.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;

use proto::{
    Account, Auth, Availability, Catalog, CommandResult, Credential, Effort,
    Event, EventKind, Instance, InstanceId, InstanceStatus, Instances, ModelId,
    Models, Provider, Rejection, Roster, SessionId,
};
use sha2::{Digest, Sha256};

use crate::Note;
use crate::prelude::*;
use crate::session::{Choice, choose};
use crate::workspace::GitError;

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

/// What an instance offers, and the model a session gets when it names
/// none.
///
/// That is the one the credential names, as its gateway serves that one,
/// then `chosen` (the instance's, else `--model <provider>=<model>`), then
/// the CLI's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// What the CLI lists with the current credential.
    pub listed: Models,
    /// The model the current credential names.
    pub named: Option<ModelId>,
    pub chosen: Option<ModelId>,
}

impl Offer {
    #[must_use]
    pub fn models(&self) -> Models {
        Models {
            default: self
                .named
                .clone()
                .or_else(|| self.chosen.clone())
                .or_else(|| self.listed.default.clone()),
            choices: self.listed.choices.clone(),
        }
    }
}

/// How the host runs an instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launcher<S> {
    /// The CLI runs, `start` starts its agents, and it signs in as `own`
    /// says when the host stores no credential for it.
    Ready {
        start: S,
        own: OwnSignIn,
        offer: Offer,
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
            .map(|&provider| (InstanceId::own(provider), Instance::default()))
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
    provider: Provider,
    account: Option<&str>,
) -> Result<InstanceId, String> {
    let id = account.map_or_else(
        || InstanceId::own(provider),
        |named| InstanceId::named(provider, named),
    );
    if roster.of(&id).is_some() {
        return Ok(id);
    }
    let known = roster
        .0
        .keys()
        .filter(|known| known.provider == provider)
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

/// What a sign-in of `from` that the CLI says is `account`'s makes of the
/// roster.
///
/// That is `None` when it is `from`'s own account or the CLI named none,
/// and otherwise the roster with that account's instance, signed in on
/// its own, in place of `from`, and the instance. An own instance stays,
/// fresh, for the next sign-in.
#[must_use]
pub fn signed_in_as(
    roster: &Roster,
    from: &InstanceId,
    account: Option<Account>,
) -> Option<(Roster, InstanceId)> {
    let named = account.filter(|named| named != &from.account)?;
    let settings = roster.of(from)?;
    let to = InstanceId {
        provider: from.provider,
        account: named,
    };
    let mut next = roster.clone();
    if from.account != Account::Own {
        next.0.remove(from);
    }
    next.0.insert(
        to.clone(),
        Instance {
            credential: None,
            ..settings.clone()
        },
    );
    Some((next, to))
}

/// The sessions whose agents run, or start, after `history`: created or
/// ready, and not closed since.
#[must_use]
pub fn running(history: &[Event]) -> BTreeSet<SessionId> {
    let mut running = BTreeSet::new();
    for event in history {
        match event.kind {
            EventKind::SessionCreated { .. } | EventKind::SessionReady => {
                running.insert(event.session);
            }
            EventKind::SessionClosed { .. } => {
                running.remove(&event.session);
            }
            _ => {}
        }
    }
    running
}

/// An instance's refresh: its agents take `credential`, and, if the
/// instance then `signs_in`, list what they offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job<S> {
    pub id: u64,
    pub instance: InstanceId,
    pub start: S,
    pub credential: Option<Credential>,
    pub signs_in: Result<(), String>,
}

/// What a refresh listed: the models, and the one the credential names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub models: Models,
    pub named: Option<ModelId>,
}

/// How the refresh `id` of `instance` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refreshed {
    pub id: u64,
    pub instance: InstanceId,
    pub listing: Result<Listing, String>,
}

/// An added instance to start: its provider's CLI, in its own home, as
/// `settings` say, with their credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Starting {
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
    /// A sign-in of `from` named `to`'s account: the shell moves `from`'s
    /// home and work dir to `to`'s before it saves.
    SignedIn {
        from: InstanceId,
        to: InstanceId,
    },
    Removed(InstanceId),
}

/// What the shell does next, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect<S, R> {
    Reply(R, CommandResult),
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
pub struct Launch<S> {
    pub start: S,
    pub choice: Choice,
    pub offered: Models,
    pub provider: Provider,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Refresh<S, R> {
    /// The latest job, which runs once the one running has ended unless it
    /// is that one, and the changes waiting for it.
    Pending {
        latest: Job<S>,
        replies: Vec<R>,
    },
    /// An added instance's CLI is starting with `credential`, and these
    /// changes wait for it.
    Starting {
        id: u64,
        credential: Option<Credential>,
        replies: Vec<R>,
    },
    Failed(String),
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
    /// when it opened, and `launchers` run the instances
    /// [`Stored::roster`] gives.
    #[must_use]
    pub fn new(
        launchers: BTreeMap<InstanceId, Launcher<S>>,
        stored: &Stored,
        catalog: Catalog,
    ) -> Self {
        let unreadable = match stored {
            Stored::Unreadable(reason) => Some(reason.clone()),
            Stored::Opened(_) | Stored::Fresh => None,
        };
        Self {
            roster: stored.roster(&catalog),
            unreadable,
            launchers,
            catalog,
            refreshes: BTreeMap::new(),
            sessions: BTreeMap::new(),
            jobs: 0,
        }
    }

    /// What `Welcome` tells a client of each provider that never changes.
    #[must_use]
    pub const fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// What `Welcome` tells a client the host can run.
    #[must_use]
    pub fn instances(&self) -> Instances {
        Instances(
            self.roster
                .0
                .keys()
                .map(|id| {
                    (
                        id.clone(),
                        InstanceStatus {
                            availability: self.availability(id),
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
    pub fn start(
        &self,
        instance: &InstanceId,
        exists: bool,
        model: Option<ModelId>,
        effort: Option<Effort>,
    ) -> Result<Launch<S>, Rejection> {
        if !exists {
            return Err(Rejection::NoSuchDirectory);
        }
        if self.roster.of(instance).is_none() {
            return Err(Rejection::UnknownInstance);
        }
        let provider = instance.provider;
        let (start, _) = self.ready(instance).map_err(unavailable)?;
        let offered = self.models(instance);
        let choice = choose(&offered, model, effort)
            .map_err(|reason| Rejection::NotOffered { reason })?;
        Ok(Launch {
            start,
            choice,
            offered,
            provider,
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
    ) -> Result<(H, Models), Rejection> {
        let (instance, handle) = self
            .sessions
            .get(&session)
            .ok_or(Rejection::UnknownSession)?;
        self.ready(instance)
            .map(|_| (handle.clone(), self.models(instance)))
            .map_err(unavailable)
    }

    #[must_use]
    pub fn find(&self, session: SessionId) -> Option<H> {
        self.sessions
            .get(&session)
            .map(|(_, handle)| handle.clone())
    }

    /// What starts a restored session's agents, whether or not the
    /// instance is available now.
    ///
    /// # Errors
    ///
    /// Why the host can't run the instance at all.
    pub fn launcher(&self, instance: &InstanceId) -> Result<S, String> {
        match self.launcher_of(instance)? {
            Launcher::Ready { start, .. } => Ok(start.clone()),
            Launcher::Unavailable(reason) => Err(reason.clone()),
        }
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
        reply: R,
    ) -> Vec<Effect<S, R>> {
        if let Some((roster, to)) =
            signed_in_as(&self.roster, &instance, account)
        {
            let change = Change::SignedIn { from: instance, to };
            return self.save(roster, change, reply);
        }
        if let Some(Launcher::Ready { own, .. }) =
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
            .of(provider)
            .ok_or_else(|| format!("this host doesn't run {}", provider.name()))
            .and_then(|info| info.account(&credential));
        let instance = match named {
            Ok(account) => InstanceId { provider, account },
            Err(reason) => {
                return Vec::from([rejected(
                    reply,
                    Rejection::InvalidCredential { reason },
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
            return Vec::from([rejected(reply, Rejection::UnknownInstance)]);
        }
        if self.unreadable.is_some() {
            return Vec::from([Effect::MoveAside { instance, reply }]);
        }
        if instance.account == Account::Own {
            return Vec::from([self.answer(reply)]);
        }
        let busy = self
            .sessions
            .iter()
            .filter(|(id, (on, _))| on == &instance && running.contains(id))
            .count();
        if busy > 0 {
            return Vec::from([rejected(
                reply,
                Rejection::InstanceInUse {
                    sessions: u64::try_from(busy).unwrap_or(u64::MAX),
                },
            )]);
        }
        let mut roster = self.roster.clone();
        roster.0.remove(&instance);
        self.save(roster, Change::Removed(instance), reply)
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
            Change::SignedIn { from, to } => {
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
                    effects.extend(self.answers(left));
                }
                effects
            }
            Change::Removed(instance) => {
                let mut waiting = self.forget(&instance);
                waiting.push(reply);
                logged(
                    Note::InstanceRemoved { instance },
                    self.answers(waiting),
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
            listing,
        } = refreshed;
        match self.refreshes.remove(&instance) {
            Some(Refresh::Pending { latest, replies }) if latest.id == id => {
                let logged = self.listed(&instance, listing);
                logged.into_iter().chain(self.answers(replies)).collect()
            }
            Some(Refresh::Pending { latest, replies }) => {
                let job = latest.clone();
                self.refreshes
                    .insert(instance, Refresh::Pending { latest, replies });
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
                credential,
                replies,
            }) if current == id => {
                self.launchers.insert(instance.clone(), launcher);
                if credential == self.credential(&instance) {
                    self.answers(replies)
                } else {
                    self.schedule(instance, replies)
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
            Some(Refresh::Failed(_)) | None => Vec::new(),
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
            return self.answers(replies);
        };
        self.jobs = self.jobs.saturating_add(1);
        self.refreshes.insert(
            instance.clone(),
            Refresh::Starting {
                id: self.jobs,
                credential: settings.credential.clone(),
                replies,
            },
        );
        Vec::from([Effect::Start(Starting {
            id: self.jobs,
            instance,
            settings,
        })])
    }

    /// Takes what the latest refresh listed, or keeps why it failed.
    fn listed(
        &mut self,
        instance: &InstanceId,
        listing: Result<Listing, String>,
    ) -> Option<Effect<S, R>> {
        match listing {
            Ok(Listing { models, named }) => {
                if let Some(Launcher::Ready { offer, .. }) =
                    self.launchers.get_mut(instance)
                {
                    offer.listed = models;
                    offer.named = named;
                }
                None
            }
            Err(reason) => {
                let note = Note::RefreshFailed {
                    instance: instance.clone(),
                    reason: reason.clone(),
                };
                self.refreshes
                    .insert(instance.clone(), Refresh::Failed(reason));
                Some(Effect::Log(note))
            }
        }
    }

    /// Refreshes `instance` with its credential, or, while a refresh
    /// runs, makes it the next. An instance whose CLI is starting takes
    /// it once started, and one the host can't run answers at once.
    fn schedule(
        &mut self,
        instance: InstanceId,
        asked: Vec<R>,
    ) -> Vec<Effect<S, R>> {
        if let Some(Refresh::Starting { replies, .. }) =
            self.refreshes.get_mut(&instance)
        {
            replies.extend(asked);
            return Vec::new();
        }
        let Some(Launcher::Ready { start, own, .. }) =
            self.launchers.get(&instance)
        else {
            return self.answers(asked);
        };
        let credential = self.credential(&instance);
        self.jobs = self.jobs.saturating_add(1);
        let latest = Job {
            id: self.jobs,
            instance: instance.clone(),
            start: start.clone(),
            signs_in: self
                .signs_in(&instance, credential.as_ref(), *own)
                .map(drop),
            credential,
        };
        let (replies, run) = match self.refreshes.remove(&instance) {
            Some(Refresh::Pending { mut replies, .. }) => {
                replies.extend(asked);
                (replies, None)
            }
            Some(Refresh::Failed(_) | Refresh::Starting { .. }) | None => {
                (asked, Some(Effect::Refresh(latest.clone())))
            }
        };
        self.refreshes
            .insert(instance, Refresh::Pending { latest, replies });
        run.into_iter().collect()
    }

    fn answer(&self, reply: R) -> Effect<S, R> {
        Effect::Reply(reply, CommandResult::Instances(self.instances()))
    }

    /// Each reply gets what the host runs now.
    fn answers(&self, replies: Vec<R>) -> Vec<Effect<S, R>> {
        let result = CommandResult::Instances(self.instances());
        replies
            .into_iter()
            .map(|reply| Effect::Reply(reply, result.clone()))
            .collect()
    }

    fn credential(&self, instance: &InstanceId) -> Option<Credential> {
        self.roster
            .of(instance)
            .and_then(|known| known.credential.clone())
    }

    fn availability(&self, instance: &InstanceId) -> Availability {
        match self.ready(instance) {
            Ok((_, auth)) => Availability::Available {
                auth,
                models: self.models(instance),
            },
            Err(reason) => Availability::Unavailable { reason },
        }
    }

    fn models(&self, instance: &InstanceId) -> Models {
        match self.launchers.get(instance) {
            Some(Launcher::Ready { offer, .. }) => offer.models(),
            Some(Launcher::Unavailable(_)) | None => Models::none(),
        }
    }

    /// How `instance`'s agents start and how it signs in, or why it
    /// can't. A stored credential comes first, then the CLI's own sign-in.
    /// A store that doesn't open may hold a credential, so it never falls
    /// back to the sign-in.
    fn ready(&self, instance: &InstanceId) -> Result<(S, Auth), String> {
        match self.refreshes.get(instance) {
            None => Ok(()),
            Some(Refresh::Pending { .. }) => Err(String::from(REFRESHING)),
            Some(Refresh::Starting { .. }) => Err(String::from(STARTING)),
            Some(Refresh::Failed(reason)) => Err(reason.clone()),
        }?;
        match (self.launcher_of(instance)?, &self.unreadable) {
            (Launcher::Unavailable(reason), _)
            | (Launcher::Ready { .. }, Some(reason)) => Err(reason.clone()),
            (Launcher::Ready { start, own, .. }, None) => self
                .signs_in(instance, self.credential(instance).as_ref(), *own)
                .map(|auth| (start.clone(), auth)),
        }
    }

    /// How `instance` signs in with `credential`, or that it can't.
    fn signs_in(
        &self,
        instance: &InstanceId,
        credential: Option<&Credential>,
        own: OwnSignIn,
    ) -> Result<Auth, String> {
        let info = self
            .roster
            .of(instance)
            .and_then(|_| self.catalog.of(instance.provider));
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
pub fn unprepared(error: GitError) -> Rejection {
    match error {
        GitError::NotRepository => Rejection::NotGitRepository,
        GitError::Failed(reason) => Rejection::WorkspaceFailure { reason },
    }
}

const fn unavailable(reason: String) -> Rejection {
    Rejection::InstanceUnavailable { reason }
}

const fn rejected<S, R>(reply: R, rejection: Rejection) -> Effect<S, R> {
    Effect::Reply(reply, CommandResult::Rejected(rejection))
}

const fn not_saved<S, R>(reply: R, reason: String) -> Effect<S, R> {
    rejected(reply, Rejection::CredentialNotSaved { reason })
}

fn logged<S, R>(note: Note, then: Vec<Effect<S, R>>) -> Vec<Effect<S, R>> {
    core::iter::once(Effect::Log(note)).chain(then).collect()
}

#[cfg(test)]
mod tests;
