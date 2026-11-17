//! The registry's actor: the one place a `StartSession` and a `Continue`
//! are checked and a session is found by its id.
//!
//! Its decisions are `host::registry`'s; the actor keeps the store, the
//! sessions' tasks and the providers' refreshes, and performs what the
//! registry decides.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::Path;

use agents::runtime::Command;
use host::registry::{Job, Launch, Listing, Refreshed};
use host::restore::{Created, Interrupted, Recovery, events_of};
use proto::{
    Checkpoint, CheckpointId, ClientName, CommandResult, Credential,
    Credentials, Effort, Event, EventKind, HistoryAnchor, HostPath, ModelId,
    Models, Provider, Providers, Rejection, RequestId, Resolution, SessionId,
    TurnOutcome, UndoAvailability, Workspace,
};
use runtime::{Runtime, Starter};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;

use crate::Stopped;
use crate::credentials::{Store, Stored};
use crate::log::Log;
use crate::session::Session;
use crate::workspace::GitError;

pub use host::registry::{Offer, OwnSignIn};

/// How the host runs a provider.
pub type Launcher = host::registry::Launcher<Starter>;

/// How the host runs each provider it knows of. Built once in `main`.
pub type Launchers = BTreeMap<Provider, Launcher>;

type Reply = oneshot::Sender<CommandResult>;
type Core = host::registry::Registry<Starter, Session, Reply>;
type Effect = host::registry::Effect<Starter, Reply>;

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
    Providers {
        reply: oneshot::Sender<Providers>,
    },
    SetCredential {
        credential: Credential,
        reply: Reply,
    },
    ClearCredential {
        provider: Provider,
        reply: Reply,
    },
}

struct RegistryActor {
    log: Log,
    store: Store,
    core: Core,
    refreshes: JoinSet<Refreshed>,
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
        let (inbox, messages) = mpsc::channel(16);
        let actor = RegistryActor {
            log,
            store,
            core: Core::new(launchers, stored),
            refreshes: JoinSet::new(),
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
    pub async fn providers(&self) -> Result<Providers, Stopped> {
        self.ask(|reply| RegistryMsg::Providers { reply }).await
    }

    /// Stores the credential and answers with the providers it changes,
    /// once the provider has refreshed.
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
                Some(Ok(result)) = self.refreshes.join_next() => RegistryMsg::Refreshed(result),
            };
            self.on(msg).await?;
        }
    }

    async fn on(&mut self, msg: RegistryMsg) -> Result<(), Stopped> {
        match msg {
            RegistryMsg::Restore => self.restore().await?,
            RegistryMsg::Refreshed(result) => {
                let effects = self.core.refreshed(result);
                self.perform(effects);
            }
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
                let _ = reply.send(self.core.relaunch(session));
            }
            RegistryMsg::Find { session, reply } => {
                let _ = reply.send(self.core.find(session));
            }
            RegistryMsg::Providers { reply } => {
                let _ = reply.send(self.core.providers());
            }
            RegistryMsg::SetCredential { credential, reply } => {
                let effects = self.core.set_credential(credential, reply);
                self.perform(effects);
            }
            RegistryMsg::ClearCredential { provider, reply } => {
                let effects = self.core.clear_credential(provider, reply);
                self.perform(effects);
            }
        }
        Ok(())
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
            Effect::MoveAside { provider, reply } => {
                let moved = self
                    .store
                    .move_aside()
                    .map(|aside| {
                        tracing::warn!(aside = %aside.display(), "moved the unreadable credential store aside");
                    })
                    .map_err(|err| format!("{err:#}"));
                self.core.moved_aside(provider, reply, moved)
            }
            Effect::Save {
                provider,
                credentials,
                reply,
            } => {
                let saved = self
                    .store
                    .save(&credentials)
                    .map_err(|err| format!("{err:#}"));
                self.core.saved(provider, credentials, reply, saved)
            }
            Effect::Refresh(job) => {
                self.refreshes.spawn(refresh(job));
                Vec::new()
            }
            Effect::Log(note) => {
                runtime::log(&note);
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
            provider,
            directory,
            choice,
        } = created;
        let recovery = Recovery::of(choice, &events_of(id, history));
        let conversation = self.conversation(id, recovery.talked).await?;
        withdraw(&self.log, id, recovery.withdrawn).await?;
        interrupt(&self.log, id, recovery.interrupted, recovery.checkpoint)
            .await?;
        close(&self.log, id, recovery.closed).await?;
        let recovered = events_of(id, &self.log.history().await?);
        let runtime = self
            .core
            .launcher(provider)
            .map_or_else(Runtime::refusing, |start| start.start());
        let session = Session::restore(
            id,
            directory,
            recovery.choice,
            (runtime, conversation),
            &recovered,
            self.log.clone(),
        );
        self.core.admit(id, provider, session);
        Ok(())
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
        provider: Provider,
        directory: HostPath,
        asked: (Option<ModelId>, Option<Effort>),
        by: ClientName,
    ) -> Result<CommandResult, Stopped> {
        // `read_dir` fails for a missing path and for a file alike.
        let exists = std::fs::read_dir(&directory.0).is_ok();
        let (model, effort) = asked;
        match self.core.start(provider, exists, model, effort) {
            Err(rejection) => Ok(CommandResult::Rejected(rejection)),
            Ok(launch) => self.create(provider, directory, launch, by).await,
        }
    }

    /// Makes the session's workspace and first checkpoint, logs them, and
    /// starts the session.
    async fn create(
        &mut self,
        provider: Provider,
        directory: HostPath,
        launch: Launch<Starter>,
        by: ClientName,
    ) -> Result<CommandResult, Stopped> {
        let id = SessionId(u128::from(ulid::Ulid::new()));
        match prepare(self.store.dir(), &directory, id).await {
            Err(rejection) => Ok(CommandResult::Rejected(rejection)),
            Ok((workspace, commit)) => {
                let Launch {
                    start,
                    choice,
                    offered,
                } = launch;
                let created = EventKind::SessionCreated {
                    provider,
                    directory: directory.clone(),
                    by,
                    model: choice.model.clone(),
                    effort: choice.effort,
                };
                self.log.append(id, created).await?;
                let workspace_event = self
                    .log
                    .record(id, EventKind::WorkspaceCreated { workspace })
                    .await?;
                let checkpoint = initial_checkpoint(commit);
                let checkpoint_event = self
                    .log
                    .record(id, EventKind::CheckpointCreated { checkpoint })
                    .await?;
                let session = Session::spawn(
                    id,
                    directory,
                    choice,
                    (start.start(), offered),
                    self.log.clone(),
                    &[workspace_event, checkpoint_event],
                );
                self.core.admit(id, provider, session);
                Ok(CommandResult::Started { session: id })
            }
        }
    }
}

/// The provider's agents take the credentials, and then it lists what it
/// offers with them, if it then signs in.
async fn refresh(job: Job<Starter>) -> Refreshed {
    Refreshed {
        id: job.id,
        provider: job.provider,
        listing: listing(job.start, job.credentials, job.signs_in).await,
    }
}

async fn listing(
    start: Starter,
    credentials: Credentials,
    signs_in: Result<(), String>,
) -> Result<Listing, String> {
    let mut runtime = start.start();
    let named = runtime
        .ask(Command::Credentials(credentials))
        .await
        .configured()?;
    signs_in?;
    let models = runtime.ask(Command::Models).await.models()?;
    Ok(Listing { models, named })
}

/// The session's workspace and the commit of its first checkpoint.
async fn prepare(
    data_dir: &Path,
    directory: &HostPath,
    id: SessionId,
) -> Result<(Workspace, String), Rejection> {
    let workspace = crate::workspace::create(data_dir, directory, id)
        .await
        .map_err(refused)?;
    let commit =
        crate::workspace::snapshot(&workspace, id, CheckpointId(0), None)
            .await
            .map_err(|err| Rejection::WorkspaceFailure {
                reason: err.to_string(),
            })?;
    Ok((workspace, commit))
}

fn refused(err: GitError) -> Rejection {
    match err {
        GitError::NotRepository => Rejection::NotGitRepository,
        GitError::Failed(reason) => Rejection::WorkspaceFailure { reason },
    }
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

async fn withdraw(
    log: &Log,
    id: SessionId,
    open: Vec<RequestId>,
) -> Result<(), Stopped> {
    for request in open {
        let withdrawn = EventKind::RequestResolved {
            request,
            resolution: Resolution::Withdrawn,
        };
        log.append(id, withdrawn).await?;
    }
    Ok(())
}

async fn interrupt(
    log: &Log,
    id: SessionId,
    interrupted: bool,
    turn: Option<Interrupted>,
) -> Result<(), Stopped> {
    if interrupted {
        checkpoint(log, id, turn).await?;
        let outcome = TurnOutcome::Interrupted;
        log.append(id, EventKind::TurnCompleted { outcome }).await?;
    }
    Ok(())
}

async fn close(log: &Log, id: SessionId, closed: bool) -> Result<(), Stopped> {
    if closed {
        let reason = host::restore::restarted();
        log.append(id, EventKind::SessionClosed { reason }).await?;
    }
    Ok(())
}

/// Snapshots an interrupted turn's files, when it has a workspace.
async fn checkpoint(
    log: &Log,
    id: SessionId,
    interrupted: Option<Interrupted>,
) -> Result<(), Stopped> {
    let Some(turn) = interrupted else {
        return Ok(());
    };
    let reserved = turn.reserved;
    log.append(
        id,
        EventKind::CheckpointReserved {
            checkpoint: reserved,
        },
    )
    .await?;
    log.barrier().await?;
    let snapshot = crate::workspace::snapshot(
        &turn.workspace,
        id,
        reserved,
        turn.base.as_deref(),
    )
    .await;
    match snapshot {
        Ok(commit) => {
            let checkpoint = turn.checkpoint(commit);
            log.append(id, EventKind::CheckpointCreated { checkpoint })
                .await
        }
        Err(err) => {
            let message = format!("interrupted turn checkpoint failed: {err}");
            log.append(id, EventKind::Error { message }).await
        }
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

    use super::{Launcher, Launchers, Offer, OwnSignIn, Registry};
    use crate::credentials::Store;
    use crate::log::Log;
    use crate::session::Order;

    fn launcher(directory: &Path) -> Launcher {
        Launcher::Ready {
            start: claude_cli::starter(claude_cli::ClaudeLaunch {
                claude_path: Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../claude-cli/tests/fake-claude.sh"),
                home: Home::User,
                credential: None,
                probe: directory.join("probe"),
            }),
            own: OwnSignIn::Unmanaged,
            offer: Offer {
                listed: Models::none(),
                named: None,
                chosen: None,
            },
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
            Launchers::from([
                (Provider::Claude, launcher(&directory)),
                (Provider::Codex, Launcher::Unavailable(String::from("test"))),
            ]),
            store,
            stored,
        );
        let providers = registry.providers().await?;
        assert_eq!(second.welcome(providers).await?.epoch, epoch);
        assert_recovered(&second.subscribe(HostSeq(4)).await?.replay);
        let (session, models) = registry
            .relaunch(id)
            .await?
            .map_err(|rejection| format!("{rejection:?}"))?;
        assert_eq!(
            session.order(Order::Start(models)).await?,
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
            .welcome(proto::Providers(
                Provider::ALL
                    .map(|provider| {
                        (
                            provider,
                            proto::Availability::Unavailable {
                                reason: String::from("test"),
                            },
                        )
                    })
                    .into(),
            ))
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
