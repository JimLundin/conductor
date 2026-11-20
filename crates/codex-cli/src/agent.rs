//! Codex's runtime: one task per session's agent runs the
//! [`codex::runtime`] machine on a [`Runtime`]'s far end, opening its thread
//! on the hub's shared app-server and making the requests it decides on.

use agents::runtime::NOT_RUNNING;
use codex::hub::Routed;
use codex::runtime::{Call, Effect, Input, Open};
use codex::{ThreadId, TurnId};
use runtime::agent::{self, Agent, Performed};
use runtime::{Runtime, Starter};
use tokio::sync::mpsc;

use crate::{Hub, Thread};

/// Starts Codex agents, each a thread on `hub`'s app-server.
#[must_use]
pub fn starter(hub: Hub) -> Starter {
    Starter::new(move || start(hub.clone()))
}

fn start(hub: Hub) -> Runtime {
    let mut machine = codex::runtime::Runtime::new();
    agent::start(
        Task { hub, thread: None },
        move |input| machine.step(input),
        Input::Command,
        Input::Event,
    )
}

struct Task {
    hub: Hub,
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
            Effect::Call { call, then } => Input::Called {
                then,
                outcome: self.call(call).await,
            },
            Effect::Turns => Input::Listed(self.turns().await),
            Effect::Models => Input::Models(
                self.hub.models().await.map_err(|error| error.to_string()),
            ),
            Effect::Credential(credential) => Input::Configured(
                self.hub
                    .set_credential(credential)
                    .await
                    .map_err(|error| error.to_string()),
            ),
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

    async fn call(&self, call: Call) -> Result<(), String> {
        let Some((thread, _)) = &self.thread else {
            return Err(String::from(NOT_RUNNING));
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
        .map_err(|error| error.to_string())
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
