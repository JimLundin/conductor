//! What a Claude agent does with a session's runtime commands
//! (`agents::runtime`), as a state machine.
//!
//! It decides which conversation the CLI starts on and which it tries
//! next, the control calls each command becomes, and the runtime events
//! each Claude event becomes.
//!
//! `claude-cli` runs it: it turns each command, call outcome and Claude
//! event into an [`Input`], and performs each [`Effect`] in order.

use agents::runtime::{Anchoring, Command, Event, Outcome, Start, Started};
use agents::{Note, Step};
use proto::{
    CloseReason, Effort, HistoryAnchor, HostPath, Models, Provider,
    SessionId as Session,
};

use crate::prelude::*;
use crate::{
    ClaudeCredential, ClaudeEvent, MessageId, PermissionDecision,
    PermissionRequestId, Resume, Sandbox, SessionId, claude_answer,
    claude_steps, credential, unprotected,
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
    /// What the [`Effect::Probe`] listed.
    Probed(Result<Models, String>),
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
    /// List the models a CLI offers in a directory of its own, and report
    /// [`Input::Probed`].
    Probe,
    /// Later CLIs start with this credential.
    Credential(Option<ClaudeCredential>),
    /// Drop the CLI: its process has gone.
    Release,
    Emit(Event),
    Log(Note),
}

/// The start in flight: the resume it tries and those left if it finds
/// its conversation missing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Starting {
    spawn: Spawn,
    rest: Vec<Resume>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    sandbox: Sandbox,
    running: bool,
    starting: Option<Starting>,
}

const NOT_RUNNING: &str = "the agent is not running";

impl Runtime {
    /// A runtime with no CLI running yet, on a host where Claude's sandbox
    /// is `sandbox`.
    #[must_use]
    pub const fn new(sandbox: Sandbox) -> Self {
        Self {
            sandbox,
            running: false,
            starting: None,
        }
    }

    pub fn step(&mut self, input: Input) -> Vec<Effect> {
        match input {
            Input::Command(command) => self.command(command),
            Input::Spawned(spawned) => self.spawned(spawned),
            Input::Called { then, outcome } => called(then, outcome),
            Input::ShutDown(outcome) => {
                self.running = false;
                emit(Outcome::ShutDown(outcome))
            }
            Input::Probed(listed) => emit(Outcome::Models(listed)),
            Input::Event(Some(event)) => self.event(event),
            Input::Event(None) => self.ended(),
        }
    }

    fn command(&mut self, command: Command) -> Vec<Effect> {
        match (command, self.running) {
            (Command::Start(start), _) => self.start(start),
            (Command::Models, _) => Vec::from([Effect::Probe]),
            (Command::Credentials(credentials), _) => {
                let claude = credential(&credentials);
                let note = unprotected(claude.as_ref(), self.sandbox);
                core::iter::once(Effect::Credential(claude))
                    .chain(note.map(Effect::Log))
                    .chain(emit(Outcome::Configured(Ok(None))))
                    .collect()
            }
            (Command::Shutdown, true) => Vec::from([Effect::Shutdown]),
            (Command::Shutdown, false) => emit(Outcome::ShutDown(Ok(()))),
            (Command::Send { text, effort }, true) => {
                Vec::from([send(text, effort)])
            }
            (Command::Interrupt(_), true) => Vec::from([Effect::Call {
                call: Call::Interrupt,
                then: Then::Interrupted,
            }]),
            (Command::Reply { native, reply, by }, true) => {
                Vec::from([Effect::Call {
                    call: Call::Respond {
                        id: PermissionRequestId(native.0),
                        decision: claude_answer(reply, &by),
                    },
                    then: Then::Replied,
                }])
            }
            (refused @ (Command::Turns | Command::Revert(_)), _) => {
                emit(refused.failed("Claude keeps no turns to revert"))
            }
            (
                idle @ (Command::Send { .. }
                | Command::Interrupt(_)
                | Command::Reply { .. }),
                false,
            ) => emit(idle.failed(NOT_RUNNING)),
        }
    }

    fn start(&mut self, start: Start) -> Vec<Effect> {
        let Start {
            session,
            directory,
            model,
            effort,
            history,
            conversation,
        } = start;
        let (resume, rest) = attempts(history, conversation);
        let spawn = Spawn {
            session,
            directory,
            model: model.map(|chosen| chosen.0),
            effort,
            resume,
        };
        self.starting = Some(Starting {
            spawn: spawn.clone(),
            rest,
        });
        Vec::from([Effect::Spawn(spawn)])
    }

    /// A CLI that finds the conversation it resumes missing starts on the
    /// next resume, if there is one.
    fn spawned(&mut self, spawned: Result<SessionId, Refusal>) -> Vec<Effect> {
        let starting = self.starting.take();
        match (spawned, starting) {
            (Ok(id), _) => {
                self.running = true;
                emit(Outcome::Started(Ok(Started {
                    conversation: id.0,
                    anchoring: Anchoring::Messages,
                })))
            }
            (Err(refusal), Some(Starting { spawn, rest }))
                if refusal.missing.is_some() && !rest.is_empty() =>
            {
                let mut left = rest.into_iter();
                let next = Spawn {
                    resume: left.next().unwrap_or(Resume::Fresh),
                    ..spawn
                };
                self.starting = Some(Starting {
                    spawn: next.clone(),
                    rest: left.collect(),
                });
                Vec::from([Effect::Spawn(next)])
            }
            (Err(refusal), _) => emit(Outcome::Started(Err(refusal.message))),
        }
    }

    /// Each event's steps, and the CLI's exit ends it.
    fn event(&mut self, event: ClaudeEvent) -> Vec<Effect> {
        let gone = matches!(event, ClaudeEvent::Exited { .. });
        let note = match &event {
            ClaudeEvent::Unknown(frame) => Some(Effect::Log(Note::Unmapped {
                provider: Provider::Claude,
                frame: frame.to_string(),
            })),
            _ => None,
        };
        let steps = claude_steps(event)
            .into_iter()
            .map(|step| Effect::Emit(Event::Step(step)));
        let released = gone.then(|| self.release());
        note.into_iter().chain(steps).chain(released).collect()
    }

    /// Events that end without the CLI's exit end it too.
    fn ended(&mut self) -> Vec<Effect> {
        if self.running {
            Vec::from([
                Effect::Emit(Event::Step(Step::Closed(CloseReason::Exited {
                    status: String::from("the agent's events ended"),
                }))),
                self.release(),
            ])
        } else {
            Vec::new()
        }
    }

    const fn release(&mut self) -> Effect {
        self.running = false;
        Effect::Release
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
        (Then::Retuned(_), Err(error)) => emit(Outcome::Sent(Err(error))),
        (Then::Sent, sent) => emit(Outcome::Sent(sent)),
        (Then::Interrupted, interrupted) => {
            emit(Outcome::Interrupted(interrupted))
        }
        (Then::Replied, replied) => emit(Outcome::Replied(replied)),
    }
}

fn emit(outcome: Outcome) -> Vec<Effect> {
    Vec::from([Effect::Emit(Event::Outcome(outcome))])
}

/// The resumes to try in turn when starting at `history`, else on the
/// saved `conversation`; each after the first only when the one before
/// finds its conversation missing.
///
/// A fork whose target already exists resumes the target instead. A
/// conversation in which no message was seen may not have been written
/// yet, so when it is missing the CLI starts fresh.
#[must_use]
pub fn attempts(
    history: Option<HistoryAnchor>,
    conversation: Option<String>,
) -> (Resume, Vec<Resume>) {
    let unseen = matches!(
        history,
        None | Some(HistoryAnchor::Messages { message: None, .. })
    );
    match resume(history, conversation) {
        Resume::Fork {
            session,
            through,
            target,
        } => (
            Resume::Session(target.clone()),
            Vec::from([Resume::Fork {
                session,
                through,
                target,
            }]),
        ),
        Resume::Session(session) if unseen => {
            (Resume::Session(session), Vec::from([Resume::Fresh]))
        }
        first => (first, Vec::new()),
    }
}

fn resume(
    history: Option<HistoryAnchor>,
    conversation: Option<String>,
) -> Resume {
    match history {
        Some(HistoryAnchor::Fresh) => Resume::Fresh,
        Some(HistoryAnchor::Fork {
            source,
            through,
            target,
        }) => Resume::Fork {
            session: SessionId(source),
            through: MessageId(through),
            target: SessionId(target),
        },
        Some(HistoryAnchor::Messages {
            conversation: native,
            ..
        }) => Resume::Session(SessionId(native)),
        _ => conversation
            .map(SessionId)
            .map_or(Resume::Fresh, Resume::Session),
    }
}

#[cfg(test)]
mod tests {
    use agents::runtime::{Anchoring, Command, Event, Outcome, Start, Started};
    use agents::{Exit, NativeRequest, NativeTurn, Note, Reply, Step};
    use proto::{
        ClientName, CloseReason, Credential, Credentials, Decision, Effort,
        HistoryAnchor, HostPath, ModelId, Provider, SessionId as Session,
    };
    use serde_json::json;

    use super::{Call, Effect, Input, Refusal, Runtime, Spawn, Then, attempts};
    use crate::{
        ClaudeCredential, ClaudeEvent, MessageId, PermissionDecision,
        PermissionRequestId, Resume, Sandbox, SessionId,
    };

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
        Vec::from([Effect::Emit(Event::Outcome(outcome))])
    }

    fn start(history: Option<HistoryAnchor>) -> Input {
        Input::Command(Command::Start(Start {
            session: Session(1),
            directory: HostPath(text("/work")),
            model: Some(ModelId(text("haiku"))),
            effort: Some(Effort::Low),
            history,
            conversation: Some(text("saved")),
        }))
    }

    fn spawn(resume: Resume) -> Effect {
        Effect::Spawn(Spawn {
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
            anchoring: Anchoring::Messages,
        })))
    }

    fn running() -> Runtime {
        let mut runtime = Runtime::new(Sandbox::Available);
        let _ = walk(
            &mut runtime,
            Vec::from([start(None), Input::Spawned(Ok(id("saved")))]),
        );
        runtime
    }

    #[test]
    fn a_missing_conversation_with_no_message_seen_starts_fresh() {
        let mut runtime = Runtime::new(Sandbox::Available);
        let seen = walk(
            &mut runtime,
            Vec::from([
                start(Some(HistoryAnchor::Messages {
                    conversation: text("conv"),
                    message: None,
                })),
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
    fn a_fork_whose_target_exists_resumes_the_target_and_else_forks() {
        let mut runtime = Runtime::new(Sandbox::Available);
        let fork = Resume::Fork {
            session: id("source"),
            through: MessageId(text("boundary")),
            target: id("target"),
        };
        let seen = walk(
            &mut runtime,
            Vec::from([
                start(Some(HistoryAnchor::Fork {
                    source: text("source"),
                    through: text("boundary"),
                    target: text("target"),
                })),
                missing("target"),
                missing("target"),
            ]),
        );
        assert_eq!(
            seen,
            [
                Vec::from([spawn(Resume::Session(id("target")))]),
                Vec::from([spawn(fork)]),
                outcome(Outcome::Started(Err(text("missing")))),
            ]
        );
    }

    #[test]
    fn the_resumes_follow_the_history_then_the_saved_conversation() {
        let messages = |message: Option<&str>| {
            Some(HistoryAnchor::Messages {
                conversation: text("conv"),
                message: message.map(String::from),
            })
        };
        assert_eq!(
            [
                attempts(Some(HistoryAnchor::Fresh), Some(text("saved"))),
                attempts(messages(Some("m1")), None),
                attempts(None, Some(text("saved"))),
                attempts(None, None),
                attempts(
                    Some(HistoryAnchor::Turns {
                        conversation: text("thread"),
                        turn: None,
                    }),
                    Some(text("saved"))
                ),
            ],
            [
                (Resume::Fresh, Vec::new()),
                (Resume::Session(id("conv")), Vec::new()),
                (Resume::Session(id("saved")), Vec::from([Resume::Fresh])),
                (Resume::Fresh, Vec::new()),
                (Resume::Session(id("saved")), Vec::new()),
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
    fn calls_reach_the_running_cli_and_fail_without_one() {
        let interrupt = || Command::Interrupt(NativeTurn(text("saved")));
        let reply = || Command::Reply {
            native: NativeRequest(text("perm-1")),
            reply: Reply::Decide(Decision::Approve),
            by: ClientName(text("phone")),
        };
        let mut idle = Runtime::new(Sandbox::Available);
        let mut runtime = running();
        let not_running = text("the agent is not running");
        assert_eq!(
            (
                walk(
                    &mut idle,
                    Vec::from([
                        Input::Command(interrupt()),
                        Input::Command(Command::Shutdown),
                    ])
                ),
                walk(
                    &mut runtime,
                    Vec::from([
                        Input::Command(interrupt()),
                        Input::Command(reply()),
                        Input::Command(Command::Turns),
                        Input::Command(Command::Shutdown),
                        Input::ShutDown(Ok(())),
                        Input::Command(reply()),
                    ])
                ),
            ),
            (
                Vec::from([
                    outcome(Outcome::Interrupted(Err(not_running.clone()))),
                    outcome(Outcome::ShutDown(Ok(()))),
                ]),
                Vec::from([
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
                    outcome(Outcome::Turns(Err(text(
                        "Claude keeps no turns to revert"
                    )))),
                    Vec::from([Effect::Shutdown]),
                    outcome(Outcome::ShutDown(Ok(()))),
                    outcome(Outcome::Replied(Err(not_running))),
                ]),
            )
        );
    }

    #[test]
    fn events_become_steps_and_the_exit_releases_the_cli() {
        let mut runtime = running();
        let closed = |status: &str| {
            Effect::Emit(Event::Step(Step::Closed(CloseReason::Exited {
                status: text(status),
            })))
        };
        let seen = walk(
            &mut runtime,
            Vec::from([
                Input::Event(Some(ClaudeEvent::Unknown(json!("x")))),
                Input::Event(Some(ClaudeEvent::Exited {
                    status: Exit::Status(text("exit status: 1")),
                })),
                Input::Event(None),
            ]),
        );
        let mut again = running();
        assert_eq!(
            (seen, again.step(Input::Event(None))),
            (
                Vec::from([
                    Vec::from([Effect::Log(Note::Unmapped {
                        provider: Provider::Claude,
                        frame: text("\"x\""),
                    })]),
                    Vec::from([closed("exit status: 1"), Effect::Release]),
                    Vec::new(),
                ]),
                Vec::from([
                    closed("the agent's events ended"),
                    Effect::Release
                ]),
            )
        );
    }

    #[test]
    fn credentials_reach_later_clis_and_an_unsandboxed_gateway_is_warned() {
        let gateway = ClaudeCredential::Gateway {
            base_url: text("https://gateway.example"),
            token: text("secret"),
        };
        let credentials = Credentials::none().set(Credential {
            provider: Provider::Claude,
            kind: text("gateway"),
            values: [
                ("base_url", "https://gateway.example"),
                ("token", "secret"),
            ]
            .map(|(id, value)| (text(id), text(value)))
            .into(),
        });
        let mut runtime = Runtime::new(Sandbox::Missing);
        let seen =
            runtime.step(Input::Command(Command::Credentials(credentials)));
        assert_eq!(
            seen,
            Vec::from([
                Effect::Credential(Some(gateway)),
                Effect::Log(Note::GatewayExposed {
                    provider: Provider::Claude
                }),
                Effect::Emit(Event::Outcome(Outcome::Configured(Ok(None)))),
            ])
        );
    }
}
