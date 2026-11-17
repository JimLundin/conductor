//! The hub's task, which shares the host's one `codex app-server` among
//! every Codex thread. [`Hub`] is a handle to it. The task alone owns the
//! server and the hub's state machine ([`codex::hub::Hub`]): it turns each
//! command and server event into the machine's inputs and performs its
//! effects.

use std::convert::Infallible;
use std::sync::Arc;

use codex::hub::{Ask, Command, Routed};
use codex::{CodexEvent, ThreadId, ThreadOptions};
use proto::{CodexCredential, Models};
use tokio::sync::{mpsc, oneshot};

use crate::{CodexServer, CodexThread, Error, ServerOptions};

/// A thread, and its events, which end with the server's exit or stop.
pub type Thread = (CodexThread, mpsc::Receiver<Routed>);

/// A session's ask for a thread.
struct Asked {
    options: ThreadOptions,
    resume: Option<ThreadId>,
    reply: oneshot::Sender<Result<Thread, Error>>,
}

type Listed = oneshot::Sender<Result<Models, Error>>;
type Configured = oneshot::Sender<()>;
type Route = mpsc::Sender<Routed>;
type Machine = codex::hub::Hub<Asked, Listed, Route>;
type Input = codex::hub::Input<Asked, Listed, Configured, Route, Arc<Error>>;
type Effect = codex::hub::Effect<Asked, Listed, Configured, Route, Arc<Error>>;
type Msg = Command<Asked, Listed, Configured>;

/// A handle to the hub's task. Clone it freely.
#[derive(Debug, Clone)]
pub struct Hub {
    inbox: mpsc::Sender<Msg>,
}

impl Hub {
    /// Starts the hub's task. The app-server starts with the first thread
    /// or model listing, with `options`' credential until another is set.
    #[must_use]
    pub fn spawn(options: ServerOptions) -> Self {
        let (inbox, commands) = mpsc::channel(16);
        let task = Task {
            machine: Machine::new(options.credential.clone()),
            options,
            server: None,
        };
        tokio::spawn(task.run(commands));
        Self { inbox }
    }

    /// Starts a thread, or resumes `resume` from Codex's own history.
    ///
    /// # Errors
    /// Fails when the hub has stopped, the app-server doesn't start, or it
    /// refuses the thread.
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

    /// What `model/list` offers.
    ///
    /// # Errors
    /// Fails when the hub has stopped, or the app-server doesn't start or
    /// answer.
    pub async fn models(&self) -> Result<Models, Error> {
        self.ask(Command::Models).await?
    }

    /// Runs the next app-server with `credential`. A running one stops, and
    /// its threads end, saying why.
    ///
    /// # Errors
    /// Fails when the hub has stopped.
    pub async fn set_credential(
        &self,
        credential: Option<CodexCredential>,
    ) -> Result<(), Error> {
        self.ask(|reply| Command::Credential { credential, reply })
            .await
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

/// The running app-server and its events.
struct Up {
    server: CodexServer,
    events: mpsc::Receiver<CodexEvent>,
}

/// Every handle is gone, so the task has ended.
struct Ended;

struct Task {
    machine: Machine,
    options: ServerOptions,
    server: Option<Up>,
}

impl Task {
    async fn run(
        mut self,
        mut commands: mpsc::Receiver<Msg>,
    ) -> Result<Infallible, Ended> {
        loop {
            let input = tokio::select! {
                command = commands.recv() => command.map_or(Input::Closed, Input::Command),
                event = next_event(self.server.as_mut()) => Input::Event(event),
            };
            self.feed(input).await?;
        }
    }

    /// Steps the machine and performs its effects in order, each effect's
    /// own effects before the next.
    async fn feed(&mut self, input: Input) -> Result<(), Ended> {
        let mut effects = self.machine.step(input);
        effects.reverse();
        while let Some(effect) = effects.pop() {
            let more = self.perform(effect).await?;
            effects.extend(more.into_iter().rev());
        }
        Ok(())
    }

    async fn perform(&mut self, effect: Effect) -> Result<Vec<Effect>, Ended> {
        match effect {
            Effect::Spawn(credential) => {
                return Ok(self.spawn(credential).await);
            }
            Effect::Open(asked) => return Ok(self.open(asked).await),
            Effect::List(reply) => {
                let _ = reply.send(self.list().await);
            }
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
                if route.send(routed).await.is_err() {
                    return Ok(self.machine.step(Input::Gone(thread)));
                }
            }
            // A session that has gone fails its send, which is fine.
            Effect::Close { route, routed } => {
                let _ = route.send(routed).await;
            }
            Effect::Stop => self.stop().await,
            Effect::Lost(status) => {
                tracing::warn!(status = %status.describe(), "the codex app-server exited");
                self.server = None;
            }
            Effect::Unrouted(event) => {
                tracing::debug!(?event, "a codex event for no thread");
            }
            Effect::Configured(reply) => {
                let _ = reply.send(());
            }
            Effect::End => return Err(Ended),
        }
        Ok(Vec::new())
    }

    async fn spawn(
        &mut self,
        credential: Option<CodexCredential>,
    ) -> Vec<Effect> {
        let options = ServerOptions {
            credential,
            ..self.options.clone()
        };
        let spawned =
            CodexServer::spawn(options).await.map(|(server, events)| {
                tracing::info!(
                    version = server.codex_version().unwrap_or("?"),
                    "codex app-server started"
                );
                self.server = Some(Up { server, events });
            });
        self.machine.step(Input::Spawned(spawned.map_err(Arc::new)))
    }

    /// Starts the thread, and has the machine route its events before it
    /// answers, so its first events, which wait in the server's channel
    /// meanwhile, all find their route.
    async fn open(&mut self, asked: Asked) -> Vec<Effect> {
        let Asked {
            options,
            resume,
            reply,
        } = asked;
        match self.start(options, resume).await {
            Ok(thread) => {
                let (route, events) = mpsc::channel(256);
                let effects = self.machine.step(Input::Opened {
                    thread: thread.id.clone(),
                    route,
                });
                let _ = reply.send(Ok((thread, events)));
                effects
            }
            Err(error) => {
                let _ = reply.send(Err(error));
                Vec::new()
            }
        }
    }

    async fn start(
        &self,
        options: ThreadOptions,
        resume: Option<ThreadId>,
    ) -> Result<CodexThread, Error> {
        self.running()?.open_thread(options, resume).await
    }

    async fn list(&self) -> Result<Models, Error> {
        self.running()?.list_models().await
    }

    async fn stop(&mut self) {
        if let Some(up) = self.server.take() {
            let stopped = up.server.shutdown().await;
            tracing::info!(
                ?stopped,
                "stopped the codex app-server for a new credential"
            );
        }
    }

    /// The machine only asks for the server while one runs.
    fn running(&self) -> Result<CodexServer, Error> {
        self.server
            .as_ref()
            .map(|up| up.server.clone())
            .ok_or(Error::Exited)
    }
}

/// The running server's next event. With none running, it waits forever,
/// so the hub only takes commands.
async fn next_event(server: Option<&mut Up>) -> Option<CodexEvent> {
    match server {
        Some(up) => up.events.recv().await,
        None => std::future::pending().await,
    }
}
