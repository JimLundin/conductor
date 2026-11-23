//! What a Claude agent does with a session's runtime commands
//! (`agents::runtime`), as a state machine.
//!
//! It decides which conversation the CLI starts on and which it tries
//! next, the control calls each command becomes, and the runtime events
//! each Claude event becomes.
//!
//! `claude-cli` runs it: it turns each command, call outcome and Claude
//! event into an [`Input`], and performs each [`Effect`] in order.

use agents::Note;
use agents::runtime::{Command, Lifecycle, Outcome, Report, Start, Started};
use proto::{Effort, HostPath, SessionId as Session};

use crate::prelude::*;
use crate::{
    ClaudeEvent, PermissionDecision, PermissionRequestId, Resume, SessionId,
    claude_answer, claude_steps,
};

/// Why the CLI did not start, and the conversation it found missing if
/// that is why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub message: String,
    pub missing: Option<SessionId>,
}

/// How to start the CLI for a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawn {
    pub session: Session,
    pub directory: HostPath,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub resume: Resume,
    pub instructions: Option<String>,
}

/// A call on the running CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    SetEffort(Effort),
    SendUser(String),
    Interrupt,
    Respond {
        id: PermissionRequestId,
        decision: PermissionDecision,
    },
}

/// What a call's outcome decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then {
    /// The effort changed for a turn: send its text next.
    Retuned(String),
    Sent,
    Interrupted,
    Replied,
}

/// What happened, as the shell saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Command(Command),
    /// How the [`Effect::Spawn`] went: the conversation the CLI runs.
    Spawned(Result<SessionId, Refusal>),
    /// How the [`Effect::Call`] for `then` went.
    Called {
        then: Then,
        outcome: Result<(), String>,
    },
    /// How the [`Effect::Shutdown`] went.
    ShutDown(Result<(), String>),
    /// The running CLI's next event, `None` once its events end.
    Event(Option<ClaudeEvent>),
}

/// What the shell does next, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Start the CLI in place of the one running, if it starts, and report
    /// [`Input::Spawned`].
    Spawn(Spawn),
    /// Make the call on the running CLI and report [`Input::Called`].
    Call {
        call: Call,
        then: Then,
    },
    /// Stop the running CLI and report [`Input::ShutDown`].
    Shutdown,
    Runtime(Report),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Runtime {
    lifecycle: Lifecycle,
    starting: Option<Spawn>,
}

impl From<Report> for Effect {
    fn from(report: Report) -> Self {
        Self::Runtime(report)
    }
}

impl Runtime {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            lifecycle: Lifecycle::new(),
            starting: None,
        }
    }

    pub fn step(&mut self, input: Input) -> Vec<Effect> {
        match input {
            Input::Command(command) => self.command(command),
            Input::Spawned(spawned) => self.spawned(spawned),
            Input::Called { then, outcome } => called(then, outcome),
            Input::ShutDown(outcome) => self.lifecycle.shut_down(outcome),
            Input::Event(Some(event)) => self.event(event),
            Input::Event(None) => self.lifecycle.ended(),
        }
    }

    fn command(&mut self, command: Command) -> Vec<Effect> {
        if let Some(refused) = self.lifecycle.refusal(&command) {
            return refused;
        }
        match command {
            Command::Start(start) => self.start(start),
            Command::Shutdown if self.lifecycle.running() => {
                Vec::from([Effect::Shutdown])
            }
            Command::Shutdown => Report::emit(Outcome::ShutDown(Ok(()))),
            Command::Send { text, effort } => Vec::from([send(text, effort)]),
            Command::Interrupt(_) => Vec::from([Effect::Call {
                call: Call::Interrupt,
                then: Then::Interrupted,
            }]),
            Command::Reply { native, reply, by } => Vec::from([Effect::Call {
                call: Call::Respond {
                    id: PermissionRequestId(native.0),
                    decision: claude_answer(reply, &by),
                },
                then: Then::Replied,
            }]),
        }
    }

    fn start(&mut self, start: Start) -> Vec<Effect> {
        let Start {
            session,
            directory,
            model,
            effort,
            conversation,
            instructions,
        } = start;
        let resume = conversation
            .map(SessionId)
            .map_or(Resume::Fresh, Resume::Session);
        let spawn = Spawn {
            session,
            directory,
            model: model.map(|chosen| chosen.0),
            effort,
            resume,
            instructions,
        };
        self.starting = Some(spawn.clone());
        Vec::from([Effect::Spawn(spawn)])
    }

    fn spawned(&mut self, spawned: Result<SessionId, Refusal>) -> Vec<Effect> {
        let starting = self.starting.take();
        match (spawned, starting) {
            (Ok(id), _) => {
                self.lifecycle.started(Started { conversation: id.0 })
            }
            (Err(refusal), Some(spawn))
                if refusal.missing.is_some()
                    && matches!(spawn.resume, Resume::Session(_)) =>
            {
                let next = Spawn {
                    resume: Resume::Fresh,
                    ..spawn
                };
                self.starting = Some(next.clone());
                Vec::from([Effect::Spawn(next)])
            }
            (Err(refusal), _) => {
                Report::emit(Outcome::Started(Err(refusal.message)))
            }
        }
    }

    fn event(&mut self, event: ClaudeEvent) -> Vec<Effect> {
        let note = match &event {
            ClaudeEvent::Unknown(_) => Some(Note::unmapped(
                crate::PROVIDER,
                String::from("unknown"),
                None,
            )),
            _ => None,
        };
        self.lifecycle.steps(claude_steps(event), note)
    }
}

/// Starts a turn, changing the CLI's effort in its process first if the
/// turn asks for another.
const fn send(text: String, effort: Option<Effort>) -> Effect {
    match effort {
        Some(changed) => Effect::Call {
            call: Call::SetEffort(changed),
            then: Then::Retuned(text),
        },
        None => Effect::Call {
            call: Call::SendUser(text),
            then: Then::Sent,
        },
    }
}

fn called(then: Then, outcome: Result<(), String>) -> Vec<Effect> {
    match (then, outcome) {
        (Then::Retuned(text), Ok(())) => Vec::from([send(text, None)]),
        (Then::Retuned(_), Err(error)) => {
            Report::emit(Outcome::Sent(Err(error)))
        }
        (Then::Sent, sent) => Report::emit(Outcome::Sent(sent)),
        (Then::Interrupted, interrupted) => {
            Report::emit(Outcome::Interrupted(interrupted))
        }
        (Then::Replied, replied) => Report::emit(Outcome::Replied(replied)),
    }
}

#[cfg(test)]
mod tests {
    use agents::runtime::{Command, Event, Outcome, Report, Start, Started};
    use agents::{NativeRequest, NativeTurn, Reply};
    use proto::{
        ClientName, Decision, Effort, HostPath, ModelId, SessionId as Session,
    };

    use super::{Call, Effect, Input, Refusal, Runtime, Spawn, Then};
    use crate::{PermissionDecision, PermissionRequestId, Resume, SessionId};

    fn id(text: &str) -> SessionId {
        SessionId(String::from(text))
    }

    fn text(value: &str) -> String {
        String::from(value)
    }

    fn walk(runtime: &mut Runtime, inputs: Vec<Input>) -> Vec<Vec<Effect>> {
        inputs
            .into_iter()
            .map(|input| runtime.step(input))
            .collect()
    }

    fn outcome(outcome: Outcome) -> Vec<Effect> {
        Vec::from([Report::Emit(Event::Outcome(outcome)).into()])
    }

    fn start(conversation: Option<&str>) -> Input {
        Input::Command(Command::Start(Start {
            instructions: None,
            session: Session(1),
            directory: HostPath(text("/work")),
            model: Some(ModelId(text("haiku"))),
            effort: Some(Effort::Low),
            conversation: conversation.map(text),
        }))
    }

    fn spawn(resume: Resume) -> Effect {
        Effect::Spawn(Spawn {
            instructions: None,
            session: Session(1),
            directory: HostPath(text("/work")),
            model: Some(text("haiku")),
            effort: Some(Effort::Low),
            resume,
        })
    }

    fn missing(session: &str) -> Input {
        Input::Spawned(Err(Refusal {
            message: text("missing"),
            missing: Some(id(session)),
        }))
    }

    fn started(conversation: &str) -> Vec<Effect> {
        outcome(Outcome::Started(Ok(Started {
            conversation: text(conversation),
        })))
    }

    fn running() -> Runtime {
        let mut runtime = Runtime::new();
        let _ = walk(
            &mut runtime,
            Vec::from([start(Some("saved")), Input::Spawned(Ok(id("saved")))]),
        );
        runtime
    }

    #[test]
    fn a_missing_saved_conversation_starts_fresh() {
        let mut runtime = Runtime::new();
        let seen = walk(
            &mut runtime,
            Vec::from([
                start(Some("conv")),
                missing("conv"),
                Input::Spawned(Ok(id("new"))),
            ]),
        );
        assert_eq!(
            seen,
            [
                Vec::from([spawn(Resume::Session(id("conv")))]),
                Vec::from([spawn(Resume::Fresh)]),
                started("new"),
            ]
        );
    }

    #[test]
    fn a_turn_at_another_effort_changes_it_in_the_process_first() {
        let mut runtime = running();
        let seen = walk(
            &mut runtime,
            Vec::from([
                Input::Command(Command::Send {
                    text: text("hi"),
                    effort: Some(Effort::High),
                }),
                Input::Called {
                    then: Then::Retuned(text("hi")),
                    outcome: Ok(()),
                },
                Input::Called {
                    then: Then::Sent,
                    outcome: Ok(()),
                },
                Input::Command(Command::Send {
                    text: text("again"),
                    effort: Some(Effort::Low),
                }),
                Input::Called {
                    then: Then::Retuned(text("again")),
                    outcome: Err(text("busy")),
                },
            ]),
        );
        assert_eq!(
            seen,
            [
                Vec::from([Effect::Call {
                    call: Call::SetEffort(Effort::High),
                    then: Then::Retuned(text("hi")),
                }]),
                Vec::from([Effect::Call {
                    call: Call::SendUser(text("hi")),
                    then: Then::Sent,
                }]),
                outcome(Outcome::Sent(Ok(()))),
                Vec::from([Effect::Call {
                    call: Call::SetEffort(Effort::Low),
                    then: Then::Retuned(text("again")),
                }]),
                outcome(Outcome::Sent(Err(text("busy")))),
            ]
        );
    }

    #[test]
    fn commands_become_calls_on_the_running_cli() {
        let mut runtime = running();
        assert_eq!(
            walk(
                &mut runtime,
                Vec::from([
                    Input::Command(Command::Interrupt(NativeTurn(text(
                        "saved"
                    )))),
                    Input::Command(Command::Reply {
                        native: NativeRequest(text("perm-1")),
                        reply: Reply::Decide(Decision::Approve),
                        by: ClientName(text("phone")),
                    }),
                    Input::Command(Command::Shutdown),
                ]),
            ),
            [
                Vec::from([Effect::Call {
                    call: Call::Interrupt,
                    then: Then::Interrupted,
                }]),
                Vec::from([Effect::Call {
                    call: Call::Respond {
                        id: PermissionRequestId(text("perm-1")),
                        decision: PermissionDecision::Allow,
                    },
                    then: Then::Replied,
                }]),
                Vec::from([Effect::Shutdown]),
            ],
        );
    }
}
