//! The host's event log.
//!
//! One task owns it; [`Log`] handles talk to it over a channel, so nothing
//! is shared behind a lock. The log's rules are [`host::log`]; the task
//! keeps the database, the channels and the live feed, and performs what
//! the rules decide.

use crate::storage::{self, Storage};
use redb::WriteTransaction;
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::Path;

use host::log::{Begun, Completion, Effect, Kind, Log as Core, Ticket};
use proto::{
    Catalog, CommandId, CommandResult, Event, EventKind, Home, HostSeq,
    Instances, SessionId, Subscribe, Welcome,
};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::{Stopped, ask};

/// A subscriber this far behind the live feed is dropped and resubscribes
/// from its cursor (architecture §8).
const LIVE_BUFFER: usize = 1024;

/// What a new subscriber needs: the log after its cursor, and the live feed,
/// attached in the same actor turn as the replay, so nothing falls between.
#[derive(Debug)]
pub struct Subscription {
    pub replay: Vec<Event>,
    pub live: broadcast::Receiver<Event>,
}

#[derive(Debug, Clone)]
pub struct Log {
    inbox: mpsc::Sender<LogMsg>,
}

struct CommandStart {
    key: CommandId,
    kind: Kind,
    reply: oneshot::Sender<BeginCommand>,
}

pub enum BeginCommand {
    Cached(CommandResult),
    Execute(CommandLease),
}

pub struct CommandLease {
    log: Log,
    key: CommandId,
    finishing: bool,
}

impl CommandLease {
    pub(crate) async fn finish(
        mut self,
        completion: impl Into<Completion>,
    ) -> Result<(), Stopped> {
        let (reply, answer) = oneshot::channel();
        self.log
            .inbox
            .send(LogMsg::Finish {
                key: self.key,
                completion: completion.into(),
                reply,
            })
            .await
            .map_err(|_| Stopped {})?;
        self.finishing = true;
        answer.await.map_err(|_| Stopped {})
    }
}

impl Drop for CommandLease {
    fn drop(&mut self) {
        if !self.finishing {
            self.log.abort(self.key);
        }
    }
}

enum LogMsg {
    Shutdown {
        reply: oneshot::Sender<()>,
    },
    Abort(CommandId),
    Begin(CommandStart),
    Finish {
        key: CommandId,
        completion: Completion,
        reply: oneshot::Sender<()>,
    },
    Conversation {
        session: SessionId,
        id: String,
        reply: oneshot::Sender<()>,
    },
    History {
        reply: oneshot::Sender<Vec<Event>>,
    },
    CurrentHistory {
        session: SessionId,
        reply: oneshot::Sender<Vec<Event>>,
    },
    SavedConversation {
        session: SessionId,
        reply: oneshot::Sender<Option<String>>,
    },
    Append {
        session: SessionId,
        kind: EventKind,
        reply: oneshot::Sender<Event>,
    },
    Barrier {
        reply: oneshot::Sender<()>,
    },
    Welcome {
        instances: Instances,
        catalog: Catalog,
        home: Option<Home>,
        reply: oneshot::Sender<Welcome>,
    },
    Subscribe {
        after: HostSeq,
        reply: oneshot::Sender<Subscription>,
    },
    Resume {
        subscribe: Subscribe,
        reply: oneshot::Sender<Option<Subscription>>,
    },
}

struct LogActor {
    inbox: mpsc::WeakSender<LogMsg>,
    core: Core,
    // Drop a pending transaction before its database: redb waits for transactions.
    transaction: Option<WriteTransaction>,
    storage: Option<Storage>,
    begins: HashMap<Ticket, oneshot::Sender<BeginCommand>>,
    finishes: HashMap<Ticket, oneshot::Sender<()>>,
    live: broadcast::Sender<Event>,
}

impl Log {
    fn abort(&self, key: CommandId) {
        match self.inbox.try_send(LogMsg::Abort(key)) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
            Err(mpsc::error::TrySendError::Full(msg)) => {
                if let Ok(_runtime) = tokio::runtime::Handle::try_current() {
                    let inbox = self.inbox.clone();
                    tokio::spawn(async move {
                        let _ = inbox.send(msg).await;
                    });
                }
            }
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn spawn(name: String, epoch: proto::Epoch) -> Self {
        Self::start(Core::new(name, epoch, Vec::new()), None)
    }

    /// Opens the log stored at `path`, creating it if missing.
    ///
    /// # Errors
    ///
    /// Fails if the database can't be opened or its events don't decode.
    pub fn open(name: String, path: &Path) -> anyhow::Result<Self> {
        let storage = Storage::open(path)?;
        let core = Core::new(name, storage.epoch()?, storage.events()?);
        Ok(Self::start(core, Some(storage)))
    }

    fn start(core: Core, storage: Option<Storage>) -> Self {
        let (inbox, messages) = mpsc::channel(64);
        let (live, _) = broadcast::channel(LIVE_BUFFER);
        let actor = LogActor {
            inbox: inbox.downgrade(),
            core,
            transaction: None,
            storage,
            begins: HashMap::new(),
            finishes: HashMap::new(),
            live,
        };
        tokio::spawn(actor.run(messages));
        Self { inbox }
    }

    /// Acknowledges after the durable write, or after staging in the active
    /// command's transaction. The command receipt commits its staged events.
    ///
    /// # Errors
    ///
    /// Fails if the log has stopped.
    pub async fn append(
        &self,
        session: SessionId,
        kind: EventKind,
    ) -> Result<(), Stopped> {
        let _ = self.record(session, kind).await?;
        Ok(())
    }

    pub(crate) async fn record(
        &self,
        session: SessionId,
        kind: EventKind,
    ) -> Result<Event, Stopped> {
        ask(&self.inbox, |reply| LogMsg::Append {
            session,
            kind,
            reply,
        })
        .await
    }

    /// Undo's durable intent/phase boundary before external Git/provider effects.
    /// Keeps the command active, but commits its explicit recovery state now.
    pub(crate) async fn barrier(&self) -> Result<(), Stopped> {
        ask(&self.inbox, |reply| LogMsg::Barrier { reply }).await
    }

    pub(crate) async fn begin(
        &self,
        key: CommandId,
        kind: Kind,
    ) -> Result<BeginCommand, Stopped> {
        ask(&self.inbox, |reply| {
            LogMsg::Begin(CommandStart { key, kind, reply })
        })
        .await
    }

    pub(crate) async fn conversation(
        &self,
        session: SessionId,
        id: String,
    ) -> Result<(), Stopped> {
        ask(&self.inbox, |reply| LogMsg::Conversation {
            session,
            id,
            reply,
        })
        .await
    }

    pub(crate) async fn history(&self) -> Result<Vec<Event>, Stopped> {
        ask(&self.inbox, |reply| LogMsg::History { reply }).await
    }

    pub(crate) async fn finish_history(
        &self,
        session: SessionId,
    ) -> Result<Vec<Event>, Stopped> {
        ask(&self.inbox, |reply| LogMsg::CurrentHistory {
            session,
            reply,
        })
        .await
    }

    pub(crate) async fn saved_conversation(
        &self,
        session: SessionId,
    ) -> Result<Option<String>, Stopped> {
        ask(&self.inbox, |reply| LogMsg::SavedConversation {
            session,
            reply,
        })
        .await
    }

    /// Resolves when the log fails or shuts down, so the host stops its
    /// transport and providers rather than continuing without durable writes.
    pub async fn stopped(&self) {
        self.inbox.closed().await;
    }

    /// Stops accepting writes and releases the database. Stop the transport
    /// before calling this so no command can be acknowledged afterwards.
    ///
    /// # Errors
    ///
    /// Fails if the log has already stopped.
    pub async fn shutdown(&self) -> Result<(), Stopped> {
        ask(&self.inbox, |reply| LogMsg::Shutdown { reply }).await
    }

    /// The log's head as of now, for a client the registry says can run
    /// `instances`, with the host's `home`.
    ///
    /// # Errors
    ///
    /// Fails if the log has stopped.
    pub async fn welcome(
        &self,
        instances: Instances,
        catalog: Catalog,
        home: Option<Home>,
    ) -> Result<Welcome, Stopped> {
        ask(&self.inbox, |reply| LogMsg::Welcome {
            instances,
            catalog,
            home,
            reply,
        })
        .await
    }

    /// The events after `after`, then the live feed.
    ///
    /// # Errors
    ///
    /// Fails if the log has stopped.
    pub async fn subscribe(
        &self,
        after: HostSeq,
    ) -> Result<Subscription, Stopped> {
        ask(&self.inbox, |reply| LogMsg::Subscribe { after, reply }).await
    }

    /// A client's subscription, or `None` if its cursor is from another
    /// epoch.
    ///
    /// # Errors
    ///
    /// Fails if the log has stopped.
    pub async fn resume(
        &self,
        subscribe: Subscribe,
    ) -> Result<Option<Subscription>, Stopped> {
        ask(&self.inbox, |reply| LogMsg::Resume { subscribe, reply }).await
    }
}

impl LogActor {
    async fn run(
        mut self,
        mut messages: mpsc::Receiver<LogMsg>,
    ) -> Result<Infallible, Stopped> {
        loop {
            let msg = messages.recv().await.ok_or(Stopped {})?;
            match msg {
                LogMsg::Shutdown { reply } => {
                    let _ = reply.send(());
                    break Err(Stopped {});
                }
                other => self.on(other).map_err(|err| {
                    tracing::error!(%err, "durable log stopped");
                    Stopped {}
                })?,
            }
        }
    }

    fn on(&mut self, msg: LogMsg) -> anyhow::Result<()> {
        match msg {
            LogMsg::Barrier { reply } => {
                let effects = self.core.barrier();
                self.perform(effects)?;
                let _ = reply.send(());
            }
            LogMsg::Shutdown { reply } => {
                let _ = reply.send(());
            }
            LogMsg::Abort(key) => self.perform(self.core.abort(key))?,
            LogMsg::Begin(start) => {
                let (ticket, effects) = self.core.begin(start.key, start.kind);
                self.begins.insert(ticket, start.reply);
                self.perform(effects)?;
            }
            LogMsg::Finish {
                key,
                completion,
                reply,
            } => {
                let (ticket, effects) = self.core.finish(key, completion);
                self.finishes.insert(ticket, reply);
                self.perform(effects)?;
            }
            LogMsg::History { reply } => {
                let _ = reply.send(self.core.history().to_vec());
            }
            LogMsg::CurrentHistory { session, reply } => {
                let _ = reply.send(self.core.current_history(session));
            }
            LogMsg::SavedConversation { session, reply } => {
                let value = self
                    .storage
                    .as_ref()
                    .map(|storage| storage.conversation(session))
                    .transpose()?
                    .flatten();
                let _ = reply.send(value);
            }
            LogMsg::Conversation { session, id, reply } => {
                self.perform(self.core.conversation(session, id))?;
                let _ = reply.send(());
            }
            LogMsg::Append {
                session,
                kind,
                reply,
            } => {
                let (event, effects) = self.core.append(session, kind);
                self.perform(effects)?;
                let _ = reply.send(event);
            }
            LogMsg::Welcome {
                instances,
                catalog,
                home,
                reply,
            } => {
                let _ = reply.send(self.core.welcome(instances, catalog, home));
            }
            LogMsg::Subscribe { after, reply } => {
                let _ = reply.send(Subscription {
                    live: self.live.subscribe(),
                    replay: self.core.replay(after),
                });
            }
            LogMsg::Resume { subscribe, reply } => {
                let _ =
                    reply.send(self.core.resume(&subscribe).map(|replay| {
                        Subscription {
                            live: self.live.subscribe(),
                            replay,
                        }
                    }));
            }
        }
        Ok(())
    }

    fn perform(&mut self, effects: Vec<Effect>) -> anyhow::Result<()> {
        for effect in effects {
            self.effect(effect)?;
        }
        Ok(())
    }

    fn effect(&mut self, effect: Effect) -> anyhow::Result<()> {
        match effect {
            Effect::Open => {
                self.transaction =
                    self.storage.as_ref().map(Storage::begin).transpose()?;
            }
            Effect::Store(write) => {
                let _ = self
                    .transaction
                    .as_ref()
                    .map(|transaction| storage::write(transaction, &write))
                    .transpose()?;
            }
            Effect::Commit => {
                let _ = self
                    .transaction
                    .take()
                    .map(WriteTransaction::commit)
                    .transpose()?;
            }
            Effect::Publish(event) => {
                let _ = self.live.send(event);
            }
            Effect::Lookup(key) => {
                let found = self
                    .storage
                    .as_ref()
                    .map(|storage| storage.receipt(key))
                    .transpose()?
                    .flatten();
                let effects = self.core.receipt(found);
                self.perform(effects)?;
            }
            Effect::Begun(ticket, begun) => self.answer(ticket, begun)?,
            Effect::Finished(ticket) => {
                let _ =
                    self.finishes.remove(&ticket).map(|reply| reply.send(()));
            }
            Effect::Stop => return Err(Stopped {}.into()),
        }
        Ok(())
    }

    fn answer(&mut self, ticket: Ticket, begun: Begun) -> anyhow::Result<()> {
        let answer = match begun {
            Begun::Cached(result) => BeginCommand::Cached(result),
            Begun::Execute(key) => {
                let inbox = self.inbox.upgrade().ok_or(Stopped {})?;
                BeginCommand::Execute(CommandLease {
                    finishing: false,
                    log: Log { inbox },
                    key,
                })
            }
        };
        if let Some(reply) = self.begins.remove(&ticket) {
            let _ = reply.send(answer);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use proto::{
        Auth, Availability, Catalog, Epoch, EventKind, HostSeq, InstanceId,
        InstanceStatus, Instances, Models, Provider, SessionId, Welcome,
    };

    use super::Log;

    fn delta(text: &str) -> EventKind {
        EventKind::AssistantDelta {
            text: String::from(text),
        }
    }

    fn message(text: &str) -> EventKind {
        EventKind::AssistantMessage {
            text: String::from(text),
        }
    }

    fn instances() -> Instances {
        Instances(
            Provider::ALL
                .into_iter()
                .zip([
                    Availability::Available {
                        auth: Auth::SignedIn,
                        models: Models::none(),
                    },
                    Availability::Unavailable {
                        reason: String::from("not in this test"),
                    },
                ])
                .map(|(provider, availability)| {
                    (InstanceId::own(provider), InstanceStatus { availability })
                })
                .collect(),
        )
    }

    #[tokio::test]
    async fn deltas_go_live_with_the_last_seq_and_are_never_replayed()
    -> Result<(), Box<dyn std::error::Error>> {
        let log = Log::spawn(String::from("test"), proto::Epoch(5));
        let mut live = log.subscribe(HostSeq(0)).await?.live;
        log.append(SessionId(1), EventKind::SessionReady).await?;
        log.append(SessionId(1), delta("a")).await?;
        log.append(
            SessionId(1),
            EventKind::ReasoningDelta {
                text: String::from("thinking"),
            },
        )
        .await?;
        log.append(SessionId(1), message("a")).await?;
        assert_eq!(
            log.welcome(instances(), Catalog::default(), None).await?,
            Welcome {
                host: String::from("test"),
                epoch: Epoch(5),
                head_seq: HostSeq(2),
                instances: instances(),
                catalog: Catalog::default(),
                home: None,
            }
        );
        let seen = [
            live.recv().await?,
            live.recv().await?,
            live.recv().await?,
            live.recv().await?,
        ];
        assert_eq!(
            seen.into_iter()
                .map(|event| (event.seq, event.kind))
                .collect::<Vec<(HostSeq, EventKind)>>(),
            [
                (HostSeq(1), EventKind::SessionReady),
                (HostSeq(1), delta("a")),
                (
                    HostSeq(1),
                    EventKind::ReasoningDelta {
                        text: String::from("thinking"),
                    },
                ),
                (HostSeq(2), message("a")),
            ]
        );
        let replay = log.subscribe(HostSeq(1)).await?.replay;
        assert_eq!(
            replay
                .into_iter()
                .map(|event| (event.seq, event.kind))
                .collect::<Vec<(HostSeq, EventKind)>>(),
            [(HostSeq(2), message("a"))]
        );
        Ok(())
    }
}

#[cfg(test)]
mod durability_tests {
    use std::time::Duration;

    use proto::{CommandId, CommandResult, EventKind, HostSeq, SessionId};

    use super::{BeginCommand, CommandLease, CommandStart, Kind, Log, LogMsg};
    use tokio::sync::oneshot;

    async fn lease(
        log: &Log,
        key: u128,
    ) -> Result<CommandLease, crate::Stopped> {
        match log.begin(CommandId(key), Kind::Session).await? {
            BeginCommand::Execute(lease) => Ok(lease),
            BeginCommand::Cached(_) => Err(crate::Stopped {}),
        }
    }

    #[tokio::test]
    async fn receipts_wait_for_commit_without_holding_the_event_transaction()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp");
        std::fs::create_dir_all(&directory)?;
        let path =
            directory.join(format!("receipts-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let log = Log::open(String::from("test"), &path)?;
        let receipt = match log.begin(CommandId(23), Kind::Receipt).await? {
            BeginCommand::Execute(lease) => Some(lease),
            BeginCommand::Cached(_) => None,
        }
        .ok_or("expected a value")?;
        let ordinary =
            tokio::time::timeout(Duration::from_secs(2), lease(&log, 24))
                .await??;
        let (finished_reply, mut finished) = oneshot::channel();
        log.inbox
            .send(LogMsg::Finish {
                key: CommandId(23),
                completion: CommandResult::Accepted.into(),
                reply: finished_reply,
            })
            .await?;
        let (retry_reply, retry) = oneshot::channel();
        log.inbox
            .send(LogMsg::Begin(CommandStart {
                key: CommandId(23),
                kind: Kind::Session,
                reply: retry_reply,
            }))
            .await?;
        let _ = log.history().await?;
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(20), &mut finished)
                .await
                .err()
                .map(|_| "pending"),
            Some("pending")
        );
        ordinary.finish(CommandResult::Accepted).await?;
        finished.await?;
        drop(receipt);
        let retried = match retry.await? {
            BeginCommand::Cached(result) => Some(result),
            BeginCommand::Execute(_) => None,
        };
        assert_eq!(retried, Some(CommandResult::Accepted));
        log.shutdown().await?;
        let reopened = Log::open(String::from("test"), &path)?;
        let cached = match reopened.begin(CommandId(23), Kind::Receipt).await? {
            BeginCommand::Cached(result) => Some(result),
            BeginCommand::Execute(_) => None,
        };
        assert_eq!(cached, Some(CommandResult::Accepted));
        reopened.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn receipt_lease_drop_fails_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        let log = Log::spawn(String::from("test"), proto::Epoch(1));
        let receipt = match log.begin(CommandId(23), Kind::Receipt).await? {
            BeginCommand::Execute(lease) => Some(lease),
            BeginCommand::Cached(_) => None,
        }
        .ok_or("expected a value")?;
        drop(receipt);
        tokio::time::timeout(Duration::from_secs(2), log.stopped()).await?;
        Ok(())
    }

    #[tokio::test]
    async fn receipt_and_events_commit_before_live_and_queued_retries_drain()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir()
            .join(format!("conductor-staging-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let log = Log::open(String::from("test"), &path)?;
        let first = lease(&log, 1).await?;
        let super::Subscription { mut live, .. } =
            log.subscribe(HostSeq(0)).await?;
        log.append(SessionId(1), EventKind::SessionReady).await?;
        log.append(
            SessionId(1),
            EventKind::AssistantDelta {
                text: String::from("a"),
            },
        )
        .await?;
        assert_eq!(log.subscribe(HostSeq(0)).await?.replay, Vec::new());
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(20), live.recv())
                .await
                .err()
                .map(|_| String::from("staged")),
            Some(String::from("staged"))
        );
        let (duplicate_reply, duplicate) = oneshot::channel();
        log.inbox
            .send(LogMsg::Begin(CommandStart {
                key: CommandId(1),
                kind: Kind::Session,
                reply: duplicate_reply,
            }))
            .await?;
        let (next_reply, next) = oneshot::channel();
        log.inbox
            .send(LogMsg::Begin(CommandStart {
                key: CommandId(2),
                kind: Kind::Session,
                reply: next_reply,
            }))
            .await?;
        first.finish(CommandResult::Accepted).await?;
        assert_eq!(live.recv().await?.kind, EventKind::SessionReady);
        assert_eq!(live.recv().await?.seq, HostSeq(1));
        let cached = match duplicate.await? {
            BeginCommand::Cached(result) => Some(result),
            BeginCommand::Execute(_) => None,
        };
        assert_eq!(cached, Some(CommandResult::Accepted));
        let second = match next.await? {
            BeginCommand::Execute(lease) => Some(lease),
            BeginCommand::Cached(_) => None,
        }
        .ok_or("expected a value")?;
        second.finish(CommandResult::Accepted).await?;
        log.shutdown().await?;
        let reopened = Log::open(String::from("test"), &path)?;
        assert_eq!(reopened.subscribe(HostSeq(0)).await?.replay.len(), 1);
        let cached = match reopened.begin(CommandId(1), Kind::Session).await? {
            BeginCommand::Cached(result) => Some(result),
            BeginCommand::Execute(_) => None,
        };
        assert_eq!(cached, Some(CommandResult::Accepted));
        reopened.shutdown().await?;
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_undo_preserves_committed_intent_but_rolls_back_its_suffix()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!(
            "conductor-undo-barrier-{}.redb",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let log = Log::open(String::from("test"), &path)?;
        let command = lease(&log, 9).await?;
        let started = EventKind::UndoStarted {
            checkpoint: proto::CheckpointId(2),
            restore: proto::CheckpointId(1),
            history: proto::HistoryAnchor::Fresh,
        };
        log.append(SessionId(1), started.clone()).await?;
        log.barrier().await?;
        assert_eq!(
            log.subscribe(HostSeq(0))
                .await?
                .replay
                .into_iter()
                .map(|event| event.kind)
                .collect::<Vec<EventKind>>(),
            Vec::from([started.clone()])
        );
        log.append(
            SessionId(1),
            EventKind::UndoHistoryRestored {
                checkpoint: proto::CheckpointId(2),
                history: proto::HistoryAnchor::Fresh,
            },
        )
        .await?;
        let inbox = log.inbox.clone();
        drop(command);
        tokio::time::timeout(Duration::from_secs(2), inbox.closed()).await?;
        let reopened = Log::open(String::from("test"), &path)?;
        assert_eq!(
            reopened
                .history()
                .await?
                .into_iter()
                .map(|event| event.kind)
                .collect::<Vec<EventKind>>(),
            Vec::from([started])
        );
        let retried = lease(&reopened, 9).await?;
        retried.finish(CommandResult::Accepted).await?;
        reopened.shutdown().await?;
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn dropping_an_unfinished_command_rolls_back_and_fails_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir()
            .join(format!("conductor-abort-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let log = Log::open(String::from("test"), &path)?;
        let command = lease(&log, 1).await?;
        log.append(SessionId(1), EventKind::SessionReady).await?;
        let inbox = log.inbox.clone();
        drop(command);
        tokio::time::timeout(Duration::from_secs(2), inbox.closed()).await?;
        assert_eq!(
            log.history().await.err().map(|_| String::from("stopped")),
            Some(String::from("stopped"))
        );
        let reopened = Log::open(String::from("test"), &path)?;
        assert_eq!(reopened.history().await?, Vec::new());
        let retried = lease(&reopened, 1).await?;
        retried.finish(CommandResult::Accepted).await?;
        reopened.shutdown().await?;
        let _ = std::fs::remove_file(path);
        Ok(())
    }
}
