//! The registry: the one place a `StartSession` and a `Continue` are
//! checked and a session is found by its id.

use std::collections::BTreeMap;
use std::convert::Infallible;

use agents::claude::Sandbox;
use proto::{
    Auth, Availability, ClaudeCredential, ClientName, CodexCredential, CommandResult, Credential,
    Credentials, Effort, EventKind, HostPath, ModelId, Models, Provider, Providers, Rejection,
    SessionId,
};
use tokio::sync::{mpsc, oneshot};

use crate::Stopped;
use crate::credentials::{Store, Stored};
use crate::log::Log;
use crate::session::{ClaudeLaunch, CodexLaunch, Launch, Session, choose, list_models};

/// How the host runs each provider. Built once in `main`.
#[derive(Debug, Clone)]
pub struct Launchers {
    pub claude: Launcher,
    pub codex: Launcher,
}

#[derive(Debug, Clone)]
pub enum Launcher {
    /// The CLI runs, and signs in as `own` says when the host stores no
    /// credential for it. `models` is what it listed when the host started;
    /// the registry lists them again when the credential changes.
    /// `chosen` (`--claude-model`, `--codex-model`) comes before the CLI's
    /// own default.
    Ready {
        launch: Launch,
        own: OwnSignIn,
        models: Models,
        chosen: Option<ModelId>,
    },
    /// The host can't run this provider's sessions, for this reason.
    Unavailable(String),
}

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
            OwnSignIn::SignedIn => Ok(Auth::SignedIn),
            OwnSignIn::Unmanaged => Ok(Auth::Unmanaged),
            OwnSignIn::Missing => Err(String::from("no credential configured")),
        }
    }
}

impl Launchers {
    fn of(&self, provider: Provider) -> &Launcher {
        match provider {
            Provider::Claude => &self.claude,
            Provider::Codex => &self.codex,
        }
    }

    fn with(self, provider: Provider, launcher: Launcher) -> Launchers {
        match provider {
            Provider::Claude => Launchers {
                claude: launcher,
                ..self
            },
            Provider::Codex => Launchers {
                codex: launcher,
                ..self
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct Registry {
    inbox: mpsc::Sender<RegistryMsg>,
}

enum RegistryMsg {
    Start {
        provider: Provider,
        directory: HostPath,
        model: Option<ModelId>,
        effort: Option<Effort>,
        by: ClientName,
        reply: oneshot::Sender<CommandResult>,
    },
    Continue {
        session: SessionId,
        reply: oneshot::Sender<Result<(Session, Launch, Models), Rejection>>,
    },
    Find {
        session: SessionId,
        reply: oneshot::Sender<Option<Session>>,
    },
    Providers {
        reply: oneshot::Sender<Providers>,
    },
    SetCredential {
        credential: Credential,
        reply: oneshot::Sender<CommandResult>,
    },
    ClearCredential {
        provider: Provider,
        reply: oneshot::Sender<CommandResult>,
    },
}

struct RegistryActor {
    log: Log,
    launchers: Launchers,
    store: Store,
    stored: Stored,
    sessions: BTreeMap<SessionId, (Provider, Session)>,
    last: SessionId,
}

impl Registry {
    /// `stored` is what `store` held when it opened.
    pub fn spawn(log: Log, launchers: Launchers, store: Store, stored: Stored) -> Registry {
        if let Stored::Opened(credentials) = &stored {
            warn_unprotected(credentials);
        }
        let (inbox, messages) = mpsc::channel(16);
        let actor = RegistryActor {
            log,
            launchers,
            store,
            stored,
            sessions: BTreeMap::new(),
            last: SessionId(0),
        };
        tokio::spawn(actor.run(messages));
        Registry { inbox }
    }

    /// Checks the directory and the provider, logs `SessionCreated` and
    /// answers `Started` at once. The agent starts inside the session's own
    /// task, so a slow start never blocks the registry.
    pub async fn start(
        &self,
        provider: Provider,
        directory: HostPath,
        choice: (Option<ModelId>, Option<Effort>),
        by: ClientName,
    ) -> Result<CommandResult, Stopped> {
        let (model, effort) = choice;
        self.ask(|reply| RegistryMsg::Start {
            provider,
            directory,
            model,
            effort,
            by,
            reply,
        })
        .await
    }

    /// The session, how its provider runs now and what it offers, for a
    /// `Continue`. The session starts its agent itself, so a slow start
    /// never blocks the registry.
    pub async fn relaunch(
        &self,
        session: SessionId,
    ) -> Result<Result<(Session, Launch, Models), Rejection>, Stopped> {
        self.ask(|reply| RegistryMsg::Continue { session, reply })
            .await
    }

    pub async fn find(&self, session: SessionId) -> Result<Option<Session>, Stopped> {
        self.ask(|reply| RegistryMsg::Find { session, reply }).await
    }

    /// What `Welcome` tells a client the host can run.
    pub async fn providers(&self) -> Result<Providers, Stopped> {
        self.ask(|reply| RegistryMsg::Providers { reply }).await
    }

    /// Stores the credential and answers with the providers it changes. A
    /// new Codex credential restarts the shared app-server.
    pub async fn set_credential(&self, credential: Credential) -> Result<CommandResult, Stopped> {
        self.ask(|reply| RegistryMsg::SetCredential { credential, reply })
            .await
    }

    pub async fn clear_credential(&self, provider: Provider) -> Result<CommandResult, Stopped> {
        self.ask(|reply| RegistryMsg::ClearCredential { provider, reply })
            .await
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
    async fn run(
        mut self,
        mut messages: mpsc::Receiver<RegistryMsg>,
    ) -> Result<Infallible, Stopped> {
        loop {
            let msg = messages.recv().await.ok_or(Stopped {})?;
            self.on(msg).await?;
        }
    }

    async fn on(&mut self, msg: RegistryMsg) -> Result<(), Stopped> {
        match msg {
            RegistryMsg::Start {
                provider,
                directory,
                model,
                effort,
                by,
                reply,
            } => {
                let result = self.start(provider, directory, (model, effort), by).await?;
                let _ = reply.send(result);
            }
            RegistryMsg::Continue { session, reply } => {
                let relaunch = match self.sessions.get(&session) {
                    None => Err(Rejection::UnknownSession),
                    Some((provider, handle)) => self
                        .ready(provider.to_owned())
                        .map(|(launch, _)| {
                            (handle.clone(), launch, self.models(provider.to_owned()))
                        })
                        .map_err(|reason| Rejection::ProviderUnavailable { reason }),
                };
                let _ = reply.send(relaunch);
            }
            RegistryMsg::Find { session, reply } => {
                let found = self
                    .sessions
                    .get(&session)
                    .map(|(_, handle)| handle.clone());
                let _ = reply.send(found);
            }
            RegistryMsg::Providers { reply } => {
                let _ = reply.send(self.providers());
            }
            RegistryMsg::SetCredential { credential, reply } => {
                let provider = credential.provider();
                let result = match (credential.check(), &self.stored) {
                    (Err(reason), _) => {
                        CommandResult::Rejected(Rejection::InvalidCredential { reason })
                    }
                    // Saving now would overwrite the other provider's.
                    (Ok(()), Stored::Unreadable(reason)) => {
                        CommandResult::Rejected(Rejection::CredentialNotSaved {
                            reason: reason.clone(),
                        })
                    }
                    (Ok(()), Stored::Opened(credentials)) => {
                        let next = credentials.clone().set(credential);
                        self.store(provider, next).await?
                    }
                };
                let _ = reply.send(result);
            }
            RegistryMsg::ClearCredential { provider, reply } => {
                let result = match self.readable() {
                    Ok(credentials) => self.store(provider, credentials.clear(provider)).await?,
                    Err(reason) => {
                        CommandResult::Rejected(Rejection::CredentialNotSaved { reason })
                    }
                };
                let _ = reply.send(result);
            }
        }
        Ok(())
    }

    /// The stored credentials. A store that doesn't open is moved aside
    /// first, which is how a Clear recovers it from the phone.
    fn readable(&mut self) -> Result<Credentials, String> {
        match &self.stored {
            Stored::Opened(credentials) => Ok(credentials.clone()),
            Stored::Unreadable(_) => {
                let aside = self.store.move_aside().map_err(|err| format!("{err:#}"))?;
                tracing::warn!(aside = %aside.display(), "moved the unreadable credential store aside");
                self.stored = Stored::Opened(Credentials::none());
                Ok(Credentials::none())
            }
        }
    }

    /// Saves `next`, which changes `provider`'s credential only, then uses
    /// it. A Codex change restarts the app-server, the only way the key in
    /// its environment changes.
    async fn store(
        &mut self,
        provider: Provider,
        next: Credentials,
    ) -> Result<CommandResult, Stopped> {
        match self.store.save(&next) {
            Err(err) => Ok(CommandResult::Rejected(Rejection::CredentialNotSaved {
                reason: format!("{err:#}"),
            })),
            Ok(()) => {
                tracing::info!(?provider, "stored a credential change");
                warn_unprotected(&next);
                if let (
                    Provider::Codex,
                    Launcher::Ready {
                        launch: Launch::Codex(CodexLaunch { hub }),
                        ..
                    },
                ) = (provider, self.launchers.of(provider))
                {
                    hub.configure(next.codex.clone()).await?;
                }
                self.stored = Stored::Opened(next);
                self.relist(provider).await;
                Ok(CommandResult::Providers(self.providers()))
            }
        }
    }

    /// Lists `provider`'s models again with its new credential, which may
    /// serve others. A CLI that lists none offers only the default.
    async fn relist(&mut self, provider: Provider) {
        if let (
            Ok((with_credential, _)),
            Launcher::Ready {
                launch,
                own,
                chosen,
                ..
            },
        ) = (self.ready(provider), self.launchers.of(provider))
        {
            let ready = Launcher::Ready {
                models: list_models(with_credential, self.store.dir())
                    .await
                    .unwrap_or_else(|reason| {
                        tracing::warn!(%reason, ?provider, "the CLI listed no models");
                        Models::none()
                    }),
                launch: launch.clone(),
                own: own.to_owned(),
                chosen: chosen.clone(),
            };
            self.launchers = self.launchers.clone().with(provider, ready);
        }
    }

    fn providers(&self) -> Providers {
        Providers {
            claude: self.availability(Provider::Claude),
            codex: self.availability(Provider::Codex),
        }
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

    /// What the CLI lists with the current credential. The default is the
    /// model a Codex provider credential names, as its gateway serves that
    /// one, then `chosen`, then the CLI's own.
    fn models(&self, provider: Provider) -> Models {
        let (listed, chosen) = match self.launchers.of(provider) {
            Launcher::Ready { models, chosen, .. } => (models.clone(), chosen.clone()),
            Launcher::Unavailable(_) => (Models::none(), None),
        };
        let credential = match (provider, &self.stored) {
            (
                Provider::Codex,
                Stored::Opened(Credentials {
                    codex: Some(CodexCredential::Provider { model, .. }),
                    ..
                }),
            ) => Some(ModelId(model.clone())),
            (_, _) => None,
        };
        Models {
            default: credential.or(chosen).or(listed.default),
            ..listed
        }
    }

    /// How `provider` runs, with the credential a Claude session gets when
    /// it starts, and how it signs in, or why it can't. A stored credential
    /// comes first, then the CLI's own sign-in. A store that doesn't open
    /// may hold a credential, so it never falls back to the sign-in.
    fn ready(&self, provider: Provider) -> Result<(Launch, Auth), String> {
        match (self.launchers.of(provider), &self.stored) {
            (Launcher::Unavailable(reason), _)
            | (Launcher::Ready { .. }, Stored::Unreadable(reason)) => Err(reason.clone()),
            (Launcher::Ready { launch, own, .. }, Stored::Opened(credentials)) => {
                let launch = match launch.clone() {
                    Launch::Claude(claude) => Launch::Claude(ClaudeLaunch {
                        credential: credentials.claude.clone(),
                        ..claude
                    }),
                    Launch::Codex(codex) => Launch::Codex(codex),
                };
                match credentials.auth(provider) {
                    Some(auth) => Ok((launch, auth)),
                    None => own.auth().map(|auth| (launch, auth)),
                }
            }
        }
    }

    async fn start(
        &mut self,
        provider: Provider,
        directory: HostPath,
        asked: (Option<ModelId>, Option<Effort>),
        by: ClientName,
    ) -> Result<CommandResult, Stopped> {
        // `read_dir` fails for a missing path and for a file alike.
        let ready = self.ready(provider).map(|(launch, _)| launch);
        let (model, effort) = asked;
        match (
            std::fs::read_dir(&directory.0),
            ready,
            choose(&self.models(provider), model, effort),
        ) {
            (Err(_), _, _) => Ok(CommandResult::Rejected(Rejection::NoSuchDirectory)),
            (Ok(_), Err(reason), _) => {
                Ok(CommandResult::Rejected(Rejection::ProviderUnavailable {
                    reason,
                }))
            }
            (Ok(_), Ok(_), Err(reason)) => {
                Ok(CommandResult::Rejected(Rejection::NotOffered { reason }))
            }
            (Ok(_), Ok(launch), Ok(choice)) => {
                let id = self.last.next();
                self.last = id;
                let created = EventKind::SessionCreated {
                    provider,
                    directory: directory.clone(),
                    by,
                    model: choice.model.clone(),
                    effort: choice.effort,
                };
                self.log.append(id, created).await?;
                let offered = self.models(provider);
                let session =
                    Session::spawn(id, directory, choice, (launch, offered), self.log.clone());
                self.sessions.insert(id, (provider, session));
                Ok(CommandResult::Started { session: id })
            }
        }
    }
}

/// Without Claude's sandbox the scrub is off (`agents::claude::sandbox`),
/// so the commands Claude runs inherit a gateway token.
fn warn_unprotected(credentials: &Credentials) {
    if let (Some(ClaudeCredential::Gateway { .. }), Sandbox::Missing) =
        (&credentials.claude, agents::claude::sandbox())
    {
        tracing::warn!(
            "Claude's commands can read the gateway token; install bubblewrap and socat to sandbox them"
        );
    }
}
