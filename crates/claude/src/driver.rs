//! The decisions of the actor that owns one `claude` process, as a state
//! machine.
//!
//! `claude-cli` owns the process: it turns each stdout line and each
//! handle's command into an [`Input`], and performs each [`Effect`]. A
//! reply's channel is the opaque `R` and a shutdown's the opaque `S`, which
//! the machine hands back with the answer; `E` is why a write failed.

use core::fmt;

use agents::{Exit, Note};
use proto::Provider;
use serde_json::Value;

use alloc::collections::BTreeSet;

use crate::prelude::*;
use crate::{
    ClaudeEvent, ControlCall, Inbound, MessageId, Open, Outbound,
    PermissionDecision, PermissionRequestId, encode, parse_line,
};

/// What a handle asks of the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command<R, S> {
    SendUser {
        text: String,
        reply: R,
    },
    Control {
        call: ControlCall,
        reply: R,
    },
    Respond {
        id: PermissionRequestId,
        decision: PermissionDecision,
        reply: R,
    },
    /// Closes stdin, waits briefly for a clean exit, then kills.
    Shutdown(S),
}

/// What happened, as the shell saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input<R, S, E> {
    /// A stdout line, `Ok(None)` once stdout closes, or why reading it
    /// failed.
    Read(Result<Option<String>, String>),
    Command(Command<R, S>),
    /// Every handle is gone.
    Closed,
    /// How the [`Effect::Write`] for `then` went.
    Wrote {
        then: Then<R>,
        outcome: Result<(), E>,
    },
    /// The process [`Effect::Stop`] stopped has exited.
    Stopped {
        exit: Exit,
        then: After<S>,
    },
}

/// What a line is written for, and so what its outcome decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then<R> {
    User(R),
    Control {
        request_id: String,
        call: ControlCall,
        reply: R,
    },
    Answer(R),
    /// The error reply to a control request of this subtype we don't serve.
    Refusal {
        subtype: String,
    },
}

/// What follows once the process has stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum After<S> {
    Nothing,
    Shutdown(S),
    /// Every handle is gone, so the actor ends.
    End,
}

/// What the shell does next, in order.
#[derive(Debug, PartialEq)]
pub enum Effect<R, S, E> {
    /// Write `line` to stdin, then report how it went as [`Input::Wrote`].
    Write {
        line: String,
        then: Then<R>,
    },
    Emit(ClaudeEvent),
    Log(Note),
    /// Answer a handle's command; a command with no value answers `Null`.
    Reply(R, Result<Value, Failure<E>>),
    /// Answer a shutdown with how the process ended.
    Report(S, Exit),
    /// Stop the process, then report its exit as [`Input::Stopped`].
    Stop(After<S>),
    /// End the actor.
    End,
}

/// Why a command failed.
#[derive(Debug)]
pub enum Failure<E> {
    Exited,
    Io(E),
    Encode(serde_json::Error),
    Control { subtype: String, message: String },
    UnknownPermissionRequest(PermissionRequestId),
}

/// An encoding failure equals another with the same description.
impl<E: PartialEq> PartialEq for Failure<E> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Exited, Self::Exited) => true,
            (Self::Io(left), Self::Io(right)) => left == right,
            (Self::Encode(left), Self::Encode(right)) => {
                left.to_string() == right.to_string()
            }
            (
                Self::Control { subtype, message },
                Self::Control {
                    subtype: other_subtype,
                    message: other_message,
                },
            ) => subtype == other_subtype && message == other_message,
            (
                Self::UnknownPermissionRequest(left),
                Self::UnknownPermissionRequest(right),
            ) => left == right,
            _ => false,
        }
    }
}

impl<E: fmt::Display> fmt::Display for Failure<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exited => f.write_str("the claude process has exited"),
            Self::Io(error) => write!(f, "claude i/o: {error}"),
            Self::Encode(error) => {
                write!(f, "could not encode a message for claude: {error}")
            }
            Self::Control { subtype, message } => {
                write!(f, "claude rejected {subtype}: {message}")
            }
            Self::UnknownPermissionRequest(id) => {
                write!(f, "no open permission request with id {}", id.0)
            }
        }
    }
}

/// The session's state: our control requests awaiting a `control_response`,
/// the `can_use_tool` requests not yet answered, the last message of the
/// running turn, and the sub-agents the process started.
#[derive(Debug, Clone, PartialEq)]
pub struct Session<R> {
    /// How the process ended, once it has.
    exited: Option<Exit>,
    /// Our control requests by `request_id`, with who waits for each.
    pending: BTreeMap<String, (ControlCall, R)>,
    /// What an answer to each open `can_use_tool` request needs.
    open: BTreeMap<PermissionRequestId, Open>,
    next_request: u64,
    last_message: Option<MessageId>,
    /// Every task the CLI reported as a sub-agent. A task's end doesn't
    /// say what the task was.
    subagents: BTreeSet<String>,
}

impl<R> Default for Session<R> {
    fn default() -> Self {
        Self {
            exited: None,
            pending: BTreeMap::new(),
            open: BTreeMap::new(),
            next_request: 0,
            last_message: None,
            subagents: BTreeSet::new(),
        }
    }
}

impl<R> Session<R> {
    /// A session whose process is running.
    #[must_use]
    pub fn running() -> Self {
        Self::default()
    }

    pub fn step<S, E: fmt::Display>(
        &mut self,
        input: Input<R, S, E>,
    ) -> Vec<Effect<R, S, E>> {
        match input {
            Input::Read(Ok(Some(line))) => self.line(&line),
            Input::Read(Ok(None)) => self.stop(After::Nothing),
            Input::Read(Err(error)) => {
                let mut effects = Vec::from([Effect::Log(Note::Unreadable {
                    provider: Provider::Claude,
                    error,
                })]);
                effects.extend(self.stop(After::Nothing));
                effects
            }
            Input::Command(command) => self.command(command),
            Input::Closed => self.stop(After::End),
            Input::Wrote { then, outcome } => self.wrote(then, outcome),
            Input::Stopped { exit, then } => self.stopped(&exit, then),
        }
    }

    fn line<S, E: fmt::Display>(&mut self, line: &str) -> Vec<Effect<R, S, E>> {
        match parse_line(line) {
            Ok(inbound) => self.inbound(inbound),
            Err(error) => Vec::from([
                Effect::Log(Note::NotJson {
                    provider: Provider::Claude,
                    error: error.to_string(),
                    line: line.to_owned(),
                }),
                Effect::Emit(ClaudeEvent::Unknown(Value::String(
                    line.to_owned(),
                ))),
            ]),
        }
    }

    fn inbound<S, E: fmt::Display>(
        &mut self,
        inbound: Inbound,
    ) -> Vec<Effect<R, S, E>> {
        match inbound {
            Inbound::Event(received) => {
                Vec::from([Effect::Emit(self.event(received))])
            }
            Inbound::Permission { id, open, event } => {
                self.open.insert(id, open);
                Vec::from([Effect::Emit(event)])
            }
            Inbound::ControlResponse {
                request_id,
                outcome,
            } => self.answered(&request_id, outcome),
            Inbound::UnsupportedControlRequest {
                request_id,
                subtype,
            } => {
                let message =
                    format!("unsupported control request subtype: {subtype}");
                let mut effects = Vec::from([Effect::Log(Note::Unsupported {
                    provider: Provider::Claude,
                    request: subtype.clone(),
                })]);
                effects.extend(self.write(
                    Outbound::ControlError {
                        request_id,
                        message,
                    },
                    Then::Refusal { subtype },
                ));
                effects
            }
        }
    }

    /// Tracks the turn's last message, which its result carries, the
    /// requests the CLI withdraws, and the sub-agents. The end of a task that
    /// is no sub-agent, such as a background command, is `Unknown`.
    fn event(&mut self, received: ClaudeEvent) -> ClaudeEvent {
        match received {
            ClaudeEvent::Message {
                role,
                content,
                uuid,
            } => {
                self.last_message.clone_from(&uuid);
                ClaudeEvent::Message {
                    role,
                    content,
                    uuid,
                }
            }
            ClaudeEvent::TurnFinished {
                subtype,
                outcome,
                terminal_reason,
                session_id,
                errors,
                last_message: _,
            } => ClaudeEvent::TurnFinished {
                subtype,
                outcome,
                terminal_reason,
                session_id,
                errors,
                last_message: self.last_message.clone(),
            },
            ClaudeEvent::PermissionCancelled { id } => {
                self.open.remove(&id);
                ClaudeEvent::PermissionCancelled { id }
            }
            ClaudeEvent::SubagentStarted {
                agent,
                tool_use_id,
                description,
            } => {
                self.subagents.insert(agent.clone());
                ClaudeEvent::SubagentStarted {
                    agent,
                    tool_use_id,
                    description,
                }
            }
            ClaudeEvent::SubagentEnded { agent, .. }
                if !self.subagents.contains(&agent) =>
            {
                ClaudeEvent::Unknown(Value::String(format!(
                    "task_notification for task {agent}, which is no sub-agent"
                )))
            }
            other => other,
        }
    }

    fn answered<S, E>(
        &mut self,
        request_id: &str,
        outcome: Result<Value, String>,
    ) -> Vec<Effect<R, S, E>> {
        self.pending.remove(request_id).map_or_else(
            || {
                Vec::from([Effect::Log(Note::UnknownResponse {
                    provider: Provider::Claude,
                    request: request_id.to_owned(),
                })])
            },
            |(call, reply)| {
                Vec::from([Effect::Reply(
                    reply,
                    outcome.map_err(|message| Failure::Control {
                        subtype: String::from(call.subtype()),
                        message,
                    }),
                )])
            },
        )
    }

    fn command<S, E: fmt::Display>(
        &mut self,
        command: Command<R, S>,
    ) -> Vec<Effect<R, S, E>> {
        match command {
            Command::SendUser { text, reply } => {
                self.write(Outbound::User(text), Then::User(reply))
            }
            Command::Control { call, reply } => {
                self.next_request = self.next_request.saturating_add(1);
                let request_id = format!("req_{}", self.next_request);
                self.write(
                    Outbound::Control {
                        request_id: request_id.clone(),
                        call,
                    },
                    Then::Control {
                        request_id,
                        call,
                        reply,
                    },
                )
            }
            Command::Respond {
                id,
                decision,
                reply,
            } => match self.open.remove(&id) {
                None => Vec::from([Effect::Reply(
                    reply,
                    Err(Failure::UnknownPermissionRequest(id)),
                )]),
                Some(open) => self.write(
                    Outbound::Permission { id, decision, open },
                    Then::Answer(reply),
                ),
            },
            Command::Shutdown(waiter) => self.stop(After::Shutdown(waiter)),
        }
    }

    fn write<S, E: fmt::Display>(
        &self,
        msg: Outbound,
        then: Then<R>,
    ) -> Vec<Effect<R, S, E>> {
        match (&self.exited, encode(msg)) {
            (Some(_), _) => failed(then, Failure::Exited),
            (None, Ok(line)) => Vec::from([Effect::Write {
                line: format!("{line}\n"),
                then,
            }]),
            (None, Err(error)) => failed(then, Failure::Encode(error)),
        }
    }

    fn wrote<S, E: fmt::Display>(
        &mut self,
        then: Then<R>,
        outcome: Result<(), E>,
    ) -> Vec<Effect<R, S, E>> {
        match (then, outcome) {
            (Then::User(reply), Ok(())) => {
                self.last_message = None;
                Vec::from([Effect::Reply(reply, Ok(Value::Null))])
            }
            (
                Then::Control {
                    request_id,
                    call,
                    reply,
                },
                Ok(()),
            ) => {
                self.pending.insert(request_id, (call, reply));
                Vec::new()
            }
            (Then::Answer(reply), Ok(())) => {
                Vec::from([Effect::Reply(reply, Ok(Value::Null))])
            }
            (Then::Refusal { .. }, Ok(())) => Vec::new(),
            (unwritten, Err(error)) => failed(unwritten, Failure::Io(error)),
        }
    }

    fn stop<S, E>(&self, then: After<S>) -> Vec<Effect<R, S, E>> {
        match &self.exited {
            None => Vec::from([Effect::Stop(then)]),
            Some(exit) => after(then, exit),
        }
    }

    /// Fails everything still waiting on the process.
    fn stopped<S, E>(
        &mut self,
        exit: &Exit,
        then: After<S>,
    ) -> Vec<Effect<R, S, E>> {
        self.exited = Some(exit.clone());
        self.open.clear();
        let pending = core::mem::take(&mut self.pending);
        pending
            .into_values()
            .map(|(_, reply)| Effect::Reply(reply, Err(Failure::Exited)))
            .chain([Effect::Emit(ClaudeEvent::Exited {
                status: exit.clone(),
            })])
            .chain(after(then, exit))
            .collect()
    }
}

fn after<R, S, E>(then: After<S>, exit: &Exit) -> Vec<Effect<R, S, E>> {
    match then {
        After::Nothing => Vec::new(),
        After::Shutdown(waiter) => {
            Vec::from([Effect::Report(waiter, exit.clone())])
        }
        After::End => Vec::from([Effect::End]),
    }
}

/// A write for `then` that could not happen.
fn failed<R, S, E: fmt::Display>(
    then: Then<R>,
    failure: Failure<E>,
) -> Vec<Effect<R, S, E>> {
    match then {
        Then::User(reply)
        | Then::Answer(reply)
        | Then::Control { reply, .. } => {
            Vec::from([Effect::Reply(reply, Err(failure))])
        }
        Then::Refusal { subtype } => {
            Vec::from([Effect::Log(Note::Unwritten {
                provider: Provider::Claude,
                writing: format!("refusing {subtype}"),
                error: failure.to_string(),
            })])
        }
    }
}

#[cfg(test)]
mod tests {
    use agents::{Exit, Note};
    use proto::Provider;
    use serde_json::{Value, json};

    use super::{After, Command, Effect, Failure, Input, Session, Then};
    use crate::{
        ClaudeEvent, ControlCall, MessageId, Outbound, PermissionDecision,
        PermissionRequestId, encode,
    };

    type Step = Effect<u8, char, &'static str>;

    fn walk(
        session: &mut Session<u8>,
        inputs: Vec<Input<u8, char, &'static str>>,
    ) -> Vec<Vec<Step>> {
        inputs
            .into_iter()
            .map(|input| session.step(input))
            .collect()
    }

    fn line(value: &Value) -> Input<u8, char, &'static str> {
        Input::Read(Ok(Some(value.to_string())))
    }

    fn task(subtype: &str, id: &str, task_type: &str) -> Value {
        json!({
            "type": "system",
            "subtype": subtype,
            "task_id": id,
            "tool_use_id": "toolu_1",
            "description": "look around",
            "task_type": task_type,
            "status": "completed",
            "summary": "",
            "session_id": "s1",
        })
    }

    #[test]
    fn only_a_sub_agents_task_ends_as_a_sub_agent() {
        let mut session = Session::<u8>::running();
        let ended = |effects: &[Step]| match effects {
            [Effect::Emit(ClaudeEvent::SubagentStarted { agent, .. })] => {
                format!("started {agent}")
            }
            [Effect::Emit(ClaudeEvent::SubagentEnded { agent, .. })] => {
                format!("ended {agent}")
            }
            [Effect::Emit(ClaudeEvent::Unknown(_))] => String::from("unknown"),
            other => format!("{other:?}"),
        };
        let seen = walk(
            &mut session,
            Vec::from([
                line(&task("task_started", "a1", "local_agent")),
                line(&task("task_started", "b1", "local_bash")),
                line(&task("task_notification", "b1", "local_bash")),
                line(&task("task_notification", "a1", "local_agent")),
            ]),
        );
        assert_eq!(
            seen.iter()
                .map(|effects| ended(effects))
                .collect::<Vec<String>>(),
            ["started a1", "unknown", "unknown", "ended a1"].map(String::from)
        );
    }

    fn written(msg: Outbound, then: Then<u8>) -> Step {
        Effect::Write {
            line: encode(msg)
                .map_or_else(|error| error.to_string(), |line| line + "\n"),
            then,
        }
    }

    fn interrupt(request_id: &str) -> Outbound {
        Outbound::Control {
            request_id: String::from(request_id),
            call: ControlCall::Interrupt,
        }
    }

    fn exit() -> Exit {
        Exit::Status(String::from("exit status: 0"))
    }

    fn control(reply: u8) -> Input<u8, char, &'static str> {
        Input::Command(Command::Control {
            call: ControlCall::Interrupt,
            reply,
        })
    }

    fn sent(request_id: &str, reply: u8) -> Then<u8> {
        Then::Control {
            request_id: String::from(request_id),
            call: ControlCall::Interrupt,
            reply,
        }
    }

    fn permission(id: &str) -> Value {
        json!({
            "type": "control_request",
            "request_id": id,
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "Bash",
                "input": { "command": "ls" },
                "tool_use_id": "toolu_1",
            },
        })
    }

    #[test]
    fn a_control_request_waits_for_its_response_by_request_id() {
        let mut session = Session::running();
        let seen = walk(
            &mut session,
            Vec::from([
                control(1),
                Input::Wrote {
                    then: sent("req_1", 1),
                    outcome: Ok(()),
                },
                control(2),
                Input::Wrote {
                    then: sent("req_2", 2),
                    outcome: Ok(()),
                },
                line(&json!({
                    "type": "control_response",
                    "response": {
                        "subtype": "error",
                        "request_id": "req_2",
                        "error": "busy",
                    },
                })),
                line(&json!({
                    "type": "control_response",
                    "response": {
                        "subtype": "success",
                        "request_id": "req_1",
                        "response": { "ok": true },
                    },
                })),
                line(&json!({
                    "type": "control_response",
                    "response": {
                        "subtype": "success",
                        "request_id": "req_1",
                    },
                })),
            ]),
        );
        assert_eq!(
            seen,
            Vec::from([
                Vec::from([written(interrupt("req_1"), sent("req_1", 1))]),
                Vec::new(),
                Vec::from([written(interrupt("req_2"), sent("req_2", 2))]),
                Vec::new(),
                Vec::from([Effect::Reply(
                    2,
                    Err(Failure::Control {
                        subtype: String::from("interrupt"),
                        message: String::from("busy"),
                    })
                )]),
                Vec::from([Effect::Reply(1, Ok(json!({ "ok": true })))]),
                Vec::from([Effect::Log(Note::UnknownResponse {
                    provider: Provider::Claude,
                    request: String::from("req_1"),
                })]),
            ])
        );
    }

    #[test]
    fn a_failed_write_fails_its_command_and_a_refusal_is_only_logged() {
        let mut session = Session::running();
        let seen = walk(
            &mut session,
            Vec::from([
                Input::Wrote {
                    then: sent("req_1", 1),
                    outcome: Err("broken pipe"),
                },
                Input::Wrote {
                    then: Then::User(2),
                    outcome: Err("broken pipe"),
                },
                Input::Wrote {
                    then: Then::Refusal {
                        subtype: String::from("hook_callback"),
                    },
                    outcome: Err("broken pipe"),
                },
            ]),
        );
        assert_eq!(
            seen,
            Vec::from([
                Vec::from([Effect::Reply(1, Err(Failure::Io("broken pipe")))]),
                Vec::from([Effect::Reply(2, Err(Failure::Io("broken pipe")))]),
                Vec::from([Effect::Log(Note::Unwritten {
                    provider: Provider::Claude,
                    writing: String::from("refusing hook_callback"),
                    error: String::from("claude i/o: broken pipe"),
                })]),
            ])
        );
    }

    #[test]
    fn an_unsupported_control_request_gets_an_error_reply() {
        let mut session = Session::<u8>::running();
        let seen: Vec<Step> = session.step(line(&json!({
            "type": "control_request",
            "request_id": "cli_1",
            "request": { "subtype": "hook_callback" },
        })));
        let message = "unsupported control request subtype: hook_callback";
        assert_eq!(
            seen,
            Vec::from([
                Effect::Log(Note::Unsupported {
                    provider: Provider::Claude,
                    request: String::from("hook_callback"),
                }),
                written(
                    Outbound::ControlError {
                        request_id: String::from("cli_1"),
                        message: String::from(message),
                    },
                    Then::Refusal {
                        subtype: String::from("hook_callback"),
                    },
                )
            ])
        );
    }

    #[test]
    fn a_permission_is_answered_once_and_a_withdrawn_one_not_at_all() {
        let mut session = Session::running();
        let respond = |id: &str, reply| {
            Input::Command(Command::Respond {
                id: PermissionRequestId(String::from(id)),
                decision: PermissionDecision::Allow,
                reply,
            })
        };
        let seen = walk(
            &mut session,
            Vec::from([
                line(&permission("perm_1")),
                line(&permission("perm_2")),
                line(&json!({
                    "type": "control_cancel_request",
                    "request_id": "perm_2",
                })),
                respond("perm_1", 1),
                respond("perm_1", 2),
                respond("perm_2", 3),
            ]),
        );
        let unknown = |id: &str, reply| {
            Vec::from([Effect::Reply(
                reply,
                Err(Failure::UnknownPermissionRequest(PermissionRequestId(
                    String::from(id),
                ))),
            )])
        };
        assert_eq!(
            (
                seen.get(3).map(|effects| match effects.as_slice() {
                    [Effect::Write { then, .. }] => Some(then.clone()),
                    _ => None,
                }),
                &seen[4..],
            ),
            (
                Some(Some(Then::Answer(1))),
                &[unknown("perm_1", 2), unknown("perm_2", 3)][..],
            )
        );
    }

    #[test]
    fn a_result_carries_the_last_message_since_the_user_wrote() {
        let mut session = Session::<u8>::running();
        let assistant = json!({
            "type": "assistant",
            "uuid": "m1",
            "message": { "role": "assistant", "content": [] },
        });
        let result = json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "session_id": "s1",
        });
        let last = |effects: &[Step]| match effects {
            [
                Effect::Emit(ClaudeEvent::TurnFinished {
                    last_message, ..
                }),
            ] => Some(last_message.clone()),
            _ => None,
        };
        let seen = walk(
            &mut session,
            Vec::from([
                line(&assistant),
                line(&result),
                Input::Wrote {
                    then: Then::User(1),
                    outcome: Ok(()),
                },
                line(&result),
            ]),
        );
        assert_eq!(
            (last(&seen[1]), &seen[2], last(&seen[3])),
            (
                Some(Some(MessageId(String::from("m1")))),
                &Vec::from([Effect::Reply(1, Ok(Value::Null))]),
                Some(None),
            )
        );
    }

    #[test]
    fn a_line_that_is_not_json_passes_on_as_unknown() {
        let mut session = Session::<u8>::running();
        let seen: Vec<Step> =
            session.step(Input::Read(Ok(Some(String::from("oops")))));
        assert_eq!(
            seen,
            Vec::from([
                Effect::Log(Note::NotJson {
                    provider: Provider::Claude,
                    error: String::from("expected value at line 1 column 1"),
                    line: String::from("oops"),
                }),
                Effect::Emit(ClaudeEvent::Unknown(Value::String(
                    String::from("oops")
                )))
            ])
        );
    }

    #[test]
    fn stopping_fails_what_waits_and_reports_the_exit_to_a_shutdown() {
        let mut session = Session::running();
        let seen = walk(
            &mut session,
            Vec::from([
                line(&permission("perm_1")),
                Input::Wrote {
                    then: sent("req_1", 1),
                    outcome: Ok(()),
                },
                Input::Command(Command::Shutdown('a')),
                Input::Stopped {
                    exit: exit(),
                    then: After::Shutdown('a'),
                },
                Input::Command(Command::Shutdown('b')),
                Input::Command(Command::SendUser {
                    text: String::from("hi"),
                    reply: 2,
                }),
                Input::Command(Command::Respond {
                    id: PermissionRequestId(String::from("perm_1")),
                    decision: PermissionDecision::Allow,
                    reply: 3,
                }),
                Input::Closed,
            ]),
        );
        assert_eq!(
            &seen[2..],
            [
                Vec::from([Effect::Stop(After::Shutdown('a'))]),
                Vec::from([
                    Effect::Reply(1, Err(Failure::Exited)),
                    Effect::Emit(ClaudeEvent::Exited { status: exit() }),
                    Effect::Report('a', exit()),
                ]),
                Vec::from([Effect::Report('b', exit())]),
                Vec::from([Effect::Reply(2, Err(Failure::Exited))]),
                Vec::from([Effect::Reply(
                    3,
                    Err(Failure::UnknownPermissionRequest(
                        PermissionRequestId(String::from("perm_1"))
                    ))
                )]),
                Vec::from([Effect::End]),
            ]
        );
    }

    #[test]
    fn stdout_closing_or_failing_stops_the_process_and_closing_ends_it() {
        let mut session = Session::<u8>::running();
        let seen: Vec<Vec<Step>> = walk(
            &mut session,
            Vec::from([
                Input::Read(Ok(None)),
                Input::Read(Err(String::from("reset"))),
                Input::Closed,
                Input::Stopped {
                    exit: exit(),
                    then: After::End,
                },
            ]),
        );
        assert_eq!(
            seen,
            Vec::from([
                Vec::from([Effect::Stop(After::Nothing)]),
                Vec::from([
                    Effect::Log(Note::Unreadable {
                        provider: Provider::Claude,
                        error: String::from("reset"),
                    }),
                    Effect::Stop(After::Nothing)
                ]),
                Vec::from([Effect::Stop(After::End)]),
                Vec::from([
                    Effect::Emit(ClaudeEvent::Exited { status: exit() }),
                    Effect::End,
                ]),
            ])
        );
    }

    #[test]
    fn failures_read_as_the_driver_errors_they_become() {
        let failures: [Failure<&str>; 4] = [
            Failure::Exited,
            Failure::Io("broken pipe"),
            Failure::Control {
                subtype: String::from("interrupt"),
                message: String::from("busy"),
            },
            Failure::UnknownPermissionRequest(PermissionRequestId(
                String::from("perm_1"),
            )),
        ];
        assert_eq!(
            failures.map(|failure| failure.to_string()),
            [
                "the claude process has exited",
                "claude i/o: broken pipe",
                "claude rejected interrupt: busy",
                "no open permission request with id perm_1",
            ]
            .map(String::from)
        );
    }
}
