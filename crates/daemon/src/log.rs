use crate::storage::{self, Storage};
use redb::WriteTransaction;
use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::path::Path;

use home::Home;
use host::log::{Effect as Logged, FreshRequest};
use host::log::{Event, HostSeq};
use host::registry::{Catalog, Instances};
use host::session::{EventKind, SessionId};
use link::CommandId;
use link::log::{Begun, Completion, Effect, Kind, Lease, Log as Core};
use link::{CommandResult, Subscribe, Welcome};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::Stopped;

const LIVE_BUFFER: usize = 1024;

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
    RunningSessions(oneshot::Sender<std::collections::BTreeSet<SessionId>>),
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
    Follow(Follow),
}

enum Follow {
    Requests(oneshot::Sender<broadcast::Receiver<FreshRequest>>),
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
    transaction: Option<WriteTransaction>,
    storage: Option<Storage>,
    begins: HashMap<Lease, oneshot::Sender<BeginCommand>>,
    finishes: HashMap<Lease, oneshot::Sender<()>>,
    live: broadcast::Sender<Event>,
    requests: broadcast::Sender<FreshRequest>,
}

impl Drop for LogActor {
    fn drop(&mut self) {
        drop(self.transaction.take());
    }
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
    pub fn spawn(name: String, epoch: host::log::Epoch) -> Self {
        Self::start(Core::new(name, epoch, Vec::new()), None)
    }

    pub fn open(name: String, path: &Path) -> anyhow::Result<Self> {
        let storage = Storage::open(path)?;
        let core = Core::new(name, storage.epoch()?, storage.events()?);
        Ok(Self::start(core, Some(storage)))
    }

    fn start(core: Core, storage: Option<Storage>) -> Self {
        let (inbox, messages) = mpsc::channel(64);
        let (live, _) = broadcast::channel(LIVE_BUFFER);
        let (requests, _) = broadcast::channel(LIVE_BUFFER);
        let actor = LogActor {
            inbox: inbox.downgrade(),
            core,
            transaction: None,
            storage,
            begins: HashMap::new(),
            finishes: HashMap::new(),
            live,
            requests,
        };
        tokio::spawn(actor.run(messages));
        Self { inbox }
    }

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

    pub(crate) async fn barrier(&self) -> Result<(), Stopped> {
        self.ask(|reply| LogMsg::Barrier { reply }).await
    }

    pub(crate) async fn begin(
        &self,
        key: CommandId,
        kind: Kind,
    ) -> Result<BeginCommand, Stopped> {
        self.ask(|reply| LogMsg::Begin(CommandStart { key, kind, reply }))
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

    pub(crate) async fn running_sessions(
        &self,
    ) -> Result<std::collections::BTreeSet<SessionId>, Stopped> {
        self.ask(LogMsg::RunningSessions).await
    }

    pub(crate) async fn finish_history(
        &self,
        session: SessionId,
    ) -> Result<Vec<Event>, Stopped> {
        self.ask(|reply| LogMsg::CurrentHistory { session, reply })
            .await
    }

    pub(crate) async fn saved_conversation(
        &self,
        session: SessionId,
    ) -> Result<Option<String>, Stopped> {
        self.ask(|reply| LogMsg::SavedConversation { session, reply })
            .await
    }

    pub async fn stopped(&self) {
        self.inbox.closed().await;
    }

    pub async fn shutdown(&self) -> Result<(), Stopped> {
        self.ask(|reply| LogMsg::Shutdown { reply }).await
    }

    pub async fn welcome(
        &self,
        instances: Instances,
        catalog: Catalog,
        home: Option<Home>,
    ) -> Result<Welcome, Stopped> {
        self.ask(|reply| LogMsg::Welcome {
            instances,
            catalog,
            home,
            reply,
        })
        .await
    }

    pub(crate) async fn follow_requests(
        &self,
    ) -> Result<broadcast::Receiver<FreshRequest>, Stopped> {
        self.ask(|reply| LogMsg::Follow(Follow::Requests(reply)))
            .await
    }

    pub async fn subscribe(
        &self,
        after: HostSeq,
    ) -> Result<Subscription, Stopped> {
        self.ask(|reply| LogMsg::Follow(Follow::Subscribe { after, reply }))
            .await
    }

    pub async fn resume(
        &self,
        subscribe: Subscribe,
    ) -> Result<Option<Subscription>, Stopped> {
        self.ask(|reply| LogMsg::Follow(Follow::Resume { subscribe, reply }))
            .await
    }

    async fn ask<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> LogMsg,
    ) -> Result<T, Stopped> {
        crate::ask(&self.inbox, build).await
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
                    drop(self);
                    drop(messages);
                    let _ = reply.send(());
                    return Err(Stopped {});
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
                let (lease, effects) = self.core.begin(start.key, start.kind);
                self.begins.insert(lease, start.reply);
                self.perform(effects)?;
            }
            LogMsg::Finish {
                key,
                completion,
                reply,
            } => {
                let (lease, effects) = self.core.finish(key, completion);
                self.finishes.insert(lease, reply);
                self.perform(effects)?;
            }
            LogMsg::History { reply } => {
                let _ = reply.send(self.core.history().to_vec());
            }
            LogMsg::RunningSessions(reply) => {
                let _ = reply.send(self.core.running_sessions());
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
            LogMsg::Follow(follow) => self.follow(follow),
        }
        Ok(())
    }

    fn follow(&self, follow: Follow) {
        match follow {
            Follow::Requests(reply) => {
                let _ = reply.send(self.requests.subscribe());
            }
            Follow::Subscribe { after, reply } => {
                let _ = reply.send(self.with_live(self.core.replay(after)));
            }
            Follow::Resume { subscribe, reply } => {
                let resumed = self.core.resume(&subscribe);
                let _ =
                    reply.send(resumed.map(|events| self.with_live(events)));
            }
        }
    }

    fn with_live(&self, replay: Vec<Event>) -> Subscription {
        Subscription {
            live: self.live.subscribe(),
            replay,
        }
    }

    fn perform(&mut self, effects: Vec<Effect>) -> anyhow::Result<()> {
        let mut pending = VecDeque::from(effects);
        while let Some(effect) = pending.pop_front() {
            self.effect(effect, &mut pending)?;
        }
        Ok(())
    }

    fn effect(
        &mut self,
        effect: Effect,
        pending: &mut VecDeque<Effect>,
    ) -> anyhow::Result<()> {
        match effect {
            Effect::Log(logged) => self.logged(logged)?,
            Effect::Receipt(key, result) => {
                let _ = self
                    .transaction
                    .as_ref()
                    .map(|transaction| {
                        storage::store_receipt(transaction, key, &result)
                    })
                    .transpose()?;
            }
            Effect::Lookup(key) => {
                let found = self
                    .storage
                    .as_ref()
                    .map(|storage| storage.receipt(key))
                    .transpose()?
                    .flatten();
                for effect in self.core.receipt(found).into_iter().rev() {
                    pending.push_front(effect);
                }
            }
            Effect::Begun(lease, begun) => self.answer(lease, begun)?,
            Effect::Finished(lease) => {
                let _ =
                    self.finishes.remove(&lease).map(|reply| reply.send(()));
            }
            Effect::Stop => return Err(Stopped {}.into()),
        }
        Ok(())
    }

    fn logged(&mut self, effect: Logged) -> anyhow::Result<()> {
        match effect {
            Logged::Open => {
                self.transaction =
                    self.storage.as_ref().map(Storage::begin).transpose()?;
            }
            Logged::Store(write) => {
                let _ = self
                    .transaction
                    .as_ref()
                    .map(|transaction| storage::write(transaction, &write))
                    .transpose()?;
            }
            Logged::Commit => {
                let _ = self
                    .transaction
                    .take()
                    .map(WriteTransaction::commit)
                    .transpose()?;
            }
            Logged::Publish(event) => {
                let _ = self.live.send(event);
            }
            Logged::RequestOpened(request) => {
                let _ = self.requests.send(request);
            }
        }
        Ok(())
    }

    fn answer(&mut self, lease: Lease, begun: Begun) -> anyhow::Result<()> {
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
        if let Some(reply) = self.begins.remove(&lease) {
            let _ = reply.send(answer);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use agents::Output;
    use agents::{Auth, Models, Provider};
    use host::log::{Epoch, HostSeq};
    use host::registry::{
        Availability, Catalog, InstanceId, InstanceStatus, Instances,
    };
    use host::session::{EventKind, SessionId};

    use super::{Log, Welcome};

    fn delta(text: &str) -> EventKind {
        EventKind::Agent(Output::AssistantDelta {
            text: String::from(text),
        })
    }

    fn message(text: &str) -> EventKind {
        EventKind::Agent(Output::AssistantMessage {
            text: String::from(text),
        })
    }

    fn instances() -> Instances {
        Instances(
            [Provider::new("claude"), Provider::new("codex")]
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
        let log = Log::spawn(String::from("test"), host::log::Epoch(5));
        let mut live = log.subscribe(HostSeq(0)).await?.live;
        log.append(SessionId(1), EventKind::SessionReady).await?;
        log.append(SessionId(1), delta("a")).await?;
        log.append(
            SessionId(1),
            EventKind::Agent(Output::ReasoningDelta {
                text: String::from("thinking"),
            }),
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
                    EventKind::Agent(Output::ReasoningDelta {
                        text: String::from("thinking"),
                    }),
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

    use agents::Output;
    use host::log::HostSeq;
    use host::session::{EventKind, SessionId};
    use link::CommandId;

    use super::{
        BeginCommand, CommandLease, CommandResult, CommandStart, Kind, Log,
        LogMsg,
    };
    use tokio::sync::oneshot;

    async fn lease(
        log: &Log,
        key: u128,
    ) -> Result<CommandLease, crate::Stopped> {
        match log
            .begin(CommandId(uuid::Uuid::from_u128(key)), Kind::Session)
            .await?
        {
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
        let receipt = match log
            .begin(CommandId(uuid::Uuid::from_u128(23)), Kind::Receipt)
            .await?
        {
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
                key: CommandId(uuid::Uuid::from_u128(23)),
                completion: CommandResult::Session(
                    host::session::SessionResult::Accepted,
                )
                .into(),
                reply: finished_reply,
            })
            .await?;
        let (retry_reply, retry) = oneshot::channel();
        log.inbox
            .send(LogMsg::Begin(CommandStart {
                key: CommandId(uuid::Uuid::from_u128(23)),
                kind: link::log::Kind::Session,
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
        ordinary
            .finish(CommandResult::Session(
                host::session::SessionResult::Accepted,
            ))
            .await?;
        finished.await?;
        drop(receipt);
        let retried = match retry.await? {
            BeginCommand::Cached(result) => Some(result),
            BeginCommand::Execute(_) => None,
        };
        assert_eq!(
            retried,
            Some(CommandResult::Session(
                host::session::SessionResult::Accepted
            ))
        );
        log.shutdown().await?;
        let reopened = Log::open(String::from("test"), &path)?;
        let cached = match reopened
            .begin(CommandId(uuid::Uuid::from_u128(23)), Kind::Receipt)
            .await?
        {
            BeginCommand::Cached(result) => Some(result),
            BeginCommand::Execute(_) => None,
        };
        assert_eq!(
            cached,
            Some(CommandResult::Session(
                host::session::SessionResult::Accepted
            ))
        );
        reopened.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn shutdown_releases_storage_with_an_open_transaction()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp");
        std::fs::create_dir_all(&directory)?;
        let path =
            directory.join(format!("shutdown-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let log = Log::open(String::from("test"), &path)?;
        let running = lease(&log, 1).await?;
        log.append(SessionId(1), EventKind::SessionReady).await?;
        log.shutdown().await?;
        let reopened = Log::open(String::from("test"), &path)?;
        assert_eq!(reopened.history().await?, Vec::new());
        drop(running);
        reopened.shutdown().await?;
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[tokio::test]
    async fn queued_cached_receipts_drain_in_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp");
        std::fs::create_dir_all(&directory)?;
        let path = directory
            .join(format!("receipt-queue-{}.redb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let storage = super::Storage::open(&path)?;
        let transaction = storage.begin()?;
        for id in 1..=64 {
            super::storage::store_receipt(
                &transaction,
                CommandId(uuid::Uuid::from_u128(id)),
                &CommandResult::Session(host::session::SessionResult::Accepted),
            )?;
        }
        transaction.commit()?;
        drop(storage);
        let log = Log::open(String::from("test"), &path)?;
        let running = lease(&log, 0).await?;
        let mut queued = Vec::new();
        for id in 1..=64 {
            let (reply, answer) = oneshot::channel();
            log.inbox
                .send(LogMsg::Begin(CommandStart {
                    key: CommandId(uuid::Uuid::from_u128(id)),
                    kind: link::log::Kind::Session,
                    reply,
                }))
                .await?;
            queued.push(answer);
        }
        let (reply, cache_miss) = oneshot::channel();
        log.inbox
            .send(LogMsg::Begin(CommandStart {
                key: CommandId(uuid::Uuid::from_u128(65)),
                kind: link::log::Kind::Session,
                reply,
            }))
            .await?;
        running
            .finish(CommandResult::Session(
                host::session::SessionResult::Accepted,
            ))
            .await?;
        let BeginCommand::Execute(last) = cache_miss.await? else {
            return Err("expected the cache miss to execute".into());
        };
        for mut answer in queued {
            assert!(matches!(
                answer.try_recv()?,
                BeginCommand::Cached(CommandResult::Session(
                    host::session::SessionResult::Accepted
                ))
            ));
        }
        last.finish(CommandResult::Session(
            host::session::SessionResult::Accepted,
        ))
        .await?;
        log.shutdown().await?;
        std::fs::remove_file(path)?;
        Ok(())
    }

    #[tokio::test]
    async fn receipt_lease_drop_fails_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        let log = Log::spawn(String::from("test"), host::log::Epoch(1));
        let receipt = match log
            .begin(CommandId(uuid::Uuid::from_u128(23)), Kind::Receipt)
            .await?
        {
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
            EventKind::Agent(Output::AssistantDelta {
                text: String::from("a"),
            }),
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
                key: CommandId(uuid::Uuid::from_u128(1)),
                kind: link::log::Kind::Session,
                reply: duplicate_reply,
            }))
            .await?;
        let (next_reply, next) = oneshot::channel();
        log.inbox
            .send(LogMsg::Begin(CommandStart {
                key: CommandId(uuid::Uuid::from_u128(2)),
                kind: link::log::Kind::Session,
                reply: next_reply,
            }))
            .await?;
        first
            .finish(CommandResult::Session(
                host::session::SessionResult::Accepted,
            ))
            .await?;
        assert_eq!(live.recv().await?.kind, EventKind::SessionReady);
        assert_eq!(live.recv().await?.seq, HostSeq(1));
        let cached = match duplicate.await? {
            BeginCommand::Cached(result) => Some(result),
            BeginCommand::Execute(_) => None,
        };
        assert_eq!(
            cached,
            Some(CommandResult::Session(
                host::session::SessionResult::Accepted
            ))
        );
        let second = match next.await? {
            BeginCommand::Execute(lease) => Some(lease),
            BeginCommand::Cached(_) => None,
        }
        .ok_or("expected a value")?;
        second
            .finish(CommandResult::Session(
                host::session::SessionResult::Accepted,
            ))
            .await?;
        log.shutdown().await?;
        let reopened = Log::open(String::from("test"), &path)?;
        assert_eq!(reopened.subscribe(HostSeq(0)).await?.replay.len(), 1);
        let cached = match reopened
            .begin(CommandId(uuid::Uuid::from_u128(1)), Kind::Session)
            .await?
        {
            BeginCommand::Cached(result) => Some(result),
            BeginCommand::Execute(_) => None,
        };
        assert_eq!(
            cached,
            Some(CommandResult::Session(
                host::session::SessionResult::Accepted
            ))
        );
        reopened.shutdown().await?;
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_capture_preserves_the_turn_start_but_rolls_back_its_suffix()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!(
            "conductor-capture-barrier-{}.redb",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let log = Log::open(String::from("test"), &path)?;
        let command = lease(&log, 9).await?;
        let started = EventKind::NativeTurnStarted {
            turn_start: Some(HostSeq(4)),
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
            EventKind::SnapshotRecorded {
                snapshot: host::workspace::Snapshot {
                    turn_start: Some(HostSeq(4)),
                    commit: String::from("captured"),
                },
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
        retried
            .finish(CommandResult::Session(
                host::session::SessionResult::Accepted,
            ))
            .await?;
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
        retried
            .finish(CommandResult::Session(
                host::session::SessionResult::Accepted,
            ))
            .await?;
        reopened.shutdown().await?;
        let _ = std::fs::remove_file(path);
        Ok(())
    }
}
