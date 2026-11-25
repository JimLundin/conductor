//! The decisions of the actor that owns one `claude` process, as a state
//! machine.
//!
use core::convert::Infallible;
use core::fmt;

use agents::driver::{self, Command, FailureText};
use agents::{Exit, Note};
use serde_json::Value;

use crate::prelude::*;
use crate::{
    ClaudeEvent, ControlCall, Inbound, Open, Outbound, PermissionDecision,
    PermissionRequestId, encode, parse_line,
};

/// What a handle asks of the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallRequest<R> {
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

type After<S> = driver::After<S, Infallible>;
pub type Input<R, S, E> =
    driver::Input<CallRequest<R>, Then<R>, S, Infallible, E>;
pub type Effect<R, S, E> =
    driver::Effect<ClaudeEvent, R, S, Then<R>, Infallible, Failure<E>>;
pub type Failure<E> = driver::Failure<E, Problem>;

#[derive(Debug, PartialEq, Eq)]
pub enum Problem {
    Control { subtype: String, message: String },
    UnknownPermissionRequest(PermissionRequestId),
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Control { subtype, message } => {
                write!(f, "claude rejected {subtype}: {message}")
            }
            Self::UnknownPermissionRequest(id) => {
                write!(f, "no open permission request with id {}", id.0)
            }
        }
    }
}

const FAILURE_TEXT: FailureText = FailureText {
    exited: "the claude process has exited",
    io: "claude i/o",
    json: "could not encode a message for claude",
};

/// The session's state: our control requests awaiting a `control_response`,
/// the `can_use_tool` requests not yet answered, and the sub-agents the process started.
#[derive(Debug, Clone, PartialEq)]
pub struct Session<R> {
    exited: Option<Exit>,
    pending: BTreeMap<String, (ControlCall, R)>,
    open: BTreeMap<PermissionRequestId, Open>,
    next_request: u64,
    subagents: BTreeMap<String, String>,
    subagent_tools: BTreeMap<String, String>,
}

impl<R> Default for Session<R> {
    fn default() -> Self {
        Self {
            exited: None,
            pending: BTreeMap::new(),
            open: BTreeMap::new(),
            next_request: 0,
            subagents: BTreeMap::new(),
            subagent_tools: BTreeMap::new(),
        }
    }
}

impl<R> Session<R> {
    pub fn step<S, E: fmt::Display>(
        &mut self,
        input: Input<R, S, E>,
    ) -> Vec<Effect<R, S, E>> {
        match input {
            Input::Read(Ok(Some(line))) => self.line(&line),
            Input::Read(Ok(None)) => self.stop(After::Nothing),
            Input::Read(Err(error)) => {
                let mut effects = Vec::from([Effect::Log(Note::Unreadable {
                    provider: crate::PROVIDER,
                    error,
                })]);
                effects.extend(self.stop(After::Nothing));
                effects
            }
            Input::Command(Command::Call(call)) => self.command(call),
            Input::Command(Command::Shutdown(waiter)) => {
                self.stop(After::Shutdown(waiter))
            }
            Input::Closed => self.stop(After::End),
            Input::Wrote { then, outcome } => self.wrote(then, outcome),
            Input::Stopped { exit, then, .. } => self.stopped(&exit, then),
        }
    }

    fn line<S, E: fmt::Display>(&mut self, line: &str) -> Vec<Effect<R, S, E>> {
        match parse_line(line) {
            Ok(inbound) => self.inbound(inbound),
            Err(error) => Vec::from([Effect::Log(Note::NotJson {
                provider: crate::PROVIDER,
                error: error.to_string(),
            })]),
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
            Inbound::Permission {
                id,
                open,
                parent_tool_use_id,
                mut event,
            } => {
                self.open.insert(id, open);
                if let ClaudeEvent::PermissionRequested {
                    agent,
                    tool_use_id,
                    ..
                }
                | ClaudeEvent::QuestionAsked {
                    agent, tool_use_id, ..
                } = &mut event
                {
                    *agent = parent_tool_use_id
                        .as_ref()
                        .and_then(|call| self.subagents.get(call))
                        .or_else(|| self.subagent_tools.get(tool_use_id))
                        .cloned();
                }
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
                    provider: crate::PROVIDER,
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

    fn event(&mut self, received: ClaudeEvent) -> ClaudeEvent {
        match received {
            ClaudeEvent::PermissionCancelled { id } => {
                self.open.remove(&id);
                ClaudeEvent::PermissionCancelled { id }
            }
            ClaudeEvent::SubagentStarted {
                agent,
                tool_use_id,
                description,
                kind,
            } => {
                self.subagents.insert(tool_use_id.clone(), agent.clone());
                ClaudeEvent::SubagentStarted {
                    agent,
                    tool_use_id,
                    description,
                    kind,
                }
            }
            ClaudeEvent::UnidentifiedSubagentMessage {
                parent,
                role,
                content,
            } => self
                .subagents
                .get(&parent)
                .cloned()
                .map_or(ClaudeEvent::Unknown(Value::Null), |agent| {
                    self.child_message(agent, role, content)
                }),
            ClaudeEvent::SubagentMessage {
                agent,
                role,
                content,
            } => self.child_message(agent, role, content),
            ClaudeEvent::SubagentEnded { agent, .. }
                if !self.subagents.values().any(|known| known == &agent) =>
            {
                ClaudeEvent::Unknown(Value::String(format!(
                    "task_notification for task {agent}, which is no sub-agent"
                )))
            }
            other => other,
        }
    }

    fn child_message(
        &mut self,
        agent: String,
        role: crate::Role,
        content: Vec<crate::Block>,
    ) -> ClaudeEvent {
        for block in &content {
            match block {
                crate::Block::ToolUse { id, .. } => {
                    self.subagent_tools.insert(id.clone(), agent.clone());
                }
                crate::Block::ToolResult { tool_use_id, .. } => {
                    self.subagent_tools.remove(tool_use_id);
                }
                _ => {}
            }
        }
        ClaudeEvent::SubagentMessage {
            agent,
            role,
            content,
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
                    provider: crate::PROVIDER,
                    request: request_id.to_owned(),
                })])
            },
            |(call, reply)| {
                Vec::from([Effect::Reply(
                    reply,
                    outcome.map_err(|message| {
                        Failure::Protocol(Problem::Control {
                            subtype: String::from(call.subtype()),
                            message,
                        })
                    }),
                )])
            },
        )
    }

    fn command<S, E: fmt::Display>(
        &mut self,
        command: CallRequest<R>,
    ) -> Vec<Effect<R, S, E>> {
        match command {
            CallRequest::SendUser { text, reply } => {
                self.write(Outbound::User(text), Then::User(reply))
            }
            CallRequest::Control { call, reply } => {
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
            CallRequest::Respond {
                id,
                decision,
                reply,
            } => match self.open.get(&id) {
                None => Vec::from([Effect::Reply(
                    reply,
                    Err(Failure::Protocol(Problem::UnknownPermissionRequest(
                        id,
                    ))),
                )]),
                Some(open) => {
                    let effects = self.write(
                        Outbound::Permission {
                            id: id.clone(),
                            decision,
                            open: open.clone(),
                        },
                        Then::Answer(reply),
                    );
                    if matches!(effects.as_slice(), [Effect::Write { .. }]) {
                        self.open.remove(&id);
                    }
                    effects
                }
            },
        }
    }

    fn write<S, E: fmt::Display>(
        &self,
        msg: Outbound,
        then: Then<R>,
    ) -> Vec<Effect<R, S, E>> {
        match (&self.exited, encode(msg)) {
            (Some(_), _) => failed(then, Failure::Exited),
            (None, Ok(line)) => Vec::from([Effect::Write { line, then }]),
            (None, Err(error)) => failed(then, Failure::Json(error)),
        }
    }

    fn wrote<S, E: fmt::Display>(
        &mut self,
        then: Then<R>,
        outcome: Result<(), E>,
    ) -> Vec<Effect<R, S, E>> {
        match (then, outcome) {
            (Then::User(reply) | Then::Answer(reply), Ok(())) => {
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
            (Then::Refusal { .. }, Ok(())) => Vec::new(),
            (unwritten, Err(error)) => failed(unwritten, Failure::Io(error)),
        }
    }

    fn stop<S, E>(&self, then: After<S>) -> Vec<Effect<R, S, E>> {
        then.stop(self.exited.as_ref())
            .into_iter()
            .flatten()
            .collect()
    }

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
            .chain(then.complete(exit).into_iter().flatten())
            .collect()
    }
}

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
                provider: crate::PROVIDER,
                writing: format!("refusing {subtype}"),
                error: failure.describe(&FAILURE_TEXT),
            })])
        }
    }
}

#[cfg(test)]
mod tests {
    use agents::Answer;
    use agents::{Exit, Note};
    use serde_json::{Value, json};

    use super::{
        After, CallRequest, Command, Effect, FAILURE_TEXT, Failure, Input,
        Problem, Session, Then,
    };
    use crate::{
        ClaudeEvent, ControlCall, Outbound, PermissionDecision,
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
        let mut session = Session::<u8>::default();
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

    #[test]
    fn child_permissions_follow_the_parent_call_or_the_childs_tool_call() {
        let mut session = Session::default();
        let mut from_parent = permission("perm-parent");
        from_parent["parent_tool_use_id"] = json!("toolu_1");
        let mut from_tool = permission("perm-tool");
        from_tool["request"]["tool_use_id"] = json!("child-shell");
        let seen = walk(
            &mut session,
            vec![
                line(&task("task_started", "child", "local_agent")),
                line(&from_parent),
                line(&json!({
                    "type": "assistant",
                    "parent_tool_use_id": "toolu_1",
                    "message": {"content": [
                        {"type":"tool_use","id":"child-shell","name":"Bash","input":{"command":"ls"}}
                    ]},
                })),
                line(&from_tool),
            ],
        );
        for reply in [&seen[1], &seen[3]] {
            assert!(
                matches!(reply.as_slice(), [Effect::Emit(ClaudeEvent::PermissionRequested {
                agent: Some(agent), ..
            })] if agent == "child")
            );
        }
    }

    fn written(msg: Outbound, then: Then<u8>) -> Step {
        Effect::Write {
            line: encode(msg).unwrap_or_else(|error| error.to_string()),
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
        Input::Command(Command::Call(CallRequest::Control {
            call: ControlCall::Interrupt,
            reply,
        }))
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
        let mut session = Session::default();
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
                    Err(Failure::Protocol(Problem::Control {
                        subtype: String::from("interrupt"),
                        message: String::from("busy"),
                    }))
                )]),
                Vec::from([Effect::Reply(1, Ok(json!({ "ok": true })))]),
                Vec::from([Effect::Log(Note::UnknownResponse {
                    provider: crate::PROVIDER,
                    request: String::from("req_1"),
                })]),
            ])
        );
    }

    #[test]
    fn a_failed_write_fails_its_command_and_a_refusal_is_only_logged() {
        let mut session = Session::default();
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
                    provider: crate::PROVIDER,
                    writing: String::from("refusing hook_callback"),
                    error: String::from("claude i/o: broken pipe"),
                })]),
            ])
        );
    }

    #[test]
    fn an_unsupported_control_request_gets_an_error_reply() {
        let mut session = Session::<u8>::default();
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
                    provider: crate::PROVIDER,
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
    fn a_permission_can_be_answered_after_an_invalid_decision() {
        let mut session = Session::default();
        let respond = |decision, reply| {
            Input::Command(Command::Call(CallRequest::Respond {
                id: PermissionRequestId(String::from("perm_1")),
                decision,
                reply,
            }))
        };
        let seen = walk(
            &mut session,
            Vec::from([
                line(&permission("perm_1")),
                respond(PermissionDecision::Answer(Vec::new()), 1),
                respond(PermissionDecision::Allow, 2),
            ]),
        );
        assert!(matches!(seen.as_slice(), [
            _,
            invalid,
            valid,
        ] if matches!(invalid.as_slice(), [Effect::Reply(1, Err(Failure::Json(_)))])
            && matches!(valid.as_slice(), [Effect::Write { then: Then::Answer(2), .. }])));
    }

    #[test]
    fn a_question_can_be_answered_after_a_wrong_answer_count()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut session = Session::default();
        let mut question = permission("question_1");
        question["request"]["tool_name"] = json!("AskUserQuestion");
        question["request"]["input"] = json!({
            "questions": [{
                "question": "Which color?",
                "header": "Color",
                "options": [{"label": "Blue", "description": "Cool"}],
                "multiSelect": false,
            }],
        });
        let respond = |answers, reply| {
            Input::Command(Command::Call(CallRequest::Respond {
                id: PermissionRequestId(String::from("question_1")),
                decision: PermissionDecision::Answer(answers),
                reply,
            }))
        };
        let seen = walk(
            &mut session,
            Vec::from([
                line(&question),
                respond(Vec::new(), 1),
                respond(
                    Vec::from([Answer(Vec::from([String::from("Blue")]))]),
                    2,
                ),
            ]),
        );
        assert!(matches!(
            seen[1].as_slice(),
            [Effect::Reply(1, Err(Failure::Json(_)))]
        ));
        let [
            Effect::Write {
                line: wire,
                then: Then::Answer(2),
            },
        ] = seen[2].as_slice()
        else {
            return Err("corrected Claude answer was not written".into());
        };
        let wire_answer = serde_json::from_str::<Value>(wire)?;
        assert_eq!(
            wire_answer.pointer("/response/request_id"),
            Some(&json!("question_1"))
        );
        assert_eq!(
            wire_answer.pointer("/response/response/updatedInput/answers"),
            Some(&json!({"Which color?": "Blue"}))
        );
        Ok(())
    }

    #[test]
    fn a_permission_is_answered_once_and_a_withdrawn_one_not_at_all() {
        let mut session = Session::default();
        let respond = |id: &str, reply| {
            Input::Command(Command::Call(CallRequest::Respond {
                id: PermissionRequestId(String::from(id)),
                decision: PermissionDecision::Allow,
                reply,
            }))
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
                Err(Failure::Protocol(Problem::UnknownPermissionRequest(
                    PermissionRequestId(String::from(id)),
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
    fn malformed_output_logs_the_parse_error() {
        let mut session = Session::<u8>::default();
        let seen: Vec<Step> = session
            .step(Input::Read(Ok(Some(String::from("private-test-marker")))));
        assert_eq!(
            seen,
            Vec::from([Effect::Log(Note::NotJson {
                provider: crate::PROVIDER,
                error: String::from("expected value at line 1 column 1"),
            })])
        );
    }

    #[test]
    fn stopping_fails_what_waits_and_reports_the_exit_to_a_shutdown() {
        let mut session = Session::default();
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
                    stderr: None,
                    exit: exit(),
                    then: After::Shutdown('a'),
                },
                Input::Command(Command::Shutdown('b')),
                Input::Command(Command::Call(CallRequest::SendUser {
                    text: String::from("hi"),
                    reply: 2,
                })),
                Input::Command(Command::Call(CallRequest::Respond {
                    id: PermissionRequestId(String::from("perm_1")),
                    decision: PermissionDecision::Allow,
                    reply: 3,
                })),
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
                    Err(Failure::Protocol(Problem::UnknownPermissionRequest(
                        PermissionRequestId(String::from("perm_1"))
                    )))
                )]),
                Vec::from([Effect::End]),
            ]
        );
    }

    #[test]
    fn stdout_closing_or_failing_stops_the_process_and_closing_ends_it() {
        let mut session = Session::<u8>::default();
        let seen: Vec<Vec<Step>> = walk(
            &mut session,
            Vec::from([
                Input::Read(Ok(None)),
                Input::Read(Err(String::from("reset"))),
                Input::Closed,
                Input::Stopped {
                    stderr: None,
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
                        provider: crate::PROVIDER,
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
            Failure::Protocol(Problem::Control {
                subtype: String::from("interrupt"),
                message: String::from("busy"),
            }),
            Failure::Protocol(Problem::UnknownPermissionRequest(
                PermissionRequestId(String::from("perm_1")),
            )),
        ];
        assert_eq!(
            failures.map(|failure| failure.describe(&FAILURE_TEXT)),
            [
                "the claude process has exited",
                "claude i/o: broken pipe",
                "claude rejected interrupt: busy",
                "no open permission request with id perm_1",
            ]
            .map(String::from)
        );
    }
    #[test]
    fn failures_are_equal_only_in_kind_and_contents() {
        type Failed = Failure<&'static str>;
        let encode = |text: &str| {
            serde_json::from_str::<Value>(text).err().map(Failed::Json)
        };
        let control = |subtype: &str, message: &str| {
            Failed::Protocol(Problem::Control {
                subtype: String::from(subtype),
                message: String::from(message),
            })
        };
        let unknown = |id: &str| {
            Failed::Protocol(Problem::UnknownPermissionRequest(
                PermissionRequestId(String::from(id)),
            ))
        };
        assert_eq!(Failed::Exited, Failed::Exited);
        assert_ne!(Failed::Exited, Failed::Io("broken pipe"));
        assert_eq!(Failed::Io("broken pipe"), Failed::Io("broken pipe"));
        assert_ne!(Failed::Io("broken pipe"), Failed::Io("closed"));
        assert_eq!(encode("{"), encode("{"));
        assert_ne!(encode("{"), encode("["));
        assert_eq!(control("set_model", "no"), control("set_model", "no"));
        assert_ne!(control("set_model", "no"), control("interrupt", "no"));
        assert_ne!(control("set_model", "no"), control("set_model", "busy"));
        assert_eq!(unknown("a"), unknown("a"));
        assert_ne!(unknown("a"), unknown("b"));
    }
}
