//! The registry: the one place a `StartSession` and a `Continue` are
//! checked and a session is found by its id.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::PathBuf;

use claude::Sandbox;
use n0_future::MaybeFuture;
use n0_future::boxed::BoxFuture;
use proto::{
    Auth, Availability, ClaudeCredential, ClientName, CloseReason,
    CodexCredential, CommandResult, Credential, Credentials, Effort, Event,
    EventKind, HostPath, ModelId, Models, Provider, Providers, Rejection,
    SessionId, TurnOutcome,
};
use tokio::sync::{mpsc, oneshot};

use crate::Stopped;
use crate::credentials::{Store, Stored};
use crate::log::Log;
use crate::session::{
    Choice, ClaudeLaunch, CodexLaunch, Launch, Session, choose, list_models,
};

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
            Self::SignedIn => Ok(Auth::SignedIn),
            Self::Unmanaged => Ok(Auth::Unmanaged),
            Self::Missing => Err(String::from("no credential configured")),
        }
    }
}

impl Launchers {
    const fn of(&self, provider: Provider) -> &Launcher {
        match provider {
            Provider::Claude => &self.claude,
            Provider::Codex => &self.codex,
        }
    }

    fn with(self, provider: Provider, launcher: Launcher) -> Self {
        match provider {
            Provider::Claude => Self {
                claude: launcher,
                ..self
            },
            Provider::Codex => Self {
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
    Refreshed(Refreshed),
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
    claude_refresh: ProviderRefresh,
    codex_refresh: ProviderRefresh,
}

#[derive(Clone)]
struct RefreshJob {
    id: ulid::Ulid,
    provider: Provider,
    launcher: Launcher,
    credentials: Credentials,
    directory: PathBuf,
}

struct Refreshed {
    id: ulid::Ulid,
    provider: Provider,
    models: Result<Models, String>,
}

enum RefreshState {
    Idle,
    Pending {
        latest: Box<RefreshJob>,
        replies: Vec<oneshot::Sender<CommandResult>>,
    },
    Failed(String),
}

struct ProviderRefresh {
    state: RefreshState,
    future: MaybeFuture<BoxFuture<Refreshed>>,
}

struct RefreshCompletion {
    models: Result<Models, String>,
    replies: Vec<oneshot::Sender<CommandResult>>,
}

impl ProviderRefresh {
    fn new() -> Self {
        Self {
            state: RefreshState::Idle,
            future: MaybeFuture::None,
        }
    }

    fn schedule(
        &mut self,
        latest: RefreshJob,
        reply: oneshot::Sender<CommandResult>,
    ) {
        let replies =
            match std::mem::replace(&mut self.state, RefreshState::Idle) {
                RefreshState::Pending { mut replies, .. } => {
                    replies.push(reply);
                    replies
                }
                RefreshState::Idle | RefreshState::Failed(_) => {
                    self.future =
                        MaybeFuture::Some(Box::pin(refresh(latest.clone())));
                    Vec::from([reply])
                }
            };
        self.state = RefreshState::Pending {
            latest: Box::new(latest),
            replies,
        };
    }

    fn finish(&mut self, result: Refreshed) -> Option<RefreshCompletion> {
        match std::mem::replace(&mut self.state, RefreshState::Idle) {
            RefreshState::Pending { latest, replies }
                if latest.id == result.id =>
            {
                self.state = match &result.models {
                    Ok(_) => RefreshState::Idle,
                    Err(reason) => RefreshState::Failed(reason.clone()),
                };
                Some(RefreshCompletion {
                    models: result.models,
                    replies,
                })
            }
            RefreshState::Pending { latest, replies } => {
                self.future = MaybeFuture::Some(Box::pin(refresh(
                    latest.as_ref().clone(),
                )));
                self.state = RefreshState::Pending { latest, replies };
                None
            }
            state @ (RefreshState::Idle | RefreshState::Failed(_)) => {
                self.state = state;
                None
            }
        }
    }

    fn ready(&self) -> Result<(), String> {
        match &self.state {
            RefreshState::Idle => Ok(()),
            RefreshState::Pending { .. } => {
                Err(String::from("refreshing provider models"))
            }
            RefreshState::Failed(reason) => Err(reason.clone()),
        }
    }
}

async fn refresh(job: RefreshJob) -> Refreshed {
    Refreshed {
        id: job.id,
        provider: job.provider,
        models: refresh_models(job).await,
    }
}

async fn refresh_models(job: RefreshJob) -> Result<Models, String> {
    if let Launcher::Ready {
        launch: Launch::Codex(CodexLaunch { hub }),
        ..
    } = &job.launcher
    {
        hub.configure(job.credentials.codex.clone())
            .await
            .map_err(|err| err.to_string())?;
    }
    let (launch, _) = configured(
        &job.launcher,
        job.provider,
        &Stored::Opened(job.credentials),
    )?;
    list_models(launch, &job.directory).await
}

impl Registry {
    /// `stored` is what `store` held when it opened.
    #[must_use]
    pub fn spawn(
        log: Log,
        launchers: Launchers,
        store: Store,
        stored: Stored,
    ) -> Self {
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
            claude_refresh: ProviderRefresh::new(),
            codex_refresh: ProviderRefresh::new(),
        };
        let _ = inbox.try_send(RegistryMsg::Restore);
        tokio::spawn(actor.run(messages));
        Self { inbox }
    }

    /// Checks the directory and the provider, logs `SessionCreated` and
    /// answers `Started` at once. The agent starts inside the session's own
    /// task, so a slow start never blocks the registry.
    ///
    /// # Errors
    ///
    /// Fails if the registry has stopped.
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

    /// The session, how its provider runs now and what it offers, for an
    /// order that may start the agent or is checked against what the
    /// provider offers: `Continue`, `UndoTurn` and `SetEffort`. The
    /// session starts its agent itself, so a slow start never blocks the
    /// registry.
    ///
    /// # Errors
    ///
    /// Fails if the registry has stopped.
    pub async fn relaunch(
        &self,
        session: SessionId,
    ) -> Result<Result<(Session, Launch, Models), Rejection>, Stopped> {
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
    pub async fn providers(&self) -> Result<Providers, Stopped> {
        self.ask(|reply| RegistryMsg::Providers { reply }).await
    }

    /// Stores the credential and answers with the providers it changes. A
    /// new Codex credential restarts the shared app-server.
    ///
    /// # Errors
    ///
    /// Fails if the registry has stopped.
    pub async fn set_credential(
        &self,
        credential: Credential,
    ) -> Result<CommandResult, Stopped> {
        self.ask(|reply| RegistryMsg::SetCredential { credential, reply })
            .await
    }

    /// Clears the provider's stored credential and answers with the
    /// providers it changes.
    ///
    /// # Errors
    ///
    /// Fails if the registry has stopped.
    pub async fn clear_credential(
        &self,
        provider: Provider,
    ) -> Result<CommandResult, Stopped> {
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
            let msg = tokio::select! {
                msg = messages.recv() => msg.ok_or(Stopped {})?,
                result = &mut self.claude_refresh.future => RegistryMsg::Refreshed(result),
                result = &mut self.codex_refresh.future => RegistryMsg::Refreshed(result),
            };
            self.on(msg).await?;
        }
    }

    async fn on(&mut self, msg: RegistryMsg) -> Result<(), Stopped> {
        match msg {
            RegistryMsg::Restore => self.restore().await?,
            RegistryMsg::Refreshed(result) => self.refreshed(result),
            RegistryMsg::Start {
                provider,
                directory,
                model,
                effort,
                by,
                reply,
            } => {
                let result = self
                    .start(provider, directory, (model, effort), by)
                    .await?;
                let _ = reply.send(result);
            }
            RegistryMsg::Continue { session, reply } => {
                let relaunch = match self.sessions.get(&session) {
                    None => Err(Rejection::UnknownSession),
                    Some((known, handle)) => {
                        let provider = *known;
                        self.ready(provider)
                            .map(|(launch, _)| {
                                (handle.clone(), launch, self.models(provider))
                            })
                            .map_err(|reason| Rejection::ProviderUnavailable {
                                reason,
                            })
                    }
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
                match (credential.check(), &self.stored) {
                    (Err(reason), _) => {
                        let _ = reply.send(CommandResult::Rejected(
                            Rejection::InvalidCredential { reason },
                        ));
                    }
                    // Saving now would overwrite the other provider's.
                    (Ok(()), Stored::Unreadable(reason)) => {
                        let _ = reply.send(CommandResult::Rejected(
                            Rejection::CredentialNotSaved {
                                reason: reason.clone(),
                            },
                        ));
                    }
                    (Ok(()), Stored::Opened(credentials)) => {
                        let next = credentials.clone().set(credential);
                        self.store(provider, next, reply);
                    }
                }
            }
            RegistryMsg::ClearCredential { provider, reply } => {
                match self.readable() {
                    Ok(credentials) => {
                        self.store(
                            provider,
                            credentials.clear(provider),
                            reply,
                        );
                    }
                    Err(reason) => {
                        let _ = reply.send(CommandResult::Rejected(
                            Rejection::CredentialNotSaved { reason },
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    async fn restore(&mut self) -> Result<(), Stopped> {
        let history = self.log.history().await?;
        let created = history
            .iter()
            .filter(|event| {
                matches!(event.kind, EventKind::SessionCreated { .. })
            })
            .cloned()
            .collect::<Vec<Event>>();
        for event in created {
            self.restore_session(event, &history).await?;
        }
        Ok(())
    }

    async fn restore_session(
        &mut self,
        event: Event,
        history: &[Event],
    ) -> Result<(), Stopped> {
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
            let choice = replay_choice(Choice { model, effort }, &events);
            let conversation = match events.iter().find(|event| {
                matches!(event.kind, EventKind::UserMessage { .. })
            }) {
                Some(_) => self.log.saved_conversation(id).await?,
                None => None,
            };
            withdraw_open_requests(&self.log, id, &events).await?;
            interrupt_running_turn(&self.log, id, &events).await?;
            close_running_agent(&self.log, id, &events).await?;
            let recovered = self
                .log
                .history()
                .await?
                .into_iter()
                .filter(|event| event.session == id)
                .collect::<Vec<Event>>();
            let session = Session::restore(
                id,
                directory,
                choice,
                conversation,
                &recovered,
                self.log.clone(),
            );
            self.sessions.insert(id, (provider, session));
        }
        Ok(())
    }

    /// The stored credentials. A store that doesn't open is moved aside
    /// first, which is how a Clear recovers it from the phone.
    fn readable(&mut self) -> Result<Credentials, String> {
        match &self.stored {
            Stored::Opened(credentials) => Ok(credentials.clone()),
            Stored::Unreadable(_) => {
                let aside = self
                    .store
                    .move_aside()
                    .map_err(|err| format!("{err:#}"))?;
                tracing::warn!(aside = %aside.display(), "moved the unreadable credential store aside");
                self.stored = Stored::Opened(Credentials::none());
                Ok(Credentials::none())
            }
        }
    }

    fn store(
        &mut self,
        provider: Provider,
        next: Credentials,
        reply: oneshot::Sender<CommandResult>,
    ) {
        if let Err(err) = self.store.save(&next) {
            let _ = reply.send(CommandResult::Rejected(
                Rejection::CredentialNotSaved {
                    reason: format!("{err:#}"),
                },
            ));
        } else {
            tracing::info!(?provider, "stored a credential change");
            warn_unprotected(&next);
            let job = RefreshJob {
                id: ulid::Ulid::new(),
                provider,
                launcher: self.launchers.of(provider).clone(),
                credentials: next.clone(),
                directory: self.store.dir().to_path_buf(),
            };
            self.stored = Stored::Opened(next);
            match provider {
                Provider::Claude => {
                    self.claude_refresh.schedule(job, reply);
                }
                Provider::Codex => self.codex_refresh.schedule(job, reply),
            }
        }
    }

    fn refreshed(&mut self, result: Refreshed) {
        let provider = result.provider;
        let completed = match provider {
            Provider::Claude => self.claude_refresh.finish(result),
            Provider::Codex => self.codex_refresh.finish(result),
        };
        if let Some(RefreshCompletion {
            models: listed,
            replies,
        }) = completed
        {
            match (listed, self.launchers.of(provider)) {
                (
                    Ok(models),
                    Launcher::Ready {
                        launch,
                        own,
                        chosen,
                        ..
                    },
                ) => {
                    let ready = Launcher::Ready {
                        models,
                        launch: launch.clone(),
                        own: own.to_owned(),
                        chosen: chosen.clone(),
                    };
                    self.launchers =
                        self.launchers.clone().with(provider, ready);
                }
                (Err(reason), _) => {
                    tracing::warn!(%reason, ?provider, "provider refresh failed");
                }
                (Ok(_), Launcher::Unavailable(_)) => {}
            }
            let result = CommandResult::Providers(self.providers());
            for reply in replies {
                let _ = reply.send(result.clone());
            }
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
            Launcher::Ready { models, chosen, .. } => {
                (models.clone(), chosen.clone())
            }
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
        match provider {
            Provider::Claude => self.claude_refresh.ready()?,
            Provider::Codex => self.codex_refresh.ready()?,
        }
        configured(self.launchers.of(provider), provider, &self.stored)
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
            (Err(_), _, _) => {
                Ok(CommandResult::Rejected(Rejection::NoSuchDirectory))
            }
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
                match crate::workspace::create(self.store.dir(), &directory, id)
                    .await
                {
                    Err(crate::workspace::GitError::NotRepository) => {
                        Ok(CommandResult::Rejected(Rejection::NotGitRepository))
                    }
                    Err(err) => Ok(CommandResult::Rejected(
                        Rejection::WorkspaceFailure {
                            reason: err.to_string(),
                        },
                    )),
                    Ok(workspace) => match crate::workspace::snapshot(
                        &workspace,
                        id,
                        proto::CheckpointId(0),
                        None,
                    )
                    .await
                    {
                        Err(err) => Ok(CommandResult::Rejected(
                            Rejection::WorkspaceFailure {
                                reason: err.to_string(),
                            },
                        )),
                        Ok(commit) => {
                            self.log.append(id, created).await?;
                            let workspace_event = self
                                .log
                                .record(
                                    id,
                                    EventKind::WorkspaceCreated { workspace },
                                )
                                .await?;
                            let checkpoint_event = self
                                .log
                                .record(
                                    id,
                                    EventKind::CheckpointCreated {
                                        checkpoint: proto::Checkpoint {
                                            id: proto::CheckpointId(0),
                                            parent: None,
                                            turn_start: None,
                                            commit,
                                            history: proto::HistoryAnchor::Fresh,
                                            undo: proto::UndoAvailability::Unavailable {
                                                reason: String::from(
                                                    "initial checkpoint has no turn",
                                                ),
                                            },
                                        },
                                    },
                                )
                                .await?;
                            let offered = self.models(provider);
                            let session = Session::spawn(
                                id,
                                directory,
                                choice,
                                (launch, offered),
                                self.log.clone(),
                                &[workspace_event, checkpoint_event],
                            );
                            self.sessions.insert(id, (provider, session));
                            Ok(CommandResult::Started { session: id })
                        }
                    },
                }
            }
        }
    }
}

async fn withdraw_open_requests(
    log: &Log,
    id: SessionId,
    events: &[Event],
) -> Result<(), Stopped> {
    let open = events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::RequestOpened { request, .. }
            | EventKind::QuestionAsked { request, .. } => {
                Some(request.to_owned())
            }
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
        log.append(
            id,
            EventKind::RequestResolved {
                request,
                resolution: proto::Resolution::Withdrawn,
            },
        )
        .await?;
    }
    Ok(())
}

async fn interrupt_running_turn(
    log: &Log,
    id: SessionId,
    events: &[Event],
) -> Result<(), Stopped> {
    let running =
        events.iter().rev().find_map(|event| match &event.kind {
            EventKind::UserMessage { .. }
            | EventKind::NativeTurnStarted { .. } => Some(RecoverTurn::Running),
            EventKind::TurnCompleted { .. }
            | EventKind::SessionClosed { .. } => Some(RecoverTurn::Idle),
            _ => None,
        });
    if matches!(running, Some(RecoverTurn::Running)) {
        recover_checkpoint(log, id, events).await?;
        log.append(
            id,
            EventKind::TurnCompleted {
                outcome: TurnOutcome::Interrupted,
            },
        )
        .await?;
    }
    Ok(())
}

async fn close_running_agent(
    log: &Log,
    id: SessionId,
    events: &[Event],
) -> Result<(), Stopped> {
    let closed = events.iter().rev().find_map(|event| match &event.kind {
        EventKind::SessionClosed { .. } => Some(RecoverAgent::Closed),
        EventKind::SessionCreated { .. } | EventKind::SessionReady => {
            Some(RecoverAgent::Running)
        }
        _ => None,
    });
    if matches!(closed, Some(RecoverAgent::Running)) {
        log.append(
            id,
            EventKind::SessionClosed {
                reason: CloseReason::Stopped {
                    reason: String::from("the host restarted"),
                },
            },
        )
        .await?;
    }
    Ok(())
}

async fn recover_checkpoint(
    log: &Log,
    id: SessionId,
    events: &[Event],
) -> Result<(), Stopped> {
    let book = host::checkpoints::Book::replay(events);
    let started = book
        .snapshot
        .as_ref()
        .map(|snapshot| (snapshot.history.clone(), snapshot.turn_start));
    match (started, book.workspace.as_ref(), book.recovery.as_ref()) {
        (Some((history, turn_start)), Some(workspace), None) => {
            let next = book.highwater.next();
            log.append(id, EventKind::CheckpointReserved { checkpoint: next })
                .await?;
            log.barrier().await?;
            match crate::workspace::snapshot(
                workspace,
                id,
                next,
                book.last().map(|checkpoint| checkpoint.commit.as_str()),
            )
            .await
            {
                Ok(commit) => {
                    let undo = book.availability(&history);
                    log.append(
                        id,
                        EventKind::CheckpointCreated {
                            checkpoint: proto::Checkpoint {
                                id: next,
                                parent: book
                                    .last()
                                    .map(|checkpoint| checkpoint.id),
                                turn_start,
                                commit,
                                history,
                                undo,
                            },
                        },
                    )
                    .await
                }
                Err(err) => {
                    log.append(
                        id,
                        EventKind::Error {
                            message: format!(
                                "interrupted turn checkpoint failed: {err}"
                            ),
                        },
                    )
                    .await
                }
            }
        }
        _ => Ok(()),
    }
}

fn replay_choice(created: Choice, events: &[Event]) -> Choice {
    events
        .iter()
        .fold(created, |current, event| match &event.kind {
            EventKind::ModelChanged { model, effort, .. } => Choice {
                model: model.clone(),
                effort: effort.to_owned(),
            },
            EventKind::EffortChanged { effort } => Choice {
                effort: Some(effort.to_owned()),
                ..current
            },
            _ => current,
        })
}

fn configured(
    launcher: &Launcher,
    provider: Provider,
    stored: &Stored,
) -> Result<(Launch, Auth), String> {
    match (launcher, stored) {
        (Launcher::Unavailable(reason), _)
        | (Launcher::Ready { .. }, Stored::Unreadable(reason)) => {
            Err(reason.clone())
        }
        (Launcher::Ready { launch, own, .. }, Stored::Opened(credentials)) => {
            let credentialed = match launch.clone() {
                Launch::Claude(claude) => Launch::Claude(ClaudeLaunch {
                    credential: credentials.claude.clone(),
                    ..claude
                }),
                Launch::Codex(codex) => Launch::Codex(codex),
            };
            match credentials.auth(provider) {
                Some(auth) => Ok((credentialed, auth)),
                None => own.auth().map(|auth| (credentialed, auth)),
            }
        }
    }
}

/// Without Claude's sandbox the scrub is off (`claude_cli::sandbox`),
/// so the commands Claude runs inherit a gateway token.
fn warn_unprotected(credentials: &Credentials) {
    if let (Some(ClaudeCredential::Gateway { .. }), Sandbox::Missing) =
        (&credentials.claude, claude_cli::sandbox())
    {
        tracing::warn!(
            "Claude's commands can read the gateway token; install bubblewrap and socat to sandbox them"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use install::Home;
    use proto::{
        ClientName, CloseReason, CommandResult, Event, EventKind, HostPath,
        HostSeq, Models, Provider, RequestId, Resolution, SessionId,
        ToolCallId, TurnOutcome,
    };

    use super::{Launcher, Launchers, OwnSignIn, Registry};
    use crate::credentials::Store;
    use crate::log::Log;
    use crate::session::{ClaudeLaunch, Launch, Order};

    fn launcher() -> Launcher {
        Launcher::Ready {
            launch: Launch::Claude(ClaudeLaunch {
                claude_path: Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../claude-cli/tests/fake-claude.sh"),
                home: Home::User,
                credential: None,
            }),
            own: OwnSignIn::Unmanaged,
            models: Models::none(),
            chosen: None,
        }
    }

    fn directory() -> std::io::Result<PathBuf> {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp")
            .join(format!("conductor-test-restart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory)?;
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/e2e/claude-resumed.jsonl"),
            directory.join("stdout.jsonl"),
        )?;
        Ok(directory)
    }

    async fn seed(
        log: &Log,
        id: SessionId,
        kinds: Vec<EventKind>,
    ) -> Result<(), crate::Stopped> {
        for kind in kinds {
            log.append(id, kind).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn restart_replays_the_cursor_withdraws_requests_and_continue_resumes()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = directory()?;
        let path = directory.join("conductor.redb");
        let id = SessionId(u128::from(ulid::Ulid::new()));
        let first = Log::open(String::from("test"), &path)?;
        let epoch = seed_before_restart(&first, id, &directory).await?;
        first.shutdown().await?;
        let second = Log::open(String::from("test"), &path)?;
        let (store, stored) = Store::open(&directory)?;
        let registry = Registry::spawn(
            second.clone(),
            Launchers {
                claude: launcher(),
                codex: Launcher::Unavailable(String::from("test")),
            },
            store,
            stored,
        );
        let providers = registry.providers().await?;
        assert_eq!(second.welcome(providers).await?.epoch, epoch);
        assert_recovered(&second.subscribe(HostSeq(4)).await?.replay);
        let (session, launch, models) = registry
            .relaunch(id)
            .await?
            .map_err(|rejection| format!("{rejection:?}"))?;
        assert_eq!(
            session.order(Order::Start(launch, models)).await?,
            CommandResult::Accepted
        );
        let arguments = std::fs::read_to_string(directory.join("argv.txt"))?;
        assert_eq!(
            arguments
                .lines()
                .find(|argument| argument.starts_with("--resume=")),
            Some("--resume=restored-native-conversation")
        );
        let ready = second.subscribe(HostSeq(7)).await?.replay;
        assert_eq!(
            ready.iter().map(|event| &event.kind).collect::<Vec<_>>(),
            [&EventKind::SessionReady]
        );
        second.shutdown().await?;
        drop(session);
        drop(registry);
        Ok(())
    }

    /// A session mid-turn with an open request and a saved conversation,
    /// and the log's epoch.
    async fn seed_before_restart(
        first: &Log,
        id: SessionId,
        directory: &Path,
    ) -> Result<proto::Epoch, crate::Stopped> {
        seed(
            first,
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
        .await?;
        first
            .conversation(id, String::from("restored-native-conversation"))
            .await?;
        first
            .welcome(proto::Providers {
                claude: proto::Availability::Unavailable {
                    reason: String::from("test"),
                },
                codex: proto::Availability::Unavailable {
                    reason: String::from("test"),
                },
            })
            .await
            .map(|welcome| welcome.epoch)
    }

    fn assert_recovered(replay: &[Event]) {
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
    }
}
