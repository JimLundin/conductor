//! The host's event log. One task owns it and assigns every stored event its
//! seq; [`Log`] handles talk to it over a channel, so nothing is shared
//! behind a lock.

use crate::storage::{self, Storage};
use redb::WriteTransaction;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::path::Path;

use proto::{
    CommandId, CommandResult, Epoch, Event, EventKind, HostSeq, Providers, SessionId, Welcome,
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
    reply: oneshot::Sender<BeginCommand>,
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
    pub(crate) async fn finish(self, result: CommandResult) -> Result<(), Stopped> {
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
    name: String,
    epoch: Epoch,
    events: Vec<Event>,
    live: broadcast::Sender<Event>,
}

impl Log {
    pub fn spawn(name: String, epoch: Epoch) -> Log {
        Self::start(name, epoch, Vec::new(), None)
    }

    pub fn open(name: String, path: &Path) -> anyhow::Result<Log> {
        let storage = Storage::open(path)?;
        let epoch = storage.epoch()?;
        let events = storage.events()?;
        Ok(Self::start(name, epoch, events, Some(storage)))
    }

    fn start(name: String, epoch: Epoch, events: Vec<Event>, storage: Option<Storage>) -> Log {
        let (inbox, messages) = mpsc::channel(64);
        let (live, _) = broadcast::channel(LIVE_BUFFER);
        let actor = LogActor {
            inbox: inbox.downgrade(),
            active: None,
            storage,
            transaction: None,
            staged: Vec::new(),
            waiting: VecDeque::new(),
            name,
            epoch,
            events,
            live,
        };
        tokio::spawn(actor.run(messages));
        Log { inbox }
    }

    /// Acknowledges after the durable write, or after staging in the active
    /// command's transaction. The command receipt commits its staged events.
    pub async fn append(&self, session: SessionId, kind: EventKind) -> Result<(), Stopped> {
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

    pub(crate) async fn begin(&self, key: CommandId) -> Result<BeginCommand, Stopped> {
        self.ask(|reply| LogMsg::Begin(CommandStart { key, reply }))
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

    pub(crate) async fn conversation(&self, session: SessionId, id: String) -> Result<(), Stopped> {
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
    pub async fn shutdown(&self) -> Result<(), Stopped> {
        self.ask(|reply| LogMsg::Shutdown { reply }).await
    }

    /// The log's head as of now, for a client the registry says can run
    /// `providers`.
    pub async fn welcome(&self, providers: Providers) -> Result<Welcome, Stopped> {
        self.ask(|reply| LogMsg::Welcome { providers, reply }).await
    }

    pub async fn subscribe(&self, after: HostSeq) -> Result<Subscription, Stopped> {
        self.ask(|reply| LogMsg::Subscribe { after, reply }).await
    }

    async fn ask<T>(&self, build: impl FnOnce(oneshot::Sender<T>) -> LogMsg) -> Result<T, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.inbox
            .send(build(reply))
            .await
            .map_err(|_| Stopped {})?;
        answer.await.map_err(|_| Stopped {})
    }
}

impl LogActor {
    async fn run(mut self, mut messages: mpsc::Receiver<LogMsg>) -> Result<Infallible, Stopped> {
        loop {
            let msg = messages.recv().await.ok_or(Stopped {})?;
            match msg {
                LogMsg::Shutdown { reply } => {
                    let _ = reply.send(());
                    break Err(Stopped {});
                }
                msg => self.on(msg).map_err(|err| {
                    tracing::error!(%err, "durable log stopped");
                    Stopped {}
                })?,
            }
        }
    }

    fn on(&mut self, msg: LogMsg) -> anyhow::Result<()> {
        match msg {
            LogMsg::Barrier { reply } => {
                self.commit()?;
                if let Some(active) = &self.active {
                    let _ = active;
                    self.transaction = self.storage.as_ref().map(Storage::begin).transpose()?;
                }
                let _ = reply.send(());
            }
            LogMsg::Shutdown { reply } => {
                let _ = reply.send(());
            }
            LogMsg::Abort(key) => match &self.active {
                Some(active) if active == &key => {
                    Err(Stopped {})?;
                }
                _ => {}
            },
            LogMsg::Begin(start) => match &self.active {
                Some(_) => self.waiting.push_back(start),
                None => self.begin(start)?,
            },
            LogMsg::Finish { key, result, reply } => {
                if let Some(write) = self.transaction.take() {
                    storage::receipt(&write, key, &result)?;
                    self.transaction = Some(write);
                }
                self.commit()?;
                self.active = None;
                let _ = reply.send(());
                if let Some(start) = self.waiting.pop_front() {
                    self.begin(start)?;
                }
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
                match (&self.storage, &self.transaction) {
                    (_, Some(write)) => storage::conversation(write, session, &id)?,
                    (Some(storage), None) => {
                        let write = storage.begin()?;
                        storage::conversation(&write, session, &id)?;
                        write.commit()?;
                    }
                    (None, None) => {}
                }
                let _ = reply.send(());
            }
            // Live only, so it takes no seq (see `EventKind::AssistantDelta`).
            LogMsg::Append {
                session,
                kind: kind @ (EventKind::AssistantDelta { .. } | EventKind::ReasoningDelta { .. }),
                reply,
            } => {
                let event = Event {
                    seq: self.staged.last().map_or(self.head(), |event| event.seq),
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
                let _ = reply.send(recorded);
            }
            LogMsg::Append {
                session,
                kind,
                reply,
            } => {
                let event = Event {
                    seq: self
                        .staged
                        .last()
                        .map_or(self.head(), |event| event.seq)
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
                let _ = reply.send(recorded);
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

    fn begin(&mut self, start: CommandStart) -> anyhow::Result<()> {
        let receipt = self
            .storage
            .as_ref()
            .map(|storage| storage.receipt(start.key))
            .transpose()?
            .flatten();
        match receipt {
            Some(result) => {
                let _ = start.reply.send(BeginCommand::Cached(result));
                if let Some(next) = self.waiting.pop_front() {
                    self.begin(next)?;
                }
            }
            None => {
                self.transaction = self.storage.as_ref().map(Storage::begin).transpose()?;
                self.active = Some(start.key);
                let inbox = self.inbox.upgrade().ok_or(Stopped {})?;
                let lease = CommandLease {
                    log: Log { inbox },
                    key: start.key,
                };
                let _ = start.reply.send(BeginCommand::Execute(lease));
            }
        }
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
            EventKind::AssistantDelta { .. } | EventKind::ReasoningDelta { .. } => {}
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
        Auth, Availability, Epoch, EventKind, HostSeq, Models, Providers, SessionId, Welcome,
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
    async fn deltas_go_live_with_the_last_seq_and_are_never_replayed() {
        let log = Log::spawn(String::from("test"), Epoch(5));
        let mut live = log.subscribe(HostSeq(0)).await.unwrap().live;
        log.append(SessionId(1), EventKind::SessionReady)
            .await
            .unwrap();
        log.append(SessionId(1), delta("a")).await.unwrap();
        log.append(
            SessionId(1),
            EventKind::ReasoningDelta {
                text: String::from("thinking"),
            },
        )
        .await
        .unwrap();
        log.append(SessionId(1), message("a")).await.unwrap();
        assert_eq!(
            log.welcome(providers()).await.unwrap(),
            Welcome {
                host: String::from("test"),
                epoch: Epoch(5),
                head_seq: HostSeq(2),
                providers: providers(),
            }
        );
        let seen = [
            live.recv().await.unwrap(),
            live.recv().await.unwrap(),
            live.recv().await.unwrap(),
            live.recv().await.unwrap(),
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
        let replay = log.subscribe(HostSeq(1)).await.unwrap().replay;
        assert_eq!(
            replay
                .into_iter()
                .map(|event| (event.seq, event.kind))
                .collect::<Vec<(HostSeq, EventKind)>>(),
            [(HostSeq(2), message("a"))]
        );
    }
}

#[cfg(test)]
mod durability_tests {
    use std::time::Duration;

    use proto::{CommandId, CommandResult, EventKind, HostSeq, SessionId};

    use super::{BeginCommand, CommandLease, CommandStart, Log, LogMsg};
    use tokio::sync::oneshot;

    async fn lease(log: &Log, key: u128) -> Result<CommandLease, crate::Stopped> {
        match log.begin(CommandId(key)).await? {
            BeginCommand::Execute(lease) => Ok(lease),
            BeginCommand::Cached(_) => Err(crate::Stopped {}),
        }
    }

    #[tokio::test]
    async fn receipt_and_events_commit_before_live_and_queued_retries_drain() {
        let path =
            std::env::temp_dir().join(format!("conductor-staging-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let log = Log::open(String::from("test"), &path).unwrap();
        let first = lease(&log, 1).await.unwrap();
        let super::Subscription { mut live, .. } = log.subscribe(HostSeq(0)).await.unwrap();
        log.append(SessionId(1), EventKind::SessionReady)
            .await
            .unwrap();
        log.append(
            SessionId(1),
            EventKind::AssistantDelta {
                text: String::from("a"),
            },
        )
        .await
        .unwrap();
        assert_eq!(log.subscribe(HostSeq(0)).await.unwrap().replay, Vec::new());
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
                reply: duplicate_reply,
            }))
            .await
            .unwrap();
        let (next_reply, next) = oneshot::channel();
        log.inbox
            .send(LogMsg::Begin(CommandStart {
                key: CommandId(2),
                reply: next_reply,
            }))
            .await
            .unwrap();
        first.finish(CommandResult::Accepted).await.unwrap();
        assert_eq!(live.recv().await.unwrap().kind, EventKind::SessionReady);
        assert_eq!(live.recv().await.unwrap().seq, HostSeq(1));
        let cached = match duplicate.await.unwrap() {
            BeginCommand::Cached(result) => Some(result),
            BeginCommand::Execute(_) => None,
        };
        assert_eq!(cached, Some(CommandResult::Accepted));
        let second = match next.await.unwrap() {
            BeginCommand::Execute(lease) => Some(lease),
            BeginCommand::Cached(_) => None,
        }
        .unwrap();
        second.finish(CommandResult::Accepted).await.unwrap();
        log.shutdown().await.unwrap();
        let reopened = Log::open(String::from("test"), &path).unwrap();
        assert_eq!(
            reopened.subscribe(HostSeq(0)).await.unwrap().replay.len(),
            1
        );
        let cached = match reopened.begin(CommandId(1)).await.unwrap() {
            BeginCommand::Cached(result) => Some(result),
            BeginCommand::Execute(_) => None,
        };
        assert_eq!(cached, Some(CommandResult::Accepted));
        reopened.shutdown().await.unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn cancelled_undo_preserves_committed_intent_but_rolls_back_its_suffix() {
        let path = std::env::temp_dir().join(format!(
            "conductor-undo-barrier-{}.redb",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let log = Log::open(String::from("test"), &path).unwrap();
        let command = lease(&log, 9).await.unwrap();
        let started = EventKind::UndoStarted {
            checkpoint: proto::CheckpointId(2),
            restore: proto::CheckpointId(1),
            history: proto::HistoryAnchor::Fresh,
        };
        log.append(SessionId(1), started.clone()).await.unwrap();
        log.barrier().await.unwrap();
        assert_eq!(
            log.subscribe(HostSeq(0))
                .await
                .unwrap()
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
        .await
        .unwrap();
        let inbox = log.inbox.clone();
        drop(command);
        tokio::time::timeout(Duration::from_secs(2), inbox.closed())
            .await
            .unwrap();
        let reopened = Log::open(String::from("test"), &path).unwrap();
        assert_eq!(
            reopened
                .history()
                .await
                .unwrap()
                .into_iter()
                .map(|event| event.kind)
                .collect::<Vec<EventKind>>(),
            Vec::from([started])
        );
        let retried = lease(&reopened, 9).await.unwrap();
        retried.finish(CommandResult::Accepted).await.unwrap();
        reopened.shutdown().await.unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn dropping_an_unfinished_command_rolls_back_and_fails_closed() {
        let path =
            std::env::temp_dir().join(format!("conductor-abort-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let log = Log::open(String::from("test"), &path).unwrap();
        let command = lease(&log, 1).await.unwrap();
        log.append(SessionId(1), EventKind::SessionReady)
            .await
            .unwrap();
        let inbox = log.inbox.clone();
        drop(command);
        tokio::time::timeout(Duration::from_secs(2), inbox.closed())
            .await
            .unwrap();
        assert_eq!(
            log.history().await.err().map(|_| String::from("stopped")),
            Some(String::from("stopped"))
        );
        let reopened = Log::open(String::from("test"), &path).unwrap();
        assert_eq!(reopened.history().await.unwrap(), Vec::new());
        let retried = lease(&reopened, 1).await.unwrap();
        retried.finish(CommandResult::Accepted).await.unwrap();
        reopened.shutdown().await.unwrap();
        let _ = std::fs::remove_file(path);
    }
}
