//! The registry: the one place a `StartSession` is checked and a session is
//! found by its id.

use std::collections::BTreeMap;
use std::convert::Infallible;

use proto::{
    Availability, ClientName, CommandResult, EventKind, HostPath, Provider, Providers, Rejection,
    SessionId,
};
use tokio::sync::{mpsc, oneshot};

use crate::Stopped;
use crate::log::Log;
use crate::session::{Launch, Session};

/// How the host runs each provider. Built once in `main`.
#[derive(Debug, Clone)]
pub struct Launchers {
    pub claude: Launcher,
    pub codex: Launcher,
}

#[derive(Debug, Clone)]
pub enum Launcher {
    Ready(Launch),
    /// The host can't run this provider's sessions, for this reason.
    Unavailable(String),
}

impl Launchers {
    /// What `Welcome` tells a client the host can run.
    pub fn providers(&self) -> Providers {
        Providers {
            claude: self.claude.availability(),
            codex: self.codex.availability(),
        }
    }

    fn of(&self, provider: Provider) -> &Launcher {
        match provider {
            Provider::Claude => &self.claude,
            Provider::Codex => &self.codex,
        }
    }
}

impl Launcher {
    fn availability(&self) -> Availability {
        match self {
            Launcher::Ready(_) => Availability::Available,
            Launcher::Unavailable(reason) => Availability::Unavailable {
                reason: reason.clone(),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct Registry {
    inbox: mpsc::Sender<RegistryMsg>,
}

enum RegistryMsg {
    Start {
        provider: Provider,
        directory: HostPath,
        by: ClientName,
        reply: oneshot::Sender<CommandResult>,
    },
    Find {
        session: SessionId,
        reply: oneshot::Sender<Option<Session>>,
    },
}

struct RegistryActor {
    log: Log,
    launchers: Launchers,
    sessions: BTreeMap<SessionId, Session>,
    last: SessionId,
}

impl Registry {
    pub fn spawn(log: Log, launchers: Launchers) -> Registry {
        let (inbox, messages) = mpsc::channel(16);
        let actor = RegistryActor {
            log,
            launchers,
            sessions: BTreeMap::new(),
            last: SessionId(0),
        };
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
        by: ClientName,
    ) -> Result<CommandResult, Stopped> {
        self.ask(|reply| RegistryMsg::Start {
            provider,
            directory,
            by,
            reply,
        })
        .await
    }

    pub async fn find(&self, session: SessionId) -> Result<Option<Session>, Stopped> {
        self.ask(|reply| RegistryMsg::Find { session, reply }).await
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
            RegistryMsg::Start {
                provider,
                directory,
                by,
                reply,
            } => {
                let result = self.start(provider, directory, by).await?;
                let _ = reply.send(result);
            }
            RegistryMsg::Find { session, reply } => {
                let _ = reply.send(self.sessions.get(&session).cloned());
            }
        }
        Ok(())
    }

    async fn start(
        &mut self,
        provider: Provider,
        directory: HostPath,
        by: ClientName,
    ) -> Result<CommandResult, Stopped> {
        // `read_dir` fails for a missing path and for a file alike.
        match (
            std::fs::read_dir(&directory.0),
            self.launchers.of(provider).clone(),
        ) {
            (Err(_), _) => Ok(CommandResult::Rejected(Rejection::NoSuchDirectory)),
            (Ok(_), Launcher::Unavailable(reason)) => {
                Ok(CommandResult::Rejected(Rejection::ProviderUnavailable {
                    reason,
                }))
            }
            (Ok(_), Launcher::Ready(launch)) => {
                let id = self.last.next();
                self.last = id;
                let created = EventKind::SessionCreated {
                    provider,
                    directory: directory.clone(),
                    by,
                };
                self.log.append(id, created).await?;
                let session = Session::spawn(id, directory, launch, self.log.clone());
                self.sessions.insert(id, session);
                Ok(CommandResult::Started { session: id })
            }
        }
    }
}
