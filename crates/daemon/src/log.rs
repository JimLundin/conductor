//! The host's event log. One task owns it and assigns every stored event its
//! seq; [`Log`] handles talk to it over a channel, so nothing is shared
//! behind a lock.

use crate::storage::{self, Storage};
use redb::WriteTransaction;
use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::path::Path;

use proto::{
    CommandId, CommandResult, Epoch, Event, EventKind, HostSeq, Providers,
    SessionId, Welcome,
};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::Stopped;

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
    kind: CommandKind,
    reply: oneshot::Sender<BeginCommand>,
}

enum CommandKind {
    Session,
    Credential,
}

struct CredentialFinish {
    key: CommandId,
    result: CommandResult,
    reply: oneshot::Sender<()>,
}

pub(crate) enum BeginCommand {
    Cached(CommandResult),
    Execute(CommandLease),
}

pub(crate) struct CommandLease {
    log: Log,
    key: CommandId,
}

impl CommandLease {
    pub(crate) async fn finish(
        self,
        result: CommandResult,
    ) -> Result<(), Stopped> {
        self.log.finish(self.key, result).await
    }
}

impl Drop for CommandLease {
    fn drop(&mut self) {
        match self.log.inbox.try_send(LogMsg::Abort(self.key)) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
            Err(mpsc::error::TrySendError::Full(msg)) => {
                if let Ok(_runtime) = tokio::runtime::Handle::try_current() {
                    let inbox = self.log.inbox.clone();
                    tokio::spawn(async move {
                        let _ = inbox.send(msg).await;
                    });
                }
            }
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
        result: CommandResult,
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
        providers: Providers,
        reply: oneshot::Sender<Welcome>,
    },
    Subscribe {
        after: HostSeq,
        reply: oneshot::Sender<Subscription>,
    },
}

struct LogActor {
    inbox: mpsc::WeakSender<LogMsg>,
    active: Option<CommandId>,
    // Drop a pending transaction before its database: redb waits for transactions.
    transaction: Option<WriteTransaction>,
    storage: Option<Storage>,
    staged: Vec<Event>,
    waiting: VecDeque<CommandStart>,
    credentials: HashMap<CommandId, Vec<oneshot::Sender<BeginCommand>>>,
    credential_finishes: VecDeque<CredentialFinish>,
    name: String,
    epoch: Epoch,
    events: Vec<Event>,
    live: broadcast::Sender<Event>,
}

impl Log {
    #[must_use]
    pub fn spawn(name: String, epoch: Epoch) -> Self {
        Self::start(name, epoch, Vec::new(), None)
    }

    /// Opens the log stored at `path`, creating it if missing.
    ///
    /// # Errors
    ///
    /// Fails if the database can't be opened or its events don't decode.
    pub fn open(name: String, path: &Path) -> anyhow::Result<Self> {
        let storage = Storage::open(path)?;
        let epoch = storage.epoch()?;
        let events = storage.events()?;
        Ok(Self::start(name, epoch, events, Some(storage)))
    }

    fn start(
        name: String,
        epoch: Epoch,
        events: Vec<Event>,
        storage: Option<Storage>,
    ) -> Self {
        let (inbox, messages) = mpsc::channel(64);
        let (live, _) = broadcast::channel(LIVE_BUFFER);
        let actor = LogActor {
            inbox: inbox.downgrade(),
            active: None,
            storage,
            transaction: None,
            staged: Vec::new(),
            waiting: VecDeque::new(),
            credentials: HashMap::new(),
            credential_finishes: VecDeque::new(),
            name,
            epoch,
            events,
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
        self.ask(|reply| LogMsg::Append {
            session,
            kind,
            reply,
        })
        .await
    }

    /// Undo's durable intent/phase boundary before external Git/provider effects.
    /// Keeps the command active, but commits its explicit recovery state now.
    pub(crate) async fn barrier(&self) -> Result<(), Stopped> {
        self.ask(|reply| LogMsg::Barrier { reply }).await
    }

    pub(crate) async fn begin(
        &self,
        key: CommandId,
    ) -> Result<BeginCommand, Stopped> {
        self.ask(|reply| {
            LogMsg::Begin(CommandStart {
                key,
                kind: CommandKind::Session,
                reply,
            })
        })
        .await
    }

    pub(crate) async fn begin_credential(
        &self,
        key: CommandId,
    ) -> Result<BeginCommand, Stopped> {
        self.ask(|reply| {
            LogMsg::Begin(CommandStart {
                key,
                kind: CommandKind::Credential,
                reply,
            })
        })
        .await
    }

    pub(crate) async fn finish(
        &self,
        key: CommandId,
        result: CommandResult,
    ) -> Result<(), Stopped> {
        self.ask(|reply| LogMsg::Finish { key, result, reply })
            .await
    }

    pub(crate) async fn conversation(
        &self,
        session: SessionId,
        id: String,
    ) -> Result<(), Stopped> {
        self.ask(|reply| LogMsg::Conversation { session, id, reply })
            .await
    }

    pub(crate) async fn history(&self) -> Result<Vec<Event>, Stopped> {
        self.ask(|reply| LogMsg::History { reply }).await
    }

    pub(crate) async fn saved_conversation(
        &self,
        session: SessionId,
    ) -> Result<Option<String>, Stopped> {
        self.ask(|reply| LogMsg::SavedConversation { session, reply })
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
        self.ask(|reply| LogMsg::Shutdown { reply }).await
    }

    /// The log's head as of now, for a client the registry says can run
    /// `providers`.
    ///
    /// # Errors
    ///
    /// Fails if the log has stopped.
    pub async fn welcome(
        &self,
        providers: Providers,
    ) -> Result<Welcome, Stopped> {
        self.ask(|reply| LogMsg::Welcome { providers, reply }).await
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
        self.ask(|reply| LogMsg::Subscribe { after, reply }).await
    }

    async fn ask<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> LogMsg,
    ) -> Result<T, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.inbox
            .send(build(reply))
            .await
            .map_err(|_| Stopped {})?;
        answer.await.map_err(|_| Stopped {})
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
                self.barrier()?;
                let _ = reply.send(());
            }
            LogMsg::Shutdown { reply } => {
                let _ = reply.send(());
            }
            LogMsg::Abort(key) => self.abort(key)?,
            LogMsg::Begin(start) => match &self.active {
                Some(_) => self.waiting.push_back(start),
                None => self.begin(start)?,
            },
            LogMsg::Finish { key, result, reply } => {
                self.finish(CredentialFinish { key, result, reply })?;
            }
            LogMsg::History { reply } => {
                let _ = reply.send(self.events.clone());
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
                self.save_conversation(session, &id)?;
                let _ = reply.send(());
            }
            // Live only, so it takes no seq (see `EventKind::AssistantDelta`).
            LogMsg::Append {
                session,
                kind:
                    kind @ (EventKind::AssistantDelta { .. }
                    | EventKind::ReasoningDelta { .. }),
                reply,
            } => {
                let _ = reply.send(self.append_live(session, kind));
            }
            LogMsg::Append {
                session,
                kind,
                reply,
            } => {
                let _ = reply.send(self.append(session, kind)?);
            }
            LogMsg::Welcome { providers, reply } => {
                let _ = reply.send(Welcome {
                    host: self.name.clone(),
                    epoch: self.epoch,
                    head_seq: self.head(),
                    providers,
                });
            }
            LogMsg::Subscribe { after, reply } => {
                let _ = reply.send(Subscription {
                    live: self.live.subscribe(),
                    replay: self
                        .events
                        .iter()
                        .filter(|event| event.seq > after)
                        .cloned()
                        .collect(),
                });
            }
        }
        Ok(())
    }

    fn barrier(&mut self) -> anyhow::Result<()> {
        self.commit()?;
        if self.active.is_some() {
            self.transaction =
                self.storage.as_ref().map(Storage::begin).transpose()?;
        }
        Ok(())
    }

    fn abort(&self, key: CommandId) -> anyhow::Result<()> {
        match (self.credentials.get(&key), &self.active) {
            (Some(_), _) => Err(Stopped {}.into()),
            (None, Some(active)) if active == &key => Err(Stopped {}.into()),
            _ => Ok(()),
        }
    }

    fn finish(&mut self, finish: CredentialFinish) -> anyhow::Result<()> {
        match (self.credentials.get(&finish.key), &self.active) {
            (Some(_), Some(_)) => {
                self.credential_finishes.push_back(finish);
            }
            (Some(_), None) => self.finish_credential(finish)?,
            (None, _) => {
                if let Some(write) = self.transaction.take() {
                    storage::receipt(&write, finish.key, &finish.result)?;
                    self.transaction = Some(write);
                }
                self.commit()?;
                self.active = None;
                let _ = finish.reply.send(());
                for credential in std::mem::take(&mut self.credential_finishes)
                {
                    self.finish_credential(credential)?;
                }
                if let Some(start) = self.waiting.pop_front() {
                    self.begin(start)?;
                }
            }
        }
        Ok(())
    }

    fn save_conversation(
        &self,
        session: SessionId,
        id: &str,
    ) -> anyhow::Result<()> {
        match (&self.storage, &self.transaction) {
            (_, Some(write)) => {
                storage::conversation(write, session, id)?;
            }
            (Some(storage), None) => {
                let write = storage.begin()?;
                storage::conversation(&write, session, id)?;
                write.commit()?;
            }
            (None, None) => {}
        }
        Ok(())
    }

    fn append_live(&mut self, session: SessionId, kind: EventKind) -> Event {
        let event = Event {
            seq: self
                .staged
                .last()
                .map_or_else(|| self.head(), |event| event.seq),
            session,
            kind,
        };
        let recorded = event.clone();
        match &self.active {
            Some(_) => self.staged.push(event),
            None => {
                let _ = self.live.send(event);
            }
        }
        recorded
    }

    fn append(
        &mut self,
        session: SessionId,
        kind: EventKind,
    ) -> anyhow::Result<Event> {
        let event = Event {
            seq: self
                .staged
                .last()
                .map_or_else(|| self.head(), |event| event.seq)
                .next(),
            session,
            kind,
        };
        let recorded = event.clone();
        match (&self.storage, &self.transaction) {
            (_, Some(write)) => {
                storage::event(write, &event)?;
                self.staged.push(event);
            }
            (Some(storage), None) => {
                let write = storage.begin()?;
                storage::event(&write, &event)?;
                write.commit()?;
                self.publish(event);
            }
            (None, None) => match &self.active {
                Some(_) => self.staged.push(event),
                None => self.publish(event),
            },
        }
        Ok(recorded)
    }

    fn begin(&mut self, start: CommandStart) -> anyhow::Result<()> {
        let receipt = self
            .storage
            .as_ref()
            .map(|storage| storage.receipt(start.key))
            .transpose()?
            .flatten();
        match (receipt, self.credentials.get_mut(&start.key)) {
            (Some(result), _) => {
                let _ = start.reply.send(BeginCommand::Cached(result));
            }
            (None, Some(waiters)) => waiters.push(start.reply),
            (None, None) => {
                match start.kind {
                    CommandKind::Session => {
                        self.transaction = self
                            .storage
                            .as_ref()
                            .map(Storage::begin)
                            .transpose()?;
                        self.active = Some(start.key);
                    }
                    CommandKind::Credential => {
                        self.credentials.insert(start.key, Vec::new());
                    }
                }
                let inbox = self.inbox.upgrade().ok_or(Stopped {})?;
                let lease = CommandLease {
                    log: Log { inbox },
                    key: start.key,
                };
                let _ = start.reply.send(BeginCommand::Execute(lease));
            }
        }
        let queued = match self.active {
            None => self.waiting.pop_front(),
            Some(_) => None,
        };
        if let Some(next) = queued {
            self.begin(next)?;
        }
        Ok(())
    }

    fn finish_credential(
        &mut self,
        finish: CredentialFinish,
    ) -> anyhow::Result<()> {
        if let Some(storage) = &self.storage {
            let write = storage.begin()?;
            storage::receipt(&write, finish.key, &finish.result)?;
            write.commit()?;
        }
        if let Some(waiters) = self.credentials.remove(&finish.key) {
            for waiter in waiters {
                let _ =
                    waiter.send(BeginCommand::Cached(finish.result.clone()));
            }
        }
        let _ = finish.reply.send(());
        Ok(())
    }

    fn commit(&mut self) -> anyhow::Result<()> {
        if let Some(write) = self.transaction.take() {
            write.commit()?;
        }
        let staged = std::mem::take(&mut self.staged);
        for event in staged {
            self.publish(event);
        }
        Ok(())
    }

    fn publish(&mut self, event: Event) {
        match &event.kind {
            EventKind::AssistantDelta { .. }
            | EventKind::ReasoningDelta { .. } => {}
            _ => self.events.push(event.clone()),
        }
        let _ = self.live.send(event);
    }

    fn head(&self) -> HostSeq {
        self.events.last().map_or(HostSeq(0), |event| event.seq)
    }
}

#[cfg(test)]
mod tests {
    use proto::{
        Auth, Availability, Epoch, EventKind, HostSeq, Models, Providers,
        SessionId, Welcome,
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

    fn providers() -> Providers {
        Providers {
            claude: Availability::Available {
                auth: Auth::SignedIn,
                models: Models::none(),
            },
            codex: Availability::Unavailable {
                reason: String::from("no codex"),
            },
        }
    }

    #[tokio::test]
    async fn deltas_go_live_with_the_last_seq_and_are_never_replayed()
    -> Result<(), Box<dyn std::error::Error>> {
        let log = Log::spawn(String::from("test"), Epoch(5));
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
            log.welcome(providers()).await?,
            Welcome {
                host: String::from("test"),
                epoch: Epoch(5),
                head_seq: HostSeq(2),
                providers: providers(),
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

    use super::{BeginCommand, CommandLease, CommandStart, Log, LogMsg};
    use tokio::sync::oneshot;

    async fn lease(
        log: &Log,
        key: u128,
    ) -> Result<CommandLease, crate::Stopped> {
        match log.begin(CommandId(key)).await? {
            BeginCommand::Execute(lease) => Ok(lease),
            BeginCommand::Cached(_) => Err(crate::Stopped {}),
        }
    }

    #[tokio::test]
    async fn credential_receipts_wait_for_commit_without_holding_the_event_transaction()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp");
        std::fs::create_dir_all(&directory)?;
        let path = directory
            .join(format!("credential-receipts-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let log = Log::open(String::from("test"), &path)?;
        let credential = match log.begin_credential(CommandId(23)).await? {
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
                result: CommandResult::Accepted,
                reply: finished_reply,
            })
            .await?;
        let (retry_reply, retry) = oneshot::channel();
        log.inbox
            .send(LogMsg::Begin(CommandStart {
                key: CommandId(23),
                kind: super::CommandKind::Session,
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
        drop(credential);
        let retried = match retry.await? {
            BeginCommand::Cached(result) => Some(result),
            BeginCommand::Execute(_) => None,
        };
        assert_eq!(retried, Some(CommandResult::Accepted));
        log.shutdown().await?;
        let reopened = Log::open(String::from("test"), &path)?;
        let cached = match reopened.begin_credential(CommandId(23)).await? {
            BeginCommand::Cached(result) => Some(result),
            BeginCommand::Execute(_) => None,
        };
        assert_eq!(cached, Some(CommandResult::Accepted));
        reopened.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn credential_lease_drop_fails_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        let log = Log::spawn(String::from("test"), proto::Epoch(1));
        let credential = match log.begin_credential(CommandId(23)).await? {
            BeginCommand::Execute(lease) => Some(lease),
            BeginCommand::Cached(_) => None,
        }
        .ok_or("expected a value")?;
        drop(credential);
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
                kind: super::CommandKind::Session,
                reply: duplicate_reply,
            }))
            .await?;
        let (next_reply, next) = oneshot::channel();
        log.inbox
            .send(LogMsg::Begin(CommandStart {
                key: CommandId(2),
                kind: super::CommandKind::Session,
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
        let cached = match reopened.begin(CommandId(1)).await? {
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
