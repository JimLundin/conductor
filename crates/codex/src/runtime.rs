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
    Anchoring, Command, Event, Lifecycle, Outcome, Report, Start, Started,
};
use proto::{CloseReason, Effort, HistoryAnchor, ModelId, Models};

use crate::hub::Routed;
use crate::mapping::codex_note;
use crate::prelude::*;
use crate::{
    ApprovalId, ApprovalPolicy, CodexCredential, Decision, Sandbox, ThreadId,
    ThreadOptions, TurnId, codex_answer, codex_steps, credential,
    default_model,
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
        id: ApprovalId,
        decision: Decision,
    },
    Revert(TurnId),
}

/// The command a call's outcome answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Then {
    Sent,
    Interrupted,
    Replied,
    Reverted,
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
    /// The turns [`Effect::Turns`] listed.
    Listed(Result<Vec<TurnId>, String>),
    /// What [`Effect::Models`] listed.
    Models(Result<Models, String>),
    /// How the [`Effect::Credential`] went.
    Configured(Result<(), String>),
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
    /// List the running thread's turns and report [`Input::Listed`].
    Turns,
    /// List the app-server's models and report [`Input::Models`].
    Models,
    /// Run the hub's next app-server with this credential, and report
    /// [`Input::Configured`].
    Credential(Option<CodexCredential>),
    Runtime(Report),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Runtime {
    lifecycle: Lifecycle,
    /// The default model of the credential being set.
    configuring: Option<ModelId>,
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
            configuring: None,
        }
    }

    pub fn step(&mut self, input: Input) -> Vec<Effect> {
        match input {
            Input::Command(command) => self.command(command),
            Input::Opened(Ok(thread)) => self.lifecycle.started(Started {
                conversation: thread.0,
                anchoring: Anchoring::Turns,
            }),
            Input::Opened(Err(error)) => {
                Report::emit(Outcome::Started(Err(error)))
            }
            Input::Called { then, outcome } => {
                Report::emit(answered(then, outcome))
            }
            Input::Listed(listed) => {
                Report::emit(Outcome::Turns(listed.map(|turns| {
                    turns.into_iter().map(|turn| turn.0).collect()
                })))
            }
            Input::Models(listed) => Report::emit(Outcome::Models(listed)),
            Input::Configured(taken) => {
                let default = self.configuring.take();
                Report::emit(Outcome::Configured(taken.map(|()| default)))
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
            Command::Models => Vec::from([Effect::Models]),
            Command::Credential(given) => {
                let codex = credential(given.as_ref());
                self.configuring = codex.as_ref().map(default_model);
                Vec::from([Effect::Credential(codex)])
            }
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
            Command::Reply { native, reply, .. } => call(
                Call::Respond {
                    id: ApprovalId(native.0),
                    decision: codex_answer(reply),
                },
                Then::Replied,
            ),
            Command::Turns => Vec::from([Effect::Turns]),
            Command::Revert(turn) => {
                call(Call::Revert(TurnId(turn)), Then::Reverted)
            }
        }
    }

    fn routed(&mut self, routed: Routed) -> Vec<Effect> {
        match routed {
            Routed::Codex(event) => {
                let note = codex_note(&event);
                self.lifecycle.steps(codex_steps(event), note)
            }
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
        resume: resume(start.history, start.conversation),
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

/// The thread to resume: the one `history` names, else the saved
/// `conversation`; a fresh history opens a new thread.
#[must_use]
pub fn resume(
    history: Option<HistoryAnchor>,
    conversation: Option<String>,
) -> Option<ThreadId> {
    match history {
        Some(HistoryAnchor::Fresh) => None,
        Some(HistoryAnchor::Turns {
            conversation: native,
            ..
        }) => Some(native),
        _ => conversation,
    }
    .map(ThreadId)
}

const fn answered(then: Then, outcome: Result<(), String>) -> Outcome {
    match then {
        Then::Sent => Outcome::Sent(outcome),
        Then::Interrupted => Outcome::Interrupted(outcome),
        Then::Replied => Outcome::Replied(outcome),
        Then::Reverted => Outcome::Reverted(outcome),
    }
}

fn call(call: Call, then: Then) -> Vec<Effect> {
    Vec::from([Effect::Call { call, then }])
}

#[cfg(test)]
mod tests {
    use agents::runtime::{
        Anchoring, Command, Event, Outcome, Report, Start, Started,
    };
    use agents::{NativeRequest, NativeTurn, Note, Reply, Step};
    use proto::{
        ClientName, CloseReason, Credential, Decision as Chosen, Effort,
        HistoryAnchor, HostPath, ModelId, Provider, SessionId,
    };
    use serde_json::json;

    use super::{Call, Effect, Input, Open, Runtime, Then, resume};
    use crate::hub::Routed;
    use crate::{
        ApprovalId, ApprovalPolicy, CodexCredential, CodexEvent, Decision,
        Sandbox, ThreadEvent, ThreadId, ThreadOptions, TurnId,
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
    fn a_start_opens_the_thread_its_history_names_with_the_preset() {
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
                    history: Some(HistoryAnchor::Turns {
                        conversation: text("th_1"),
                        turn: Some(text("tu_1")),
                    }),
                    conversation: Some(text("saved")),
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
                    anchoring: Anchoring::Turns,
                }))),
            ]
        );
    }

    #[test]
    fn the_thread_resumed_follows_the_history_then_the_saved_one() {
        let fork = HistoryAnchor::Fork {
            source: text("a"),
            through: text("b"),
            target: text("c"),
        };
        assert_eq!(
            [
                resume(Some(HistoryAnchor::Fresh), Some(text("saved"))),
                resume(Some(fork), Some(text("saved"))),
                resume(None, None),
            ],
            [None, Some(ThreadId(text("saved"))), None]
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
                    native: NativeRequest(text("7")),
                    reply: Reply::Decide(Chosen::Deny),
                    by: ClientName(text("phone")),
                }),
                Input::Command(Command::Turns),
                Input::Listed(Ok(Vec::from([TurnId(text("tu_1"))]))),
                Input::Command(Command::Revert(text("tu_1"))),
                Input::Called {
                    then: Then::Reverted,
                    outcome: Err(text("busy")),
                },
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
                        id: ApprovalId(text("7")),
                        decision: Decision::Decline,
                    },
                    then: Then::Replied,
                }]),
                Vec::from([Effect::Turns]),
                outcome(Outcome::Turns(Ok(Vec::from([text("tu_1")])))),
                Vec::from([Effect::Call {
                    call: Call::Revert(TurnId(text("tu_1"))),
                    then: Then::Reverted,
                }]),
                outcome(Outcome::Reverted(Err(text("busy")))),
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
    fn unknown_thread_frames_keep_the_thread_id() {
        assert_eq!(
            running().step(Input::Event(Some(Routed::Codex(
                CodexEvent::Thread {
                    thread: ThreadId(text("th_1")),
                    event: ThreadEvent::Unknown(json!({ "method": "x" })),
                }
            )))),
            [Report::Log(Note::UnmappedOnThread {
                provider: Provider::Codex,
                thread: text("th_1"),
                frame: text("{\"method\":\"x\"}"),
            })
            .into()],
        );
    }

    #[test]
    fn credentials_reach_the_hub_and_report_the_model_they_name() {
        let gateway = CodexCredential::Provider {
            name: text("gateway"),
            base_url: text("https://gateway.example"),
            key: text("secret"),
            model: text("gw-model"),
        };
        let stored = Credential {
            kind: text("provider"),
            values: [
                ("name", "gateway"),
                ("base_url", "https://gateway.example"),
                ("key", "secret"),
                ("model", "gw-model"),
            ]
            .map(|(id, value)| (text(id), text(value)))
            .into(),
        };
        let mut runtime = Runtime::new();
        assert_eq!(
            walk(
                &mut runtime,
                Vec::from([
                    Input::Command(Command::Credential(Some(stored))),
                    Input::Configured(Ok(())),
                    Input::Command(Command::Credential(None)),
                    Input::Configured(Ok(())),
                ]),
            ),
            Vec::from([
                Vec::from([Effect::Credential(Some(gateway))]),
                outcome(Outcome::Configured(Ok(Some(ModelId(text(
                    "gw-model"
                )))))),
                Vec::from([Effect::Credential(None)]),
                outcome(Outcome::Configured(Ok(None))),
            ])
        );
    }
}
