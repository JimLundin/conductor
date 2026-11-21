//! Codex's runtime: one task per session's agent runs the
//! [`codex::runtime`] machine on a [`Runtime`]'s far end, opening its thread
//! on the hub's shared app-server and making the requests it decides on.

use std::sync::Arc;

use agents::Configuration;
use agents::runtime::NOT_RUNNING;
use codex::hub::Routed;
use codex::runtime::{Call, Effect, Input, Open};
use codex::{ThreadId, TurnId};
use proto::Credential;
use runtime::agent::{self, Agent, Performed};
use runtime::{Configuring, ProviderInstance, Runtime};
use tokio::sync::mpsc;

use super::hub::{Hub, Thread};
use super::server::Error;

impl ProviderInstance for Hub {
    fn configure(&self, given: Option<Credential>) -> Configuring<'_> {
        Box::pin(async move {
            let credential =
                given.as_ref().map(codex::credential).transpose()?;
            let named = credential.as_ref().map(codex::default_model);
            self.set_credential(credential)
                .await
                .map_err(|error| error.to_string())?;
            Ok(Configuration {
                named,
                models: self.models().await.map_err(|error| error.to_string()),
            })
        })
    }

    fn start(self: Arc<Self>) -> Runtime {
        let mut machine = codex::runtime::Runtime::new();
        agent::start(
            Task {
                hub: self,
                thread: None,
            },
            move |input| machine.step(input),
            Input::Command,
            Input::Event,
        )
    }
}

struct Task {
    hub: Arc<Hub>,
    thread: Option<Thread>,
}

impl Agent<Input, Effect> for Task {
    type Event = Routed;

    fn events(&mut self) -> Option<&mut mpsc::Receiver<Self::Event>> {
        self.thread.as_mut().map(|(_, events)| events)
    }

    fn release(&mut self) {
        self.thread = None;
    }

    async fn perform(&mut self, effect: Effect) -> Performed<Input> {
        let feedback = match effect {
            Effect::Open(open) => Input::Opened(self.open(open).await),
            Effect::Call { call, then } => match self.call(call).await {
                Err(Error::MutationUnknown(_)) => Input::RevertUnknown,
                outcome => Input::Called {
                    then,
                    outcome: outcome.map_err(|error| error.to_string()),
                },
            },
            Effect::Turns => Input::Listed(self.turns().await),
            Effect::Runtime(report) => return Performed::Report(report),
        };
        Performed::Feedback(feedback)
    }
}

impl Task {
    /// Opens the thread in place of the running one, if it opens.
    async fn open(&mut self, open: Open) -> Result<ThreadId, String> {
        let (thread, events) = self
            .hub
            .thread(open.options, open.resume)
            .await
            .map_err(|error| error.to_string())?;
        let id = thread.id.clone();
        self.thread = Some((thread, events));
        Ok(id)
    }

    async fn call(&self, call: Call) -> Result<(), Error> {
        let Some((thread, _)) = &self.thread else {
            return Err(Error::NotRunning);
        };
        match call {
            Call::SendTurn { text, effort } => {
                thread.send_turn(text, effort).await.map(drop)
            }
            Call::Interrupt(turn) => thread.interrupt(turn).await,
            Call::Respond { id, decision } => {
                thread.respond(id, decision).await
            }
            Call::Revert(turn) => thread.revert(turn).await,
        }
    }

    async fn turns(&self) -> Result<Vec<TurnId>, String> {
        let Some((thread, _)) = &self.thread else {
            return Err(String::from(NOT_RUNNING));
        };
        thread
            .history_turns()
            .await
            .map_err(|error| error.to_string())
    }
}
