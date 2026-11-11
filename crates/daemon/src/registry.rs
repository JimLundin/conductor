//! The registry: the one place a `StartSession` is checked and a session is
//! found by its id.

use std::convert::Infallible;

use proto::{CommandResult, HostPath, Provider, Rejection};
use tokio::sync::{mpsc, oneshot};

use crate::Stopped;

/// How the host runs each provider. Built once in `main`.
#[derive(Debug, Clone)]
pub struct Providers {
    pub claude: Availability,
    pub codex: Availability,
}

#[derive(Debug, Clone)]
pub enum Availability {
    /// The host can't run this provider's sessions, for this reason.
    Unavailable(String),
}

#[derive(Debug, Clone)]
pub struct Registry {
    inbox: mpsc::Sender<RegistryMsg>,
}

enum RegistryMsg {
    Start {
        provider: Provider,
        directory: HostPath,
        reply: oneshot::Sender<CommandResult>,
    },
}

struct RegistryActor {
    providers: Providers,
}

impl Registry {
    pub fn spawn(providers: Providers) -> Registry {
        let (inbox, messages) = mpsc::channel(16);
        tokio::spawn(RegistryActor { providers }.run(messages));
        Registry { inbox }
    }

    /// Checks the directory, then the provider.
    pub async fn start(
        &self,
        provider: Provider,
        directory: HostPath,
    ) -> Result<CommandResult, Stopped> {
        self.ask(|reply| RegistryMsg::Start {
            provider,
            directory,
            reply,
        })
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
    async fn run(self, mut messages: mpsc::Receiver<RegistryMsg>) -> Result<Infallible, Stopped> {
        loop {
            let msg = messages.recv().await.ok_or(Stopped {})?;
            self.on(msg);
        }
    }

    fn on(&self, msg: RegistryMsg) {
        match msg {
            RegistryMsg::Start {
                provider,
                directory,
                reply,
            } => {
                let _ = reply.send(self.start(provider, &directory));
            }
        }
    }

    fn start(&self, provider: Provider, directory: &HostPath) -> CommandResult {
        let availability = match provider {
            Provider::Claude => &self.providers.claude,
            Provider::Codex => &self.providers.codex,
        };
        // `read_dir` fails for a missing path and for a file alike.
        match (std::fs::read_dir(&directory.0), availability) {
            (Err(_), _) => CommandResult::Rejected(Rejection::NoSuchDirectory),
            (Ok(_), Availability::Unavailable(message)) => {
                CommandResult::Rejected(Rejection::ProviderUnavailable {
                    message: message.clone(),
                })
            }
        }
    }
}
