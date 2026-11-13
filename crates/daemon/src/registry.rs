//! The registry: the one place a `StartSession` and a `Continue` are
//! checked and a session is found by its id.

use std::collections::BTreeMap;
use std::convert::Infallible;

use agents::claude::Sandbox;
use proto::{
    Auth, Availability, ClaudeCredential, ClientName, CloseReason, CodexCredential, CommandResult,
    Credential, Credentials, Effort, Event, EventKind, HostPath, ModelId, Models, Provider,
    Providers, Rejection, SessionId, TurnOutcome,
};
use tokio::sync::{mpsc, oneshot};

use crate::Stopped;
use crate::credentials::{Store, Stored};
use crate::log::Log;
use crate::session::{Choice, ClaudeLaunch, CodexLaunch, Launch, Session, choose, list_models};

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
    Restore,
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

enum RecoverTurn {
    Running,
    Idle,
}
enum RecoverAgent {
    Running,
    Closed,
}

struct RegistryActor {
    log: Log,
    launchers: Launchers,
    store: Store,
    stored: Stored,
    sessions: BTreeMap<SessionId, (Provider, Session)>,
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
        };
        let _ = inbox.try_send(RegistryMsg::Restore);
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
            RegistryMsg::Restore => self.restore().await?,
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

    async fn restore(&mut self) -> Result<(), Stopped> {
        let history = self.log.history().await?;
        let created = history
            .iter()
            .filter(|event| matches!(event.kind, EventKind::SessionCreated { .. }))
            .cloned()
            .collect::<Vec<Event>>();
        for event in created {
            self.restore_session(event, &history).await?;
        }
        Ok(())
    }

    async fn restore_session(&mut self, event: Event, history: &[Event]) -> Result<(), Stopped> {
        if let EventKind::SessionCreated {
            provider,
            directory,
            model,
            effort,
            ..
        } = event.kind
        {
            let id = event.session;
            let events = history
                .iter()
                .filter(|event| event.session == id)
                .cloned()
                .collect::<Vec<Event>>();
            let choice = events
                .iter()
                .fold(Choice { model, effort }, |choice, event| {
                    match &event.kind {
                        EventKind::ModelChanged { model, effort, .. } => Choice {
                            model: model.clone(),
                            effort: effort.to_owned(),
                        },
                        _ => choice,
                    }
                });
            let conversation = match events
                .iter()
                .find(|event| matches!(event.kind, EventKind::UserMessage { .. }))
            {
                Some(_) => self.log.saved_conversation(id).await?,
                None => None,
            };
            let session = Session::restore(
                id,
                directory,
                choice,
                conversation,
                &events,
                self.log.clone(),
            );
            self.sessions.insert(id, (provider, session));
            let open = events
                .iter()
                .filter_map(|event| match &event.kind {
                    EventKind::RequestOpened { request, .. }
                    | EventKind::QuestionAsked { request, .. } => Some(request.to_owned()),
                    _ => None,
                })
                .filter(|request| {
                    !events
                        .iter()
                        .filter_map(|event| match &event.kind {
                            EventKind::RequestResolved { request, .. } => Some(request),
                            _ => None,
                        })
                        .any(|resolved| resolved == request)
                })
                .collect::<Vec<proto::RequestId>>();
            for request in open {
                self.log
                    .append(
                        id,
                        EventKind::RequestResolved {
                            request,
                            resolution: proto::Resolution::Withdrawn,
                        },
                    )
                    .await?;
            }
            let running = events.iter().rev().find_map(|event| match &event.kind {
                EventKind::UserMessage { .. } => Some(RecoverTurn::Running),
                EventKind::TurnCompleted { .. } | EventKind::SessionClosed { .. } => {
                    Some(RecoverTurn::Idle)
                }
                _ => None,
            });
            if let Some(RecoverTurn::Running) = running {
                self.log
                    .append(
                        id,
                        EventKind::TurnCompleted {
                            outcome: TurnOutcome::Interrupted,
                        },
                    )
                    .await?;
            }
            let closed = events.iter().rev().find_map(|event| match &event.kind {
                EventKind::SessionClosed { .. } => Some(RecoverAgent::Closed),
                EventKind::SessionCreated { .. } | EventKind::SessionReady => {
                    Some(RecoverAgent::Running)
                }
                _ => None,
            });
            if let Some(RecoverAgent::Running) = closed {
                self.log
                    .append(
                        id,
                        EventKind::SessionClosed {
                            reason: CloseReason::Stopped {
                                reason: String::from("the host restarted"),
                            },
                        },
                    )
                    .await?;
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
                let id = SessionId(u128::from(ulid::Ulid::new()));
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use agents::Home;
    use proto::{
        ClientName, CloseReason, CommandResult, EventKind, HostPath, HostSeq, Models, Provider,
        RequestId, Resolution, SessionId, ToolCallId, TurnOutcome,
    };

    use super::{Launcher, Launchers, OwnSignIn, Registry};
    use crate::credentials::Store;
    use crate::log::Log;
    use crate::session::{ClaudeLaunch, Launch, Order};

    fn launcher() -> Launcher {
        Launcher::Ready {
            launch: Launch::Claude(ClaudeLaunch {
                claude_path: Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../agents/tests/fake-claude.sh"),
                home: Home::User,
                credential: None,
            }),
            own: OwnSignIn::Unmanaged,
            models: Models::none(),
            chosen: None,
        }
    }

    fn directory() -> PathBuf {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("conductor-test-restart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/e2e/claude-resumed.jsonl"),
            directory.join("stdout.jsonl"),
        )
        .unwrap();
        directory
    }

    async fn seed(log: &Log, id: SessionId, kinds: Vec<EventKind>) -> Result<(), crate::Stopped> {
        for kind in kinds {
            log.append(id, kind).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn restart_replays_the_cursor_withdraws_requests_and_continue_resumes() {
        let directory = directory();
        let path = directory.join("conductor.redb");
        let id = SessionId(u128::from(ulid::Ulid::new()));
        let first = Log::open(String::from("test"), &path).unwrap();
        seed(
            &first,
            id,
            Vec::from([
                EventKind::SessionCreated {
                    provider: Provider::Claude,
                    directory: HostPath(directory.display().to_string()),
                    by: ClientName(String::from("phone")),
                    model: None,
                    effort: None,
                },
                EventKind::SessionReady,
                EventKind::UserMessage {
                    text: String::from("before restart"),
                    by: ClientName(String::from("phone")),
                },
                EventKind::RequestOpened {
                    request: RequestId(8),
                    call: ToolCallId(String::from("tool")),
                    tool: proto::ToolKind::Other {
                        name: String::from("test"),
                        detail: String::new(),
                    },
                    reason: None,
                    for_session: None,
                },
            ]),
        )
        .await
        .unwrap();
        first
            .conversation(id, String::from("restored-native-conversation"))
            .await
            .unwrap();
        let epoch = first
            .welcome(proto::Providers {
                claude: proto::Availability::Unavailable {
                    reason: String::from("test"),
                },
                codex: proto::Availability::Unavailable {
                    reason: String::from("test"),
                },
            })
            .await
            .unwrap()
            .epoch;
        first.shutdown().await.unwrap();
        let second = Log::open(String::from("test"), &path).unwrap();
        let (store, stored) = Store::open(&directory).unwrap();
        let registry = Registry::spawn(
            second.clone(),
            Launchers {
                claude: launcher(),
                codex: Launcher::Unavailable(String::from("test")),
            },
            store,
            stored,
        );
        let providers = registry.providers().await.unwrap();
        assert_eq!(second.welcome(providers).await.unwrap().epoch, epoch);
        let replay = second.subscribe(HostSeq(4)).await.unwrap().replay;
        assert_eq!(
            replay
                .iter()
                .map(|event| (event.seq, event.kind.clone()))
                .collect::<Vec<_>>(),
            [
                (
                    HostSeq(5),
                    EventKind::RequestResolved {
                        request: RequestId(8),
                        resolution: Resolution::Withdrawn
                    }
                ),
                (
                    HostSeq(6),
                    EventKind::TurnCompleted {
                        outcome: TurnOutcome::Interrupted
                    }
                ),
                (
                    HostSeq(7),
                    EventKind::SessionClosed {
                        reason: CloseReason::Stopped {
                            reason: String::from("the host restarted")
                        }
                    }
                ),
            ]
        );
        let (session, launch, models) = registry.relaunch(id).await.unwrap().unwrap();
        assert_eq!(
            session.order(Order::Start(launch, models)).await.unwrap(),
            CommandResult::Accepted
        );
        let arguments = std::fs::read_to_string(directory.join("argv.txt")).unwrap();
        assert_eq!(
            arguments
                .lines()
                .find(|argument| argument.starts_with("--resume=")),
            Some("--resume=restored-native-conversation")
        );
        let ready = second.subscribe(HostSeq(7)).await.unwrap().replay;
        assert_eq!(
            ready.iter().map(|event| &event.kind).collect::<Vec<_>>(),
            [&EventKind::SessionReady]
        );
        second.shutdown().await.unwrap();
        drop(session);
        drop(registry);
    }
}
