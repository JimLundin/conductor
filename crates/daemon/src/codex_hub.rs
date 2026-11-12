//! The host's one `codex app-server`, shared by every Codex session. The hub
//! alone reads the server's events and routes each thread's to its session.
//!
//! The server starts with the first thread a session asks for. When it
//! exits, the hub sends the exit to every Codex session, which logs
//! `SessionClosed` and ends, and forgets the server. The next thread start
//! runs a new one, so a crash costs the sessions that were open, not the
//! host's Codex.
//!
//! A new credential stops the server the same way. Its key is in the
//! server's environment, which is fixed at start, so the next thread runs a
//! server that has it.

use std::collections::HashMap;
use std::convert::Infallible;

use agents::Exit;
use agents::codex::{
    CodexEvent, CodexServer, CodexThread, ServerOptions, ThreadEvent, ThreadId, ThreadOptions,
};
use n0_future::StreamExt;
use proto::CodexCredential;
use tokio::sync::{mpsc, oneshot};

use crate::Stopped;

/// What the hub sends a Codex session.
#[derive(Debug, Clone)]
pub(crate) enum Routed {
    Codex(CodexEvent),
    /// The hub stopped the app-server on purpose, for this reason.
    Stopped {
        reason: String,
    },
}

/// A handle to the hub's task. Clone it freely.
#[derive(Debug, Clone)]
pub struct CodexHub {
    inbox: mpsc::Sender<HubMsg>,
}

#[derive(Debug, thiserror::Error)]
pub enum HubError {
    #[error("{0}")]
    Codex(#[from] agents::codex::Error),
    #[error("the codex hub has stopped")]
    Stopped(#[from] Stopped),
}

enum HubMsg {
    /// Starts a thread, answered with the thread and its events.
    Start {
        options: ThreadOptions,
        reply: oneshot::Sender<Result<(CodexThread, mpsc::Receiver<Routed>), agents::codex::Error>>,
    },
    /// Runs the next server with this credential, stopping the current one.
    Configure {
        credential: Option<CodexCredential>,
        reply: oneshot::Sender<()>,
    },
}

enum Server {
    /// No app-server runs: none has started yet, or the last one exited.
    Down,
    Up {
        server: CodexServer,
        events: mpsc::Receiver<CodexEvent>,
    },
}

struct HubActor {
    options: ServerOptions,
    server: Server,
    routes: HashMap<ThreadId, mpsc::Sender<Routed>>,
}

enum HubInput {
    Msg(HubMsg),
    Codex(CodexEvent),
    /// Every handle was dropped.
    Closed,
}

impl CodexHub {
    /// Starts the hub's task. The app-server starts with the first thread.
    pub fn spawn(options: ServerOptions) -> CodexHub {
        let (inbox, messages) = mpsc::channel(16);
        let actor = HubActor {
            options,
            server: Server::Down,
            routes: HashMap::new(),
        };
        tokio::spawn(actor.run(messages));
        CodexHub { inbox }
    }

    /// Starts a thread, and the app-server first if none runs. The thread's
    /// events arrive on the receiver, ending with the server's exit.
    pub(crate) async fn start_thread(
        &self,
        options: ThreadOptions,
    ) -> Result<(CodexThread, mpsc::Receiver<Routed>), HubError> {
        Ok(self.ask(|reply| HubMsg::Start { options, reply }).await??)
    }

    /// Takes a new credential. The running server stops, and its sessions
    /// close, saying why.
    pub(crate) async fn configure(
        &self,
        credential: Option<CodexCredential>,
    ) -> Result<(), Stopped> {
        self.ask(|reply| HubMsg::Configure { credential, reply })
            .await
    }

    async fn ask<T>(&self, build: impl FnOnce(oneshot::Sender<T>) -> HubMsg) -> Result<T, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.inbox
            .send(build(reply))
            .await
            .map_err(|_| Stopped {})?;
        answer.await.map_err(|_| Stopped {})
    }
}

impl Server {
    /// The next event of the running server. With none running, it waits
    /// forever, so the hub only takes commands.
    async fn next(&mut self) -> CodexEvent {
        match self {
            Server::Up { events, .. } => events.recv().await.unwrap_or(CodexEvent::Exited {
                status: Exit::Unknown(String::from("the app-server's events ended")),
            }),
            Server::Down => std::future::pending().await,
        }
    }
}

impl HubActor {
    async fn run(mut self, mut inbox: mpsc::Receiver<HubMsg>) -> Result<Infallible, Stopped> {
        loop {
            let input = tokio::select! {
                msg = inbox.recv() => msg.map_or(HubInput::Closed, HubInput::Msg),
                event = self.server.next() => HubInput::Codex(event),
            };
            self.on(input).await?;
        }
    }

    async fn on(&mut self, input: HubInput) -> Result<(), Stopped> {
        match input {
            HubInput::Msg(HubMsg::Start { options, reply }) => {
                let started = self.start(options).await;
                let _ = reply.send(started);
                Ok(())
            }
            HubInput::Msg(HubMsg::Configure { credential, reply }) => {
                self.options.credential = credential;
                if let Server::Up { server, .. } = &self.server {
                    let stopped = server.shutdown().await;
                    tracing::info!(
                        ?stopped,
                        "stopped the codex app-server for a new credential"
                    );
                    self.server = Server::Down;
                    let reason = String::from("new Codex credential");
                    self.close_all(Routed::Stopped { reason }).await;
                }
                let _ = reply.send(());
                Ok(())
            }
            HubInput::Codex(CodexEvent::Thread { thread, event }) => {
                self.route(thread, event).await;
                Ok(())
            }
            HubInput::Codex(CodexEvent::Exited { status }) => {
                tracing::warn!(status = %status.describe(), "the codex app-server exited");
                self.server = Server::Down;
                self.close_all(Routed::Codex(CodexEvent::Exited { status }))
                    .await;
                Ok(())
            }
            HubInput::Codex(CodexEvent::Unknown(frame)) => {
                tracing::debug!(%frame, "a codex frame for no thread");
                Ok(())
            }
            HubInput::Closed => Err(Stopped {}),
        }
    }

    /// Tells every session the server has gone, waiting for room in a full
    /// channel, and forgets their routes.
    async fn close_all(&mut self, last: Routed) {
        let closes = self
            .routes
            .drain()
            .map(|(_, route)| (route, last.clone()))
            .collect::<Vec<(mpsc::Sender<Routed>, Routed)>>();
        // A session that has gone fails its send, which is fine.
        let _ = n0_future::stream::iter(closes)
            .then(|(route, close)| async move { route.send(close).await })
            .collect::<Vec<Result<(), mpsc::error::SendError<Routed>>>>()
            .await;
    }

    /// Inserts the route before it answers, so the thread's first events,
    /// which wait in the server's channel meanwhile, all find it.
    async fn start(
        &mut self,
        options: ThreadOptions,
    ) -> Result<(CodexThread, mpsc::Receiver<Routed>), agents::codex::Error> {
        let server = self.up().await?;
        let thread = server.start_thread(options).await?;
        let (route, events) = mpsc::channel(256);
        self.routes.insert(thread.id.clone(), route);
        Ok((thread, events))
    }

    /// The running server, started first if none runs.
    async fn up(&mut self) -> Result<CodexServer, agents::codex::Error> {
        match &self.server {
            Server::Up { server, .. } => Ok(server.clone()),
            Server::Down => {
                let (server, events) = CodexServer::spawn(self.options.clone()).await?;
                tracing::info!(
                    version = server.codex_version().unwrap_or("?"),
                    "codex app-server started"
                );
                self.server = Server::Up {
                    server: server.clone(),
                    events,
                };
                Ok(server)
            }
        }
    }

    /// A thread whose session ended has no route any more, and its events
    /// are dropped.
    async fn route(&mut self, thread: ThreadId, event: ThreadEvent) {
        let event = Routed::Codex(CodexEvent::Thread {
            thread: thread.clone(),
            event,
        });
        match self.routes.get(&thread) {
            Some(route) => {
                if let Err(mpsc::error::SendError(_)) = route.send(event).await {
                    self.routes.remove(&thread);
                }
            }
            None => tracing::debug!(thread = %thread.0, "an event for a thread with no session"),
        }
    }
}
