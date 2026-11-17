//! Codex's runtime: one task per session's agent runs the
//! [`codex::runtime`] machine on a [`Runtime`]'s far end, opening its thread
//! on the hub's shared app-server and making the requests it decides on.

use std::convert::Infallible;
use std::future::pending;

use codex::hub::Routed;
use codex::runtime::{Call, Effect, Input, Open};
use codex::{ThreadId, TurnId};
use runtime::{Plug, Runtime, Starter};

use crate::{Hub, Thread};

/// Starts Codex agents, each a thread on `hub`'s app-server.
#[must_use]
pub fn starter(hub: Hub) -> Starter {
    Starter::new(move || start(hub.clone()))
}

fn start(hub: Hub) -> Runtime {
    let (runtime, plug) = runtime::pair();
    let task = Task {
        machine: codex::runtime::Runtime::new(),
        hub,
        thread: None,
        plug,
    };
    tokio::spawn(task.run());
    runtime
}

struct Task {
    machine: codex::runtime::Runtime,
    hub: Hub,
    thread: Option<Thread>,
    plug: Plug,
}

impl Task {
    /// Runs until the session drops its runtime.
    async fn run(mut self) -> Option<Infallible> {
        loop {
            let input = tokio::select! {
                command = self.plug.commands.recv() => Input::Command(command?),
                routed = next_event(self.thread.as_mut()) => Input::Event(routed),
            };
            self.feed(input).await;
        }
    }

    /// Steps the machine and performs its effects in order, each effect's
    /// own effects before the next.
    async fn feed(&mut self, input: Input) {
        let mut effects = self.machine.step(input);
        effects.reverse();
        while let Some(effect) = effects.pop() {
            let more = self.perform(effect).await;
            effects.extend(more.into_iter().rev());
        }
    }

    async fn perform(&mut self, effect: Effect) -> Vec<Effect> {
        match effect {
            Effect::Open(open) => {
                let opened = self.open(open).await;
                self.machine.step(Input::Opened(opened))
            }
            Effect::Call { call, then } => {
                let outcome = self.call(call).await;
                self.machine.step(Input::Called { then, outcome })
            }
            Effect::Turns => {
                let listed = self.turns().await;
                self.machine.step(Input::Listed(listed))
            }
            Effect::Models => {
                let listed = self.hub.models().await;
                self.machine.step(Input::Models(
                    listed.map_err(|error| error.to_string()),
                ))
            }
            Effect::Credential(credential) => {
                let set = self.hub.set_credential(credential).await;
                self.machine.step(Input::Configured(
                    set.map_err(|error| error.to_string()),
                ))
            }
            Effect::Release => {
                self.thread = None;
                Vec::new()
            }
            Effect::Emit(event) => {
                // A session that has gone takes no more events.
                let _ = self.plug.events.send(event).await;
                Vec::new()
            }
            Effect::Log(note) => {
                runtime::log(&note);
                Vec::new()
            }
        }
    }

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
            return Err(String::from("the agent is not running"));
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
            return Err(String::from("the agent is not running"));
        };
        thread
            .history_turns()
            .await
            .map_err(|error| error.to_string())
    }
}

/// The thread's next event. With none open, it waits forever, so the task
/// only takes commands.
async fn next_event(thread: Option<&mut Thread>) -> Option<Routed> {
    match thread {
        Some((_, events)) => events.recv().await,
        None => pending().await,
    }
}
