//! What a Codex agent does with a session's runtime commands
//! (`agents::runtime`), as a state machine.
//!
//! It decides which thread opens, the app-server requests each command
//! becomes, and the runtime events each thread event becomes. The thread
//! runs on the hub's shared app-server.
//!
//! `codex-cli` runs it: it turns each command, request outcome and routed
//! event into an [`Input`], and performs each [`Effect`] in order.

use agents::Step;
use agents::runtime::{
    Command, Event, Lifecycle, Outcome, Report, Start, Started,
};
use proto::{CloseReason, Effort};

use crate::hub::Routed;
use crate::mapping::codex_note;
use crate::prelude::*;
use crate::{
    ApprovalId, ApprovalPolicy, Decision, Sandbox, ThreadId, ThreadOptions,
    TurnId, codex_answer, codex_steps,
};

/// The thread to open: resumed from Codex's own history, or a new one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Open {
    pub options: ThreadOptions,
    pub resume: Option<ThreadId>,
}

/// A request on the running thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// Starts a turn, at `effort` if given: it stays the thread's effort.
    SendTurn {
        text: String,
        effort: Option<Effort>,
    },
    Interrupt(TurnId),
    Respond {
        thread: Option<ThreadId>,
        id: ApprovalId,
        decision: Decision,
    },
}

/// The command a call's outcome answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Then {
    Sent,
    Interrupted,
    Replied,
}

/// What happened, as the shell saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Command(Command),
    /// How the [`Effect::Open`] went: the thread that opened.
    Opened(Result<ThreadId, String>),
    /// How the [`Effect::Call`] for `then` went.
    Called {
        then: Then,
        outcome: Result<(), String>,
    },
    /// The thread's next event, `None` once its events end.
    Event(Option<Routed>),
}

/// What the shell does next, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Open the thread in place of the running one, if it opens, and report
    /// [`Input::Opened`].
    Open(Open),
    /// Make the request on the running thread and report [`Input::Called`].
    Call {
        call: Call,
        then: Then,
    },
    Runtime(Report),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Runtime {
    lifecycle: Lifecycle,
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
        }
    }

    pub fn step(&mut self, input: Input) -> Vec<Effect> {
        match input {
            Input::Command(command) => self.command(command),
            Input::Opened(Ok(thread)) => self.lifecycle.started(Started {
                conversation: thread.0,
            }),
            Input::Opened(Err(error)) => {
                Report::emit(Outcome::Started(Err(error)))
            }
            Input::Called { then, outcome } => {
                Report::emit(answered(then, outcome))
            }
            Input::Event(Some(routed)) => self.routed(routed),
            Input::Event(None) => self.lifecycle.ended(),
        }
    }

    fn command(&mut self, command: Command) -> Vec<Effect> {
        if let Some(refused) = self.lifecycle.refusal(&command) {
            return refused;
        }
        match command {
            Command::Start(start) => Vec::from([Effect::Open(open(start))]),
            Command::Shutdown => Vec::from([
                self.lifecycle.release().into(),
                Report::Emit(Event::Outcome(Outcome::ShutDown(Ok(())))).into(),
            ]),
            Command::Send { text, effort } => {
                call(Call::SendTurn { text, effort }, Then::Sent)
            }
            Command::Interrupt(turn) => {
                call(Call::Interrupt(TurnId(turn.0)), Then::Interrupted)
            }
            Command::Reply { native, reply, .. } => {
                let (thread, id) = crate::children::request(native);
                call(
                    Call::Respond {
                        thread,
                        id,
                        decision: codex_answer(reply),
                    },
                    Then::Replied,
                )
            }
        }
    }

    fn routed(&mut self, routed: Routed) -> Vec<Effect> {
        match routed {
            Routed::Codex(event) => {
                let note = codex_note(&event);
                self.lifecycle.steps(codex_steps(event), note)
            }
            Routed::Subagent(steps) => self.lifecycle.steps(steps, None),
            Routed::Stopped { reason } => self.lifecycle.steps(
                Vec::from([Step::Closed(CloseReason::Stopped { reason })]),
                None,
            ),
        }
    }
}

/// The thread a start opens, with Paseo's preset: Codex asks before it
/// leaves the workspace.
fn open(start: Start) -> Open {
    Open {
        resume: start.conversation.map(ThreadId),
        options: ThreadOptions {
            cwd: start.directory.0,
            model: start.model.map(|model| model.0),
            effort: start.effort,
            approval_policy: ApprovalPolicy::OnRequest,
            sandbox: Sandbox::WorkspaceWrite,
            instructions: start.instructions,
        },
    }
}

const fn answered(then: Then, outcome: Result<(), String>) -> Outcome {
    match then {
        Then::Sent => Outcome::Sent(outcome),
        Then::Interrupted => Outcome::Interrupted(outcome),
        Then::Replied => Outcome::Replied(outcome),
    }
}

fn call(call: Call, then: Then) -> Vec<Effect> {
    Vec::from([Effect::Call { call, then }])
}

#[cfg(test)]
mod tests {
    use agents::runtime::{Command, Event, Outcome, Report, Start, Started};
    use agents::{NativeRequest, NativeTurn, Note, Reply, Step};
    use proto::{
        ClientName, CloseReason, Decision as Chosen, Effort, HostPath, ModelId,
        Provider, SessionId,
    };
    use serde_json::json;

    use super::{Call, Effect, Input, Open, Runtime, Then};
    use crate::hub::Routed;
    use crate::{
        ApprovalId, ApprovalPolicy, CodexEvent, Decision, Sandbox, ThreadEvent,
        ThreadId, ThreadOptions, TurnId,
    };

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

    fn running() -> Runtime {
        let mut runtime = Runtime::new();
        let _ = runtime.step(Input::Opened(Ok(ThreadId(text("th_1")))));
        runtime
    }

    #[test]
    fn a_start_resumes_the_saved_conversation_with_the_preset() {
        let mut runtime = Runtime::new();
        let seen = walk(
            &mut runtime,
            Vec::from([
                Input::Command(Command::Start(Start {
                    instructions: None,
                    session: SessionId(1),
                    directory: HostPath(text("/work")),
                    model: Some(ModelId(text("gpt"))),
                    effort: Some(Effort::High),
                    conversation: Some(text("th_1")),
                })),
                Input::Opened(Ok(ThreadId(text("th_1")))),
            ]),
        );
        assert_eq!(
            seen,
            [
                Vec::from([Effect::Open(Open {
                    options: ThreadOptions {
                        instructions: None,
                        cwd: text("/work"),
                        model: Some(text("gpt")),
                        effort: Some(Effort::High),
                        approval_policy: ApprovalPolicy::OnRequest,
                        sandbox: Sandbox::WorkspaceWrite,
                    },
                    resume: Some(ThreadId(text("th_1"))),
                })]),
                outcome(Outcome::Started(Ok(Started {
                    conversation: text("th_1"),
                }))),
            ]
        );
    }

    #[test]
    fn commands_become_requests_on_the_running_thread() {
        let mut runtime = running();
        let seen = walk(
            &mut runtime,
            Vec::from([
                Input::Command(Command::Send {
                    text: text("hi"),
                    effort: Some(Effort::Low),
                }),
                Input::Called {
                    then: Then::Sent,
                    outcome: Ok(()),
                },
                Input::Command(Command::Interrupt(NativeTurn(text("tu_1")))),
                Input::Command(Command::Reply {
                    native: NativeRequest(text("integer:7")),
                    reply: Reply::Decide(Chosen::Deny),
                    by: ClientName(text("phone")),
                }),
            ]),
        );
        assert_eq!(
            seen,
            [
                Vec::from([Effect::Call {
                    call: Call::SendTurn {
                        text: text("hi"),
                        effort: Some(Effort::Low),
                    },
                    then: Then::Sent,
                }]),
                outcome(Outcome::Sent(Ok(()))),
                Vec::from([Effect::Call {
                    call: Call::Interrupt(TurnId(text("tu_1"))),
                    then: Then::Interrupted,
                }]),
                Vec::from([Effect::Call {
                    call: Call::Respond {
                        thread: None,
                        id: ApprovalId(text("integer:7")),
                        decision: Decision::Decline,
                    },
                    then: Then::Replied,
                }]),
            ]
        );
    }

    #[test]
    fn shutdown_releases_the_thread_without_stopping_the_server() {
        assert_eq!(
            running().step(Input::Command(Command::Shutdown)),
            [
                Report::Release.into(),
                Report::Emit(Event::Outcome(Outcome::ShutDown(Ok(())))).into(),
            ],
        );
    }

    #[test]
    fn stopping_releases_the_thread_and_reports_why() {
        assert_eq!(
            running().step(Input::Event(Some(Routed::Stopped {
                reason: text("new Codex credential"),
            }))),
            [
                Report::Emit(Event::Step(Step::Closed(CloseReason::Stopped {
                    reason: text("new Codex credential"),
                })))
                .into(),
                Report::Release.into(),
            ],
        );
    }

    #[test]
    fn unknown_thread_frame_diagnostics_only_keep_the_thread_id() {
        assert_eq!(
            running().step(Input::Event(Some(Routed::Codex(
                CodexEvent::Thread {
                    thread: ThreadId(text("th_1")),
                    event: ThreadEvent::Unknown(json!({ "method": "private-test-marker", "content": "private-test-marker" })),
                }
            )))),
            [Report::Log(Note::UnmappedOnThread {
                provider: Provider::Codex,
                thread: text("th_1"),
                frame: text("unknown"),
            })
            .into()],
        );
    }
}
