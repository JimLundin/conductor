//! The registry's decisions about sessions and providers.
//!
//! They are whether a `StartSession` or a `Continue` may go ahead and why
//! not, the model and effort a session starts with, which session an id
//! names, whether each provider is available, and how a credential change
//! is stored and the provider refreshed.
//!
//! `daemon` runs it: it owns the store, the sessions' tasks and the
//! providers' agents. It asks the registry, performs each [`Effect`] in
//! order, and reports how each went. The registry holds each provider's
//! starter `S`, each session's handle `H` and each answer's reply `R` as
//! opaque values; it knows no provider by name.
//!
//! A credential change refreshes its provider: the provider's agents take
//! the new credentials and list what they offer with them. Until that ends
//! the provider is unavailable. One refresh per provider runs at a time;
//! changes made meanwhile coalesce into one more, and every change waiting
//! gets the answer of the last.

use alloc::collections::BTreeMap;
use alloc::format;

use agents::Note;
use proto::{
    Auth, Availability, CommandResult, Credential, Credentials, Effort,
    ModelId, Models, Provider, Providers, Rejection, SessionId,
};

use crate::prelude::*;
use crate::session::{Choice, choose};
use crate::workspace::GitError;

/// What a CLI signs in with when the host stores no credential for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnSignIn {
    /// The private home is signed in (`conductord login`).
    SignedIn,
    /// A CLI given on the command line, signed in however it is.
    Unmanaged,
    /// Nothing: the provider is unavailable until it gets a credential.
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
const REFRESHING: &str = "refreshing provider models";

/// What a provider offers and the model a session gets when it names none:
/// the one the credential names, as its gateway serves that one, then
/// `chosen` (`--claude-model`, `--codex-model`), then the CLI's own.
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

/// How the host runs a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launcher<S> {
    /// The CLI runs, `start` starts its agents, and it signs in as `own`
    /// says when the host stores no credential for it.
    Ready {
        start: S,
        own: OwnSignIn,
        offer: Offer,
    },
    /// The host can't run this provider's sessions, for this reason.
    Unavailable(String),
}

/// What the credential store holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stored {
    Opened(Credentials),
    /// The file exists and doesn't open, for this reason. The host runs
    /// no provider until a Clear moves it aside.
    Unreadable(String),
}

impl Stored {
    /// The credentials a provider's CLI starts with: none from a store
    /// that doesn't open.
    #[must_use]
    pub fn credentials(&self) -> Credentials {
        match self {
            Self::Opened(credentials) => credentials.clone(),
            Self::Unreadable(_) => Credentials::none(),
        }
    }
}

/// A provider's refresh: its agents take `credentials`, and, if the
/// provider then `signs_in`, list what they offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job<S> {
    pub id: u64,
    pub provider: Provider,
    pub start: S,
    pub credentials: Credentials,
    pub signs_in: Result<(), String>,
}

/// What a refresh listed: the models, and the one the credentials name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub models: Models,
    pub named: Option<ModelId>,
}

/// How the refresh `id` of `provider` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refreshed {
    pub id: u64,
    pub provider: Provider,
    pub listing: Result<Listing, String>,
}

/// What the shell does next, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect<S, R> {
    Reply(R, CommandResult),
    /// Move the store that doesn't open aside, and report
    /// [`Registry::moved_aside`].
    MoveAside {
        provider: Provider,
        reply: R,
    },
    /// Store `credentials` in place of what the store holds, and report
    /// [`Registry::saved`].
    Save {
        provider: Provider,
        credentials: Credentials,
        reply: R,
    },
    /// Run the refresh, and report [`Registry::refreshed`].
    Refresh(Job<S>),
    Log(Note),
}

/// A session that may start: its provider's starter, the model and effort
/// it runs with, and what the provider offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch<S> {
    pub start: S,
    pub choice: Choice,
    pub offered: Models,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Refresh<S, R> {
    /// The latest job, which runs once the one running has ended unless it
    /// is that one, and the changes waiting for it.
    Pending {
        latest: Job<S>,
        replies: Vec<R>,
    },
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry<S, H, R> {
    launchers: BTreeMap<Provider, Launcher<S>>,
    /// A provider with none is idle.
    refreshes: BTreeMap<Provider, Refresh<S, R>>,
    stored: Stored,
    sessions: BTreeMap<SessionId, (Provider, H)>,
    jobs: u64,
}

impl<S: Clone, H: Clone, R> Registry<S, H, R> {
    /// A registry with no session yet. `stored` is what the store held
    /// when it opened.
    #[must_use]
    pub const fn new(
        launchers: BTreeMap<Provider, Launcher<S>>,
        stored: Stored,
    ) -> Self {
        Self {
            launchers,
            refreshes: BTreeMap::new(),
            stored,
            sessions: BTreeMap::new(),
            jobs: 0,
        }
    }

    /// What `Welcome` tells a client the host can run.
    #[must_use]
    pub fn providers(&self) -> Providers {
        Providers(
            self.launchers
                .keys()
                .map(|&provider| (provider, self.availability(provider)))
                .collect(),
        )
    }

    /// Whether a session may start in a directory that `exists`, on
    /// `provider` with the model and effort asked for.
    ///
    /// # Errors
    ///
    /// Why it may not: no such directory, the provider is unavailable, or
    /// it doesn't offer the model or effort.
    pub fn start(
        &self,
        provider: Provider,
        exists: bool,
        model: Option<ModelId>,
        effort: Option<Effort>,
    ) -> Result<Launch<S>, Rejection> {
        if !exists {
            return Err(Rejection::NoSuchDirectory);
        }
        let (start, _) = self.ready(provider).map_err(unavailable)?;
        let offered = self.models(provider);
        let choice = choose(&offered, model, effort)
            .map_err(|reason| Rejection::NotOffered { reason })?;
        Ok(Launch {
            start,
            choice,
            offered,
        })
    }

    /// Keeps a session that started or was restored.
    pub fn admit(&mut self, session: SessionId, provider: Provider, handle: H) {
        self.sessions.insert(session, (provider, handle));
    }

    /// The session and what its provider offers, for an order that may
    /// start its agent.
    ///
    /// # Errors
    ///
    /// No such session, or its provider is unavailable.
    pub fn relaunch(
        &self,
        session: SessionId,
    ) -> Result<(H, Models), Rejection> {
        let (provider, handle) = self
            .sessions
            .get(&session)
            .ok_or(Rejection::UnknownSession)?;
        self.ready(*provider)
            .map(|_| (handle.clone(), self.models(*provider)))
            .map_err(unavailable)
    }

    #[must_use]
    pub fn find(&self, session: SessionId) -> Option<H> {
        self.sessions
            .get(&session)
            .map(|(_, handle)| handle.clone())
    }

    /// What starts a restored session's agents, whether or not the
    /// provider is available now.
    ///
    /// # Errors
    ///
    /// Why the host can't run the provider at all.
    pub fn launcher(&self, provider: Provider) -> Result<S, String> {
        match self.launcher_of(provider)? {
            Launcher::Ready { start, .. } => Ok(start.clone()),
            Launcher::Unavailable(reason) => Err(reason.clone()),
        }
    }

    /// Stores a provider's credential and refreshes the provider. A store
    /// that doesn't open is left alone, since saving would overwrite the
    /// other providers' credentials.
    pub fn set_credential(
        &mut self,
        credential: Credential,
        reply: R,
    ) -> Vec<Effect<S, R>> {
        let provider = credential.provider();
        match (credential.check(), &self.stored) {
            (Err(reason), _) => Vec::from([Effect::Reply(
                reply,
                CommandResult::Rejected(Rejection::InvalidCredential {
                    reason,
                }),
            )]),
            (Ok(()), Stored::Unreadable(reason)) => {
                Vec::from([not_saved(reply, reason.clone())])
            }
            (Ok(()), Stored::Opened(credentials)) => {
                Vec::from([Effect::Save {
                    provider,
                    credentials: credentials.clone().set(credential),
                    reply,
                }])
            }
        }
    }

    /// Clears a provider's credential and refreshes the provider. A store
    /// that doesn't open is moved aside first, which is how a Clear
    /// recovers it.
    pub fn clear_credential(
        &mut self,
        provider: Provider,
        reply: R,
    ) -> Vec<Effect<S, R>> {
        Vec::from([match &self.stored {
            Stored::Opened(credentials) => Effect::Save {
                provider,
                credentials: credentials.clone().clear(provider),
                reply,
            },
            Stored::Unreadable(_) => Effect::MoveAside { provider, reply },
        }])
    }

    /// How the [`Effect::MoveAside`] went.
    pub fn moved_aside(
        &mut self,
        provider: Provider,
        reply: R,
        moved: Result<(), String>,
    ) -> Vec<Effect<S, R>> {
        match moved {
            Ok(()) => {
                self.stored = Stored::Opened(Credentials::none());
                self.clear_credential(provider, reply)
            }
            Err(reason) => Vec::from([not_saved(reply, reason)]),
        }
    }

    /// How the [`Effect::Save`] went.
    pub fn saved(
        &mut self,
        provider: Provider,
        credentials: Credentials,
        reply: R,
        saved: Result<(), String>,
    ) -> Vec<Effect<S, R>> {
        match saved {
            Ok(()) => {
                self.stored = Stored::Opened(credentials.clone());
                let mut effects = Vec::from([Effect::Log(Note::info(
                    format!("stored a credential change for {provider:?}"),
                ))]);
                effects.extend(self.schedule(provider, credentials, reply));
                effects
            }
            Err(reason) => Vec::from([not_saved(reply, reason)]),
        }
    }

    /// How an [`Effect::Refresh`] went. A result for a job since
    /// superseded is dropped, and the latest job runs.
    pub fn refreshed(&mut self, refreshed: Refreshed) -> Vec<Effect<S, R>> {
        let Refreshed {
            id,
            provider,
            listing,
        } = refreshed;
        match self.refreshes.remove(&provider) {
            Some(Refresh::Pending { latest, replies }) if latest.id == id => {
                let logged = self.listed(provider, listing);
                let result = CommandResult::Providers(self.providers());
                logged
                    .into_iter()
                    .chain(
                        replies
                            .into_iter()
                            .map(|reply| Effect::Reply(reply, result.clone())),
                    )
                    .collect()
            }
            Some(Refresh::Pending { latest, replies }) => {
                let job = latest.clone();
                self.refreshes
                    .insert(provider, Refresh::Pending { latest, replies });
                Vec::from([Effect::Refresh(job)])
            }
            settled => {
                self.refreshes
                    .extend(settled.map(|refresh| (provider, refresh)));
                Vec::new()
            }
        }
    }

    /// Takes what the latest refresh listed, or keeps why it failed.
    fn listed(
        &mut self,
        provider: Provider,
        listing: Result<Listing, String>,
    ) -> Option<Effect<S, R>> {
        match listing {
            Ok(Listing { models, named }) => {
                if let Some(Launcher::Ready { offer, .. }) =
                    self.launchers.get_mut(&provider)
                {
                    offer.listed = models;
                    offer.named = named;
                }
                None
            }
            Err(reason) => {
                let note = Note::warn(format!(
                    "provider refresh failed for {provider:?}: {reason}"
                ));
                self.refreshes.insert(provider, Refresh::Failed(reason));
                Some(Effect::Log(note))
            }
        }
    }

    /// Refreshes `provider` with `credentials`, or, while a refresh runs,
    /// makes them the next. A provider the host can't run answers at once.
    fn schedule(
        &mut self,
        provider: Provider,
        credentials: Credentials,
        reply: R,
    ) -> Vec<Effect<S, R>> {
        let Some(Launcher::Ready { start, own, .. }) =
            self.launchers.get(&provider)
        else {
            return Vec::from([Effect::Reply(
                reply,
                CommandResult::Providers(self.providers()),
            )]);
        };
        self.jobs = self.jobs.saturating_add(1);
        let latest = Job {
            id: self.jobs,
            provider,
            start: start.clone(),
            signs_in: signs_in(&credentials, provider, *own).map(drop),
            credentials,
        };
        let (replies, run) = match self.refreshes.remove(&provider) {
            Some(Refresh::Pending { mut replies, .. }) => {
                replies.push(reply);
                (replies, None)
            }
            Some(Refresh::Failed(_)) | None => {
                (Vec::from([reply]), Some(Effect::Refresh(latest.clone())))
            }
        };
        self.refreshes
            .insert(provider, Refresh::Pending { latest, replies });
        run.into_iter().collect()
    }

    fn availability(&self, provider: Provider) -> Availability {
        match self.ready(provider) {
            Ok((_, auth)) => Availability::Available {
                auth,
                models: self.models(provider),
            },
            Err(reason) => Availability::Unavailable { reason },
        }
    }

    fn models(&self, provider: Provider) -> Models {
        match self.launchers.get(&provider) {
            Some(Launcher::Ready { offer, .. }) => offer.models(),
            Some(Launcher::Unavailable(_)) | None => Models::none(),
        }
    }

    /// How `provider`'s agents start and how it signs in, or why it can't.
    /// A stored credential comes first, then the CLI's own sign-in. A store
    /// that doesn't open may hold a credential, so it never falls back to
    /// the sign-in.
    fn ready(&self, provider: Provider) -> Result<(S, Auth), String> {
        match self.refreshes.get(&provider) {
            None => Ok(()),
            Some(Refresh::Pending { .. }) => Err(String::from(REFRESHING)),
            Some(Refresh::Failed(reason)) => Err(reason.clone()),
        }?;
        match (self.launcher_of(provider)?, &self.stored) {
            (Launcher::Unavailable(reason), _)
            | (Launcher::Ready { .. }, Stored::Unreadable(reason)) => {
                Err(reason.clone())
            }
            (
                Launcher::Ready { start, own, .. },
                Stored::Opened(credentials),
            ) => signs_in(credentials, provider, *own)
                .map(|auth| (start.clone(), auth)),
        }
    }

    fn launcher_of(&self, provider: Provider) -> Result<&Launcher<S>, String> {
        self.launchers
            .get(&provider)
            .ok_or_else(|| format!("this host doesn't run {provider:?}"))
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

/// How `provider` signs in with `credentials`, or that it can't.
fn signs_in(
    credentials: &Credentials,
    provider: Provider,
    own: OwnSignIn,
) -> Result<Auth, String> {
    credentials.auth(provider).map_or_else(|| own.auth(), Ok)
}

const fn unavailable(reason: String) -> Rejection {
    Rejection::ProviderUnavailable { reason }
}

const fn not_saved<S, R>(reply: R, reason: String) -> Effect<S, R> {
    Effect::Reply(
        reply,
        CommandResult::Rejected(Rejection::CredentialNotSaved { reason }),
    )
}

#[cfg(test)]
mod tests;
