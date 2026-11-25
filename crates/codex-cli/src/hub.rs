use std::convert::Infallible;
use std::sync::Arc;

use agents::Models;
use codex::CodexCredential;
use codex::hub::{Ask, Command, EarlyEvents, Overflowed, Routed};
use codex::{CodexEvent, ThreadId, ThreadOptions};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;

use super::server::{CodexServer, CodexThread, Error, ServerOptions};

pub type Thread = (CodexThread, mpsc::Receiver<Routed>);

struct Asked {
    options: ThreadOptions,
    resume: Option<ThreadId>,
    reply: oneshot::Sender<Result<Thread, Error>>,
}

type Listed = oneshot::Sender<Result<Models, Error>>;
type Configured = oneshot::Sender<Result<(), Error>>;
const ROUTE_BACKLOG: usize = 256;
const OVERFLOW: &str = "the Codex session event backlog was exceeded";

#[derive(Clone)]
struct Route {
    events: mpsc::Sender<Routed>,
    terminal: watch::Sender<Option<Routed>>,
    generation: u64,
}

impl Route {
    fn send(&self, routed: Routed) -> bool {
        match self.events.try_send(routed) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Closed(_)) => false,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.close(Routed::Stopped {
                    reason: String::from(OVERFLOW),
                });
                false
            }
        }
    }

    fn close(&self, routed: Routed) {
        self.terminal.send_replace(Some(routed));
    }
}

struct Completed {
    reply: oneshot::Sender<Result<Thread, Error>>,
    outcome: Result<CodexThread, Error>,
}
type Machine = codex::hub::Hub<Asked, Listed, Route>;
type Input = codex::hub::Input<Asked, Listed, Configured, Route, Arc<Error>>;
type Effect = codex::hub::Effect<Asked, Listed, Configured, Route, Arc<Error>>;
type Msg = Command<Asked, Listed, Configured>;

#[derive(Debug, Clone)]
pub struct Hub {
    inbox: mpsc::Sender<Msg>,
}

impl Hub {
    #[must_use]
    pub fn spawn(options: ServerOptions) -> Self {
        let (inbox, commands) = mpsc::channel(16);
        tokio::spawn(Task::new(options).run(commands));
        Self { inbox }
    }

    pub async fn thread(
        &self,
        options: ThreadOptions,
        resume: Option<ThreadId>,
    ) -> Result<Thread, Error> {
        self.ask(|reply| {
            Command::Thread(Asked {
                options,
                resume,
                reply,
            })
        })
        .await?
    }

    pub async fn models(&self) -> Result<Models, Error> {
        self.ask(Command::Models).await?
    }

    pub async fn set_credential(
        &self,
        credential: Option<CodexCredential>,
    ) -> Result<(), Error> {
        self.ask(|reply| Command::Credential { credential, reply })
            .await?
    }

    async fn ask<T>(
        &self,
        build: impl FnOnce(oneshot::Sender<T>) -> Msg,
    ) -> Result<T, Error> {
        let (reply, answer) = oneshot::channel();
        self.inbox
            .send(build(reply))
            .await
            .map_err(|_| Error::HubStopped)?;
        answer.await.map_err(|_| Error::HubStopped)
    }
}

struct Up {
    server: CodexServer,
    events: mpsc::Receiver<CodexEvent>,
}

struct Ended;

struct Task {
    machine: Machine,
    options: ServerOptions,
    server: Option<Up>,
    opens: JoinSet<Completed>,
    lists: JoinSet<()>,
    relays: JoinSet<(ThreadId, u64)>,
    early: EarlyEvents,
    generation: u64,
}

impl Task {
    fn new(options: ServerOptions) -> Self {
        Self {
            machine: Machine::new(options.credential.clone()),
            options,
            server: None,
            opens: JoinSet::new(),
            lists: JoinSet::new(),
            relays: JoinSet::new(),
            early: EarlyEvents::default(),
            generation: 0,
        }
    }

    async fn run(
        mut self,
        mut commands: mpsc::Receiver<Msg>,
    ) -> Result<Infallible, Ended> {
        loop {
            let input = tokio::select! {
                command = commands.recv() => command.map_or(Input::Closed, Input::Command),
                event = next_event_or_pending(self.server.as_mut()) => Input::Event(event),
                completed = self.opens.join_next(), if !self.opens.is_empty() => {
                    self.opened(completed).await?;
                    continue;
                }
                _ = self.lists.join_next(), if !self.lists.is_empty() => continue,
                gone = self.relays.join_next(), if !self.relays.is_empty() => {
                    let Some(Ok((thread, generation))) = gone else { continue };
                    Input::Gone { thread, generation }
                }
            };
            self.apply_input_depth_first(input).await?;
        }
    }

    async fn apply_input_depth_first(
        &mut self,
        input: Input,
    ) -> Result<(), Ended> {
        let mut effect_stack = self.machine.step(input);
        effect_stack.reverse();
        while let Some(effect) = effect_stack.pop() {
            let followups = self.perform(effect).await?;
            effect_stack.extend(followups.into_iter().rev());
        }
        Ok(())
    }

    async fn perform(&mut self, effect: Effect) -> Result<Vec<Effect>, Ended> {
        match effect {
            Effect::Spawn(credential) => {
                return Ok(self.spawn(credential, CodexServer::spawn).await);
            }
            Effect::SpawnConfigured(credential) => {
                return Ok(self.spawn(credential, CodexServer::open).await);
            }
            Effect::Open(asked) => self.open(asked),
            Effect::List(reply) => self.list_models(reply),
            Effect::Fail(Ask::Thread(asked), error) => {
                let _ = asked.reply.send(Err(Error::Start(error)));
            }
            Effect::Fail(Ask::Models(reply), error) => {
                let _ = reply.send(Err(Error::Start(error)));
            }
            Effect::Route {
                thread,
                route,
                routed,
            } => {
                if !route.send(routed) {
                    return Ok(self.machine.step(Input::Gone {
                        thread,
                        generation: route.generation,
                    }));
                }
            }
            Effect::Close { route, routed } => route.close(routed),
            Effect::Stop => self.stop().await,
            Effect::Lost(status) => {
                tracing::warn!(status = %status.describe(), "the codex app-server exited");
                self.server = None;
                self.clear_opening();
            }
            Effect::Unrouted(event) => self.unrouted(event),
            Effect::Configured(reply) => {
                let applied = crate::config::write(
                    &self.options.home,
                    self.machine.credential(),
                );
                let more = self.machine.step(Input::Applied(applied.is_ok()));
                let _ = reply.send(applied);
                return Ok(more);
            }
            Effect::End => return Err(Ended),
        }
        Ok(Vec::new())
    }

    fn list_models(&mut self, reply: Listed) {
        let server = self.running();
        self.lists.spawn(async move {
            let listed = match server {
                Ok(running) => running.list_models().await,
                Err(error) => Err(error),
            };
            let _ = reply.send(listed);
        });
    }

    fn unrouted(&mut self, event: CodexEvent) {
        if let Some(unhandled) =
            self.early.capture(event, !self.opens.is_empty())
        {
            let (kind, thread) = unhandled.diagnostic();
            tracing::debug!(
                event = kind,
                thread = thread.map(|id| id.0.as_str()),
                "a codex event for no thread"
            );
        }
    }

    async fn spawn(
        &mut self,
        credential: Option<CodexCredential>,
        launch: impl AsyncFnOnce(
            ServerOptions,
        ) -> Result<
            (CodexServer, mpsc::Receiver<CodexEvent>),
            Error,
        >,
    ) -> Vec<Effect> {
        let options = ServerOptions {
            credential,
            ..self.options.clone()
        };
        let spawned = launch(options).await.map(|(server, events)| {
            tracing::info!(
                version = server.codex_version().unwrap_or("?"),
                "codex app-server started"
            );
            self.server = Some(Up { server, events });
        });
        self.machine.step(Input::Spawned(spawned.map_err(Arc::new)))
    }

    fn open(&mut self, asked: Asked) {
        let Asked {
            options,
            resume,
            reply,
        } = asked;
        if reply.is_closed() {
            return;
        }
        let server = self.running();
        self.opens.spawn(async move {
            let outcome = match server {
                Ok(running) => running.open_thread(options, resume).await,
                Err(error) => Err(error),
            };
            Completed { reply, outcome }
        });
    }

    async fn opened(
        &mut self,
        completed: Option<Result<Completed, tokio::task::JoinError>>,
    ) -> Result<(), Ended> {
        let route = completed.and_then(Result::ok).and_then(
            |Completed { reply, outcome }| {
                connect_open(outcome, reply, &mut self.early)
            },
        );
        if let Some((id, sender, early)) = route {
            self.install_route(id, sender, early).await?;
        }
        if self.opens.is_empty() {
            self.early.settle();
        }
        Ok(())
    }

    async fn install_route(
        &mut self,
        thread: ThreadId,
        sender: mpsc::Sender<Routed>,
        early: Vec<CodexEvent>,
    ) -> Result<(), Ended> {
        let (events, queued) = mpsc::channel(ROUTE_BACKLOG);
        let (terminal, closing) = watch::channel(None);
        self.generation += 1;
        let generation = self.generation;
        let route = Route {
            events,
            terminal,
            generation,
        };
        self.relays.spawn(relay(
            thread.clone(),
            generation,
            queued,
            closing,
            sender,
        ));
        self.apply_input_depth_first(Input::Opened {
            thread: thread.clone(),
            route: route.clone(),
            generation,
        })
        .await?;
        for event in early {
            if !route.send(Routed::Codex(event)) {
                self.apply_input_depth_first(Input::Gone {
                    thread,
                    generation,
                })
                .await?;
                break;
            }
        }
        Ok(())
    }

    fn clear_opening(&mut self) {
        self.opens = JoinSet::new();
        self.lists = JoinSet::new();
        self.early.settle();
    }

    async fn stop(&mut self) {
        self.clear_opening();
        if let Some(up) = self.server.take() {
            let stopped = up.server.shutdown().await;
            tracing::info!(
                ?stopped,
                "stopped the codex app-server for a new credential"
            );
        }
    }

    fn running(&self) -> Result<CodexServer, Error> {
        self.server
            .as_ref()
            .map(|up| up.server.clone())
            .ok_or(Error::Exited)
    }
}

fn connect_open(
    outcome: Result<CodexThread, Error>,
    reply: oneshot::Sender<Result<Thread, Error>>,
    early: &mut EarlyEvents,
) -> Option<(ThreadId, mpsc::Sender<Routed>, Vec<CodexEvent>)> {
    let opened = outcome.and_then(|thread| {
        early
            .open(&thread.id)
            .map(|events| (thread, events))
            .map_err(|Overflowed| Error::EventBacklog)
    });
    match opened {
        Ok((thread, events)) => {
            let id = thread.id.clone();
            connect(thread, reply).map(|sender| (id, sender, events))
        }
        Err(error) => {
            let _ = reply.send(Err(error));
            None
        }
    }
}

fn connect<T>(
    thread: T,
    reply: oneshot::Sender<Result<(T, mpsc::Receiver<Routed>), Error>>,
) -> Option<mpsc::Sender<Routed>> {
    let (sender, events) = mpsc::channel(256);
    reply.send(Ok((thread, events))).ok().map(|()| sender)
}

async fn relay(
    thread: ThreadId,
    generation: u64,
    queued: mpsc::Receiver<Routed>,
    closing: watch::Receiver<Option<Routed>>,
    sender: mpsc::Sender<Routed>,
) -> (ThreadId, u64) {
    let _ = forward(queued, closing, sender).await;
    (thread, generation)
}

struct ReceiverClosed;

async fn forward(
    mut queued: mpsc::Receiver<Routed>,
    mut closing: watch::Receiver<Option<Routed>>,
    sender: mpsc::Sender<Routed>,
) -> Result<(), ReceiverClosed> {
    let mut watch_open = true;
    loop {
        let next = tokio::select! {
            () = sender.closed() => return Ok(()),
            _ = closing.changed(), if watch_open => {
                watch_open = false;
                queued.close();
                continue;
            }
            routed = queued.recv() => routed,
        };
        let Some(routed) = next else { break };
        sender.send(routed).await.map_err(|_| ReceiverClosed)?;
    }
    let last = closing.borrow_and_update().clone();
    if let Some(terminal) = last {
        sender.send(terminal).await.map_err(|_| ReceiverClosed)?;
    }
    Ok(())
}

async fn next_event_or_pending(server: Option<&mut Up>) -> Option<CodexEvent> {
    match server {
        Some(up) => up.events.recv().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use codex::{ThreadEvent, TurnId};
    use install::Home;
    use tokio::sync::{mpsc, oneshot, watch};

    use super::{
        Command, Input, OVERFLOW, Route, Routed, ServerOptions, Task, connect,
        relay,
    };
    use codex::{CodexEvent, ThreadId};

    fn task() -> Task {
        let mut task = Task::new(ServerOptions {
            codex_path: "codex".into(),
            home: Home::User,
            credential: None,
            cwd: ".".into(),
        });
        let (reply, _) = oneshot::channel();
        let _: Vec<super::Effect> =
            task.machine.step(Input::Command(Command::Models(reply)));
        let _: Vec<super::Effect> = task.machine.step(Input::Spawned(Ok(())));
        task
    }

    fn event(thread: &ThreadId, turn: &str) -> CodexEvent {
        CodexEvent::Thread {
            thread: thread.clone(),
            event: ThreadEvent::TurnStarted {
                turn: TurnId(String::from(turn)),
            },
        }
    }

    #[tokio::test]
    async fn a_paused_receiver_preserves_order_without_blocking_another_thread()
    -> Result<(), &'static str> {
        let mut task = task();
        let paused = ThreadId(String::from("paused"));
        let active = ThreadId(String::from("active"));
        let (paused_sender, mut paused_receiver) = mpsc::channel(1);
        let (active_sender, mut active_receiver) = mpsc::channel(1);
        task.install_route(paused.clone(), paused_sender, Vec::new())
            .await
            .map_err(|_| "stopped")?;
        task.install_route(active.clone(), active_sender, Vec::new())
            .await
            .map_err(|_| "stopped")?;

        let first = event(&paused, "first");
        let second = event(&paused, "second");
        let other = event(&active, "other");
        for event in [&first, &second, &other] {
            task.apply_input_depth_first(Input::Event(Some(event.clone())))
                .await
                .map_err(|_| "stopped")?;
        }
        assert_eq!(active_receiver.recv().await, Some(Routed::Codex(other)));
        assert_eq!(paused_receiver.recv().await, Some(Routed::Codex(first)));
        assert_eq!(paused_receiver.recv().await, Some(Routed::Codex(second)));
        Ok(())
    }

    #[tokio::test]
    async fn a_full_route_delivers_accepted_events_then_disconnects()
    -> Result<(), &'static str> {
        let mut task = task();
        let thread = ThreadId(String::from("paused"));
        let (events, queued) = mpsc::channel(1);
        let (terminal, closing) = watch::channel(None);
        task.apply_input_depth_first(Input::Opened {
            thread: thread.clone(),
            route: Route {
                events,
                terminal,
                generation: 1,
            },
            generation: 1,
        })
        .await
        .map_err(|_| "stopped")?;

        let accepted = event(&thread, "accepted");
        for event in [accepted.clone(), event(&thread, "over capacity")] {
            task.apply_input_depth_first(Input::Event(Some(event)))
                .await
                .map_err(|_| "stopped")?;
        }
        assert!(matches!(
            task.machine
                .step::<(), ()>(codex::hub::Input::Event(Some(event(
                    &thread, "later"
                ))))
                .as_slice(),
            [codex::hub::Effect::Unrouted(_)]
        ));
        let (sender, mut receiver) = mpsc::channel(1);
        let task =
            tokio::spawn(relay(thread.clone(), 1, queued, closing, sender));
        assert_eq!(receiver.recv().await, Some(Routed::Codex(accepted)));
        assert_eq!(
            receiver.recv().await,
            Some(Routed::Stopped {
                reason: String::from(OVERFLOW)
            })
        );
        assert!(receiver.recv().await.is_none());
        assert_eq!(task.await.map_err(|_| "relay failed")?, (thread, 1));
        Ok(())
    }

    #[tokio::test]
    async fn early_events_are_delivered_when_an_open_completes()
    -> Result<(), &'static str> {
        let mut task = task();
        let (unblock, blocked) = oneshot::channel();
        let (reply, _) = oneshot::channel();
        task.opens.spawn(async move {
            let _ = blocked.await;
            super::Completed {
                reply,
                outcome: Err(super::Error::Exited),
            }
        });
        let thread = ThreadId(String::from("opening"));
        let first = event(&thread, "early");
        task.apply_input_depth_first(Input::Event(Some(first.clone())))
            .await
            .map_err(|_| "stopped")?;
        assert!(unblock.send(()).is_ok());
        let _ = task.opens.join_next().await;

        let (sender, mut events) = mpsc::channel(1);
        let early = task.early.open(&thread).map_err(|_| "overflowed")?;
        task.install_route(thread.clone(), sender, early)
            .await
            .map_err(|_| "stopped")?;
        let later = event(&thread, "later");
        task.apply_input_depth_first(Input::Event(Some(later.clone())))
            .await
            .map_err(|_| "stopped")?;
        assert_eq!(events.recv().await, Some(Routed::Codex(first)));
        assert_eq!(events.recv().await, Some(Routed::Codex(later)));
        Ok(())
    }

    #[tokio::test]
    async fn quiet_and_cancelled_sessions_leave_no_routes()
    -> Result<(), &'static str> {
        let mut task = task();
        let thread = ThreadId(String::from("quiet"));
        let (reply, cancel) = oneshot::channel();
        drop(cancel);
        assert!(connect((), reply).is_none());

        for _ in 0..3 {
            let (sender, events) = mpsc::channel(1);
            task.install_route(thread.clone(), sender, Vec::new())
                .await
                .map_err(|_| "stopped")?;
            drop(events);
            let (gone, generation) = task
                .relays
                .join_next()
                .await
                .ok_or("no relay")?
                .map_err(|_| "relay failed")?;
            task.apply_input_depth_first(Input::Gone {
                thread: gone,
                generation,
            })
            .await
            .map_err(|_| "stopped")?;
            assert!(matches!(
                task.machine
                    .step::<(), ()>(codex::hub::Input::Event(Some(event(
                        &thread, "later"
                    ))))
                    .as_slice(),
                [codex::hub::Effect::Unrouted(_)]
            ));
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod backpressure_tests {
    use std::time::Duration;

    use install::Home;
    use tokio::sync::oneshot;

    use crate::replay_support::{TestResult, committed_fake_cli_path};

    use super::{CodexServer, Command, Input, ServerOptions, Task, Up};

    #[tokio::test]
    async fn credential_rotation_stops_a_server_with_a_full_event_queue()
    -> TestResult<()> {
        let dir = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let cwd = dir.path().to_path_buf();
        let mut frames = String::from(
            "{\"id\":1,\"result\":{\"userAgent\":\"codex-test/1.0.0\"}}\n",
        );
        frames
            .push_str(&"{\"method\":\"conductor/testUnknown\"}\n".repeat(4097));
        std::fs::write(cwd.join("stdout.jsonl"), frames)?;
        let options = ServerOptions {
            codex_path: committed_fake_cli_path(),
            home: Home::User,
            credential: None,
            cwd,
        };
        let (server, events) = CodexServer::open(options.clone()).await?;
        let mut task = Task::new(options);
        task.server = Some(Up { server, events });
        let _ = task
            .machine
            .step::<(), ()>(codex::hub::Input::Spawned(Ok(())));
        let (reply, answer) = oneshot::channel();
        tokio::time::timeout(Duration::from_secs(20), async {
            let events = &task.server.as_ref().ok_or("server missing")?.events;
            while events.capacity() > 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            task.apply_input_depth_first(Input::Command(Command::Credential {
                credential: Some(codex::CodexCredential::Provider {
                    name: String::from("Test gateway"),
                    base_url: String::from("https://gateway.example"),
                    model: String::from("test-model"),
                    key: String::from("conductor-test-key"),
                }),
                reply,
            }))
            .await
            .map_err(|_| "hub stopped")?;
            answer.await??;
            assert!(task.server.is_none());
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        })
        .await??;
        Ok(())
    }
}
