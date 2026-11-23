//! The decisions of the actor that owns the host's `codex app-server`, as a
//! state machine: routing each JSON-RPC response to the call that waits for
//! it, keeping Codex's open requests until they are answered or withdrawn,
//! and failing what waits once the server exits.
//!
use core::fmt;

use agents::driver::{self, Command, FailureText};
use agents::{Exit, Note};
use serde_json::Value;

use crate::prelude::*;
use crate::protocol::ResponseError;
use crate::{
    AnswerError, AnswerShape, ApprovalId, Call, CodexEvent, Decision,
    INITIALIZE, Inbound, Outbound, RequestId, ThreadEvent, ThreadId, answer,
    encode, parse_line,
};

/// What a handle asks of the server.
#[derive(Debug, Clone)]
pub enum CallRequest<R> {
    Request {
        call: Call,
        reply: R,
    },
    /// Answers an open approval or user-input request exactly once.
    Respond {
        thread: ThreadId,
        id: ApprovalId,
        decision: Decision,
        reply: R,
    },
}

/// What a line is written for, and so what its outcome decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Then<R> {
    /// Our request `id`, whose response the caller waits for.
    Request { id: i64, method: String, reply: R },
    /// The answer to one of Codex's requests.
    Answer(R),
    /// A line nobody waits for; a failure is logged after this context.
    Logged(String),
}

type After<R, S, E> = driver::After<S, (R, E)>;
type Completion<R, S, E> = Result<Option<Effect<R, S, E>>, (R, E)>;
pub type Input<R, S, E> = driver::Input<CallRequest<R>, Then<R>, S, (R, E), E>;
pub type Effect<R, S, E> =
    driver::Effect<CodexEvent, R, S, Then<R>, (R, E), Failure<E>>;
pub type Failure<E> = driver::Failure<E, Problem>;

#[derive(Debug, PartialEq, Eq)]
pub enum Problem {
    Rpc {
        method: String,
        message: String,
    },
    MalformedResponse {
        method: String,
        detail: &'static str,
    },
    UnknownApproval(ApprovalId),
    DecisionNotApplicable(ApprovalId),
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rpc { method, message } => {
                write!(f, "codex rejected {method}: {message}")
            }
            Self::MalformedResponse { method, detail } => {
                write!(f, "codex {method}: {detail}")
            }
            Self::UnknownApproval(id) => {
                write!(f, "no open approval with id {}", id.0)
            }
            Self::DecisionNotApplicable(id) => write!(
                f,
                "approval {} cannot be answered with that decision",
                id.0
            ),
        }
    }
}

const FAILURE_TEXT: FailureText = FailureText {
    exited: "the codex app-server has exited",
    io: "codex i/o",
    json: "could not encode or decode a codex message",
};

/// A Codex request we have not answered yet.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Open {
    rpc_id: RequestId,
    thread: ThreadId,
    shape: AnswerShape,
}

/// The server's state: our requests awaiting a response, Codex's requests
/// awaiting our answer, and how the process ended once it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server<R> {
    exited: Option<Exit>,
    /// The server's last stderr line once it has exited, which says why.
    refusal: Option<String>,
    /// Our requests by JSON-RPC id, with their method and who waits.
    pending: BTreeMap<i64, (String, R)>,
    open: BTreeMap<(ThreadId, ApprovalId), Open>,
    next_id: i64,
}

impl<R> Default for Server<R> {
    fn default() -> Self {
        Self {
            exited: None,
            refusal: None,
            pending: BTreeMap::new(),
            open: BTreeMap::new(),
            next_id: 0,
        }
    }
}

impl<R> Server<R> {
    pub fn step<S, E: fmt::Display>(
        &mut self,
        input: Input<R, S, E>,
    ) -> Vec<Effect<R, S, E>> {
        match input {
            Input::Read(Ok(Some(line))) => self.line(&line),
            Input::Read(Ok(None)) => self.stop(After::Nothing),
            Input::Read(Err(error)) => logged(
                Note::Unreadable {
                    provider: crate::PROVIDER,
                    error,
                },
                self.stop(After::Nothing),
            ),
            Input::Command(Command::Call(call)) => self.command(call),
            Input::Command(Command::Shutdown(waiter)) => {
                self.stop(After::Shutdown(waiter))
            }
            Input::Closed => self.stop(After::End),
            Input::Wrote { then, outcome } => self.wrote(then, outcome),
            Input::Stopped { exit, stderr, then } => {
                self.stopped(&exit, stderr, then)
            }
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
            Inbound::Event(event) => Vec::from([Effect::Emit(event)]),
            Inbound::Response { id, outcome } => self.response(&id, outcome),
            Inbound::Approval {
                rpc_id,
                thread,
                shape,
                event,
            } => {
                self.open.insert(
                    (thread.clone(), ApprovalId::of(&rpc_id)),
                    Open {
                        rpc_id,
                        thread: thread.clone(),
                        shape,
                    },
                );
                Vec::from([Effect::Emit(CodexEvent::Thread { thread, event })])
            }
            Inbound::Resolved { thread, rpc_id } => {
                self.resolved(thread, &rpc_id)
            }
            Inbound::Unsupported {
                rpc_id,
                method,
                event,
            } => {
                let message = format!("conductor does not handle {method}");
                logged(
                    Note::Unsupported {
                        provider: crate::PROVIDER,
                        request: method.clone(),
                    },
                    self.reject(
                        rpc_id,
                        message,
                        format!("rejecting {method}"),
                        event,
                    ),
                )
            }
            Inbound::Declined { rpc_id, event } => logged(
                Note::SecretDeclined {
                    provider: crate::PROVIDER,
                },
                self.reject(
                    rpc_id,
                    String::from("conductor does not take secret answers"),
                    String::from("declining a secret question"),
                    event,
                ),
            ),
        }
    }

    /// Codex also resolves requests we answered; only an unanswered one
    /// means Codex withdrew it.
    fn resolved<S, E>(
        &mut self,
        thread: ThreadId,
        rpc_id: &RequestId,
    ) -> Vec<Effect<R, S, E>> {
        let id = ApprovalId::of(rpc_id);
        match self.open.remove(&(thread.clone(), id.clone())) {
            Some(_) => Vec::from([Effect::Emit(CodexEvent::Thread {
                thread,
                event: ThreadEvent::ApprovalCancelled { id },
            })]),
            None => Vec::new(),
        }
    }

    /// Answers a request we don't serve with an error, which it needs or
    /// the turn waits forever, then passes on its event.
    fn reject<S, E: fmt::Display>(
        &self,
        rpc_id: RequestId,
        message: String,
        context: String,
        event: CodexEvent,
    ) -> Vec<Effect<R, S, E>> {
        let mut effects = self
            .write(Outbound::Reject { rpc_id, message }, Then::Logged(context));
        effects.push(Effect::Emit(event));
        effects
    }

    fn response<S, E: fmt::Display>(
        &mut self,
        id: &RequestId,
        outcome: Result<Value, ResponseError>,
    ) -> Vec<Effect<R, S, E>> {
        let waiting = match id {
            RequestId::Integer(n) => self.pending.remove(n),
            RequestId::String(_) => None,
        };
        let Some((method, reply)) = waiting else {
            return Vec::from([Effect::Log(Note::UnknownResponse {
                provider: crate::PROVIDER,
                request: format!("{id:?}"),
            })]);
        };
        let initialized = match (method.as_str(), &outcome) {
            (INITIALIZE, Ok(_)) => self.write(
                Outbound::Initialized,
                Then::Logged(String::from("sending initialized")),
            ),
            _ => Vec::new(),
        };
        let replied = Effect::Reply(
            reply,
            outcome.map_err(|error| {
                Failure::Protocol(match error {
                    ResponseError::Rejected(message) => {
                        Problem::Rpc { method, message }
                    }
                    ResponseError::Malformed(detail) => {
                        Problem::MalformedResponse { method, detail }
                    }
                })
            }),
        );
        initialized.into_iter().chain([replied]).collect()
    }

    fn command<S, E: fmt::Display>(
        &mut self,
        command: CallRequest<R>,
    ) -> Vec<Effect<R, S, E>> {
        match command {
            CallRequest::Request { call, reply } => {
                self.next_id = self.next_id.saturating_add(1);
                let id = self.next_id;
                let method = String::from(call.method());
                self.write(
                    Outbound::Request { id, call },
                    Then::Request { id, method, reply },
                )
            }
            CallRequest::Respond {
                thread,
                id,
                decision,
                reply,
            } => match self.open.remove(&(thread, id.clone())) {
                None => Vec::from([Effect::Reply(
                    reply,
                    Err(Failure::Protocol(Problem::UnknownApproval(id))),
                )]),
                Some(open) => {
                    let answered = answer(&open.shape, decision);
                    self.settle(id, open, answered, reply)
                }
            },
        }
    }

    /// Sends the answer, or puts the approval back when the decision didn't
    /// fit.
    fn settle<S, E: fmt::Display>(
        &mut self,
        id: ApprovalId,
        open: Open,
        answered: Result<Value, AnswerError>,
        reply: R,
    ) -> Vec<Effect<R, S, E>> {
        match answered {
            Ok(result) => logged(
                Note::Answering {
                    provider: crate::PROVIDER,
                    approval: id.0,
                    thread: open.thread.0,
                },
                self.write(
                    Outbound::Answer {
                        rpc_id: open.rpc_id,
                        result,
                    },
                    Then::Answer(reply),
                ),
            ),
            Err(AnswerError::NotApplicable) => {
                self.open.insert((open.thread.clone(), id.clone()), open);
                Vec::from([Effect::Reply(
                    reply,
                    Err(Failure::Protocol(Problem::DecisionNotApplicable(id))),
                )])
            }
            Err(AnswerError::Encode(error)) => {
                Vec::from([Effect::Reply(reply, Err(Failure::Json(error)))])
            }
        }
    }

    fn write<S, E: fmt::Display>(
        &self,
        msg: Outbound,
        then: Then<R>,
    ) -> Vec<Effect<R, S, E>> {
        match (&self.exited, encode(msg)) {
            (Some(_), _) => failed(then, self.gone()),
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
            (Then::Request { id, method, reply }, Ok(())) => {
                self.pending.insert(id, (method, reply));
                Vec::new()
            }
            // The server can't take requests: stop it, and say why.
            (Then::Request { reply, .. }, Err(error)) => {
                Vec::from([Effect::Stop(After::Protocol((reply, error)))])
            }
            (Then::Answer(reply), Ok(())) => {
                Vec::from([Effect::Reply(reply, Ok(Value::Null))])
            }
            (Then::Logged(_), Ok(())) => Vec::new(),
            (unwritten, Err(error)) => failed(unwritten, Failure::Io(error)),
        }
    }

    /// Why a call fails once the server has exited.
    fn gone<E>(&self) -> Failure<E> {
        self.refusal
            .clone()
            .map_or(Failure::Exited, Failure::Refused)
    }

    fn stop<S, E>(&self, then: After<R, S, E>) -> Vec<Effect<R, S, E>> {
        self.completed(then.stop(self.exited.as_ref()))
            .into_iter()
            .collect()
    }

    /// Fails everything still waiting on the process.
    fn stopped<S, E>(
        &mut self,
        exit: &Exit,
        refusal: Option<String>,
        then: After<R, S, E>,
    ) -> Vec<Effect<R, S, E>> {
        self.exited = Some(exit.clone());
        self.refusal = refusal;
        self.open.clear();
        let pending = core::mem::take(&mut self.pending);
        pending
            .into_values()
            .map(|(_, reply)| Effect::Reply(reply, Err(self.gone())))
            .chain([Effect::Emit(CodexEvent::Exited {
                status: exit.clone(),
            })])
            .chain(self.completed(then.complete(exit)))
            .collect()
    }

    fn completed<S, E>(
        &self,
        completion: Completion<R, S, E>,
    ) -> Option<Effect<R, S, E>> {
        match completion {
            Ok(effect) => effect,
            Err((reply, error)) => Some(Effect::Reply(
                reply,
                Err(self
                    .refusal
                    .clone()
                    .map_or(Failure::Io(error), Failure::Refused)),
            )),
        }
    }
}

/// A write for `then` that could not happen.
fn failed<R, S, E: fmt::Display>(
    then: Then<R>,
    failure: Failure<E>,
) -> Vec<Effect<R, S, E>> {
    match then {
        Then::Request { reply, .. } | Then::Answer(reply) => {
            Vec::from([Effect::Reply(reply, Err(failure))])
        }
        Then::Logged(context) => Vec::from([Effect::Log(Note::Unwritten {
            provider: crate::PROVIDER,
            writing: context,
            error: failure.describe(&FAILURE_TEXT),
        })]),
    }
}

/// `note`, then `effects`.
fn logged<R, S, E>(
    note: Note,
    effects: Vec<Effect<R, S, E>>,
) -> Vec<Effect<R, S, E>> {
    core::iter::once(Effect::Log(note)).chain(effects).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use agents::{Exit, Note};
    use proto::Answer;
    use serde_json::{Value, json};

    use super::{
        After, CallRequest, Command, Effect, FAILURE_TEXT, Failure, Input,
        Open, Problem, Server, Then,
    };
    use crate::{
        AnswerError, AnswerShape, ApprovalId, Call, CodexEvent, Decision,
        Outbound, RequestId, ThreadEvent, ThreadId, encode,
    };

    type Step = Effect<u8, char, &'static str>;
    type Seen = Input<u8, char, &'static str>;

    fn walk(server: &mut Server<u8>, inputs: Vec<Seen>) -> Vec<Vec<Step>> {
        inputs.into_iter().map(|input| server.step(input)).collect()
    }

    fn line(value: &Value) -> Seen {
        Input::Read(Ok(Some(value.to_string())))
    }

    #[test]
    fn malformed_output_logs_the_parse_error() {
        let mut server = Server::<u8>::default();
        let seen: Vec<Step> = server
            .step(Input::Read(Ok(Some(String::from("private-test-marker")))));
        assert_eq!(
            seen,
            [Effect::Log(Note::NotJson {
                provider: crate::PROVIDER,
                error: String::from("expected value at line 1 column 1"),
            })]
        );
    }

    #[test]
    fn declining_a_secret_question_keeps_its_payload_out_of_diagnostics() {
        let mut server = Server::<u8>::default();
        let seen: Vec<Step> = server.step(line(&json!({
            "method": "item/tool/requestUserInput",
            "id": 42,
            "params": {
                "threadId": "thread_1",
                "turnId": "turn_1",
                "itemId": "item_1",
                "questions": [{
                    "id": "test",
                    "header": "Secret",
                    "question": "private-test-marker",
                    "isSecret": true,
                }],
            },
        })));
        assert!(matches!(
            seen.as_slice(),
            [
                Effect::Log(Note::SecretDeclined { provider }),
                Effect::Write { .. },
                Effect::Emit(event),
            ] if *provider == crate::PROVIDER && event.diagnostic() == ("secret_question_declined", Some(&ThreadId(String::from("thread_1"))))
        ));
        assert!(!format!("{seen:?}").contains("private-test-marker"));
    }

    fn written(msg: Outbound, then: Then<u8>) -> Step {
        Effect::Write {
            line: encode(msg).unwrap_or_else(|error| error.to_string()),
            then,
        }
    }

    fn request(call: Call, reply: u8) -> Seen {
        Input::Command(Command::Call(CallRequest::Request { call, reply }))
    }

    fn sent(id: i64, method: &str, reply: u8) -> Then<u8> {
        Then::Request {
            id,
            method: String::from(method),
            reply,
        }
    }

    fn wrote(then: Then<u8>) -> Seen {
        Input::Wrote {
            then,
            outcome: Ok(()),
        }
    }

    fn approval(id: i64) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "item/commandExecution/requestApproval",
            "params": {
                "threadId": "th_1",
                "turnId": "tu_1",
                "itemId": "it_1",
                "command": "ls",
                "cwd": "/work",
            },
        })
    }

    fn exit() -> Exit {
        Exit::Status(String::from("exit status: 1"))
    }

    #[test]
    fn a_response_goes_to_its_request_and_initialize_is_acknowledged() {
        let mut server = Server::default();
        let seen = walk(
            &mut server,
            Vec::from([
                request(Call::Initialize, 1),
                wrote(sent(1, "initialize", 1)),
                request(Call::ModelList, 2),
                wrote(sent(2, "model/list", 2)),
                line(
                    &json!({ "id": 2, "error": { "code": 1, "message": "no" } }),
                ),
                line(&json!({ "id": 1, "result": { "userAgent": "c/1" } })),
                line(&json!({ "id": 1, "result": {} })),
                line(&json!({ "id": "1", "result": {} })),
            ]),
        );
        assert_eq!(
            (&seen[1], &seen[4..]),
            (
                &Vec::new(),
                &[
                    Vec::from([Effect::Reply(
                        2,
                        Err(Failure::Protocol(Problem::Rpc {
                            method: String::from("model/list"),
                            message: String::from("no"),
                        }))
                    )]),
                    Vec::from([
                        written(
                            Outbound::Initialized,
                            Then::Logged(String::from("sending initialized"))
                        ),
                        Effect::Reply(1, Ok(json!({ "userAgent": "c/1" }))),
                    ]),
                    Vec::from([Effect::Log(Note::UnknownResponse {
                        provider: crate::PROVIDER,
                        request: String::from("Integer(1)"),
                    })]),
                    Vec::from([Effect::Log(Note::UnknownResponse {
                        provider: crate::PROVIDER,
                        request: String::from("String(\"1\")"),
                    })]),
                ][..]
            )
        );
    }

    #[test]
    fn matching_responses_require_an_explicit_result_or_valid_error() {
        let mut server = Server::default();
        let seen = walk(
            &mut server,
            Vec::from([
                wrote(sent(1, "initialize", 1)),
                wrote(sent(2, "model/list", 2)),
                wrote(sent(3, "model/list", 3)),
                wrote(sent(4, "turn/interrupt", 4)),
                line(&json!({"id": 1})),
                line(
                    &json!({"id": 2, "error": {"code": "wrong", "message": "bad"}}),
                ),
                line(&json!({"id": 3, "result": null})),
                line(&json!({"id": 4, "method": "unexpected", "result": null})),
            ]),
        );
        for (effects, reply, method, detail) in [
            (&seen[4], 1, "initialize", "malformed JSON-RPC response"),
            (
                &seen[5],
                2,
                "model/list",
                "malformed JSON-RPC error response",
            ),
            (&seen[7], 4, "turn/interrupt", "malformed JSON-RPC response"),
        ] {
            assert_eq!(
                effects.as_slice(),
                [Effect::Reply(
                    reply,
                    Err(Failure::Protocol(Problem::MalformedResponse {
                        method: String::from(method),
                        detail,
                    }))
                )]
            );
        }
        assert_eq!(seen[6], Vec::from([Effect::Reply(3, Ok(Value::Null))]));
    }

    #[test]
    fn an_approval_is_answered_once_and_a_withdrawn_one_is_cancelled() {
        let mut server = Server::default();
        let respond = |id: &str, decision, reply| {
            Input::Command(Command::Call(CallRequest::Respond {
                thread: ThreadId(String::from("th_1")),
                id: ApprovalId(format!("integer:{id}")),
                decision,
                reply,
            }))
        };
        let resolved = |id: i64| {
            line(&json!({
                "method": "serverRequest/resolved",
                "params": { "threadId": "th_1", "requestId": id },
            }))
        };
        let seen = walk(
            &mut server,
            Vec::from([
                line(&approval(7)),
                line(&approval(8)),
                respond("7", Decision::Answer(Vec::new()), 1),
                respond("7", Decision::Accept, 2),
                respond("7", Decision::Accept, 3),
                resolved(7),
                resolved(8),
                respond("8", Decision::Accept, 4),
            ]),
        );
        let thread = ThreadId(String::from("th_1"));
        assert_eq!(
            &seen[2..],
            [
                Vec::from([Effect::Reply(
                    1,
                    Err(Failure::Protocol(Problem::DecisionNotApplicable(
                        ApprovalId(String::from("integer:7"))
                    )))
                )]),
                Vec::from([
                    Effect::Log(Note::Answering {
                        provider: crate::PROVIDER,
                        approval: String::from("integer:7"),
                        thread: String::from("th_1"),
                    }),
                    written(
                        Outbound::Answer {
                            rpc_id: RequestId::Integer(7),
                            result: json!({ "decision": "accept" }),
                        },
                        Then::Answer(2)
                    )
                ]),
                Vec::from([Effect::Reply(
                    3,
                    Err(Failure::Protocol(Problem::UnknownApproval(
                        ApprovalId(String::from("integer:7"))
                    )))
                )]),
                Vec::new(),
                Vec::from([Effect::Emit(CodexEvent::Thread {
                    thread,
                    event: ThreadEvent::ApprovalCancelled {
                        id: ApprovalId(String::from("integer:8")),
                    },
                })]),
                Vec::from([Effect::Reply(
                    4,
                    Err(Failure::Protocol(Problem::UnknownApproval(
                        ApprovalId(String::from("integer:8"))
                    )))
                )]),
            ]
        );
    }

    #[test]
    fn answers_and_withdrawals_are_scoped_to_thread_and_wire_id() {
        let mut server = Server::default();
        let mut other = approval(7);
        other["id"] = json!("7");
        other["params"]["threadId"] = json!("th_2");
        let respond = |thread: &str, wire_id: &RequestId, reply| {
            Input::Command(Command::Call(CallRequest::Respond {
                thread: ThreadId(String::from(thread)),
                id: ApprovalId::of(wire_id),
                decision: Decision::Accept,
                reply,
            }))
        };
        let number = RequestId::Integer(7);
        let string = RequestId::String(String::from("7"));
        let seen = walk(
            &mut server,
            Vec::from([
                line(&approval(7)),
                line(&other),
                respond("th_2", &number, 1),
                line(&json!({
                    "method": "serverRequest/resolved",
                    "params": { "threadId": "th_1", "requestId": "7" },
                })),
                respond("th_2", &string, 2),
                respond("th_1", &number, 3),
            ]),
        );
        assert_eq!(
            seen[2],
            Vec::from([Effect::Reply(
                1,
                Err(Failure::Protocol(Problem::UnknownApproval(
                    ApprovalId::of(&number)
                ))),
            )])
        );
        assert_eq!(seen[3].as_slice(), &[]);
        for (effects, id, reply) in
            [(&seen[4], string, 2), (&seen[5], number, 3)]
        {
            assert_eq!(
                effects.get(1),
                Some(&written(
                    Outbound::Answer {
                        rpc_id: id,
                        result: json!({"decision": "accept"})
                    },
                    Then::Answer(reply),
                ))
            );
        }
    }

    #[test]
    fn a_question_can_be_answered_after_a_wrong_answer_count() {
        let mut server = Server::default();
        let request = json!({
            "method": "item/tool/requestUserInput",
            "id": 42,
            "params": {
                "threadId": "th_1",
                "turnId": "tu_1",
                "itemId": "item_1",
                "questions": [{
                    "id": "color",
                    "header": "Color",
                    "question": "Which color?",
                    "options": [{"label": "Blue", "description": "Cool"}],
                }],
            },
        });
        let rpc_id = RequestId::Integer(42);
        let id = ApprovalId::of(&rpc_id);
        let respond = |answers, reply| {
            Input::Command(Command::Call(CallRequest::Respond {
                thread: ThreadId(String::from("th_1")),
                id: id.clone(),
                decision: Decision::Answer(answers),
                reply,
            }))
        };
        let seen = walk(
            &mut server,
            Vec::from([
                line(&request),
                respond(Vec::new(), 1),
                respond(
                    Vec::from([Answer(Vec::from([String::from("Blue")]))]),
                    2,
                ),
            ]),
        );
        assert_eq!(
            seen[1],
            [Effect::Reply(
                1,
                Err(Failure::Protocol(Problem::DecisionNotApplicable(id)))
            )],
        );
        assert!(matches!(
            seen[2].as_slice(),
            [Effect::Log(_), Effect::Write { .. }]
        ));
        assert_eq!(
            seen[2].get(1),
            Some(&written(
                Outbound::Answer {
                    rpc_id,
                    result: json!({"answers": {"color": {"answers": ["Blue"]}}}),
                },
                Then::Answer(2),
            )),
        );
    }

    #[test]
    fn an_answer_that_does_not_encode_fails_and_closes_the_approval() {
        let mut server = Server::default();
        let id = ApprovalId(String::from("ap_1"));
        let open = Open {
            rpc_id: RequestId::Integer(3),
            thread: ThreadId(String::from("th_1")),
            shape: AnswerShape::Command,
        };
        let failure = serde_json::to_value(BTreeMap::from([((1, 2), 0)]))
            .map_or_else(AnswerError::Encode, |_| AnswerError::NotApplicable);
        let first: Vec<Step> = server.settle(id.clone(), open, Err(failure), 1);
        let again: Vec<Step> =
            server.step(Input::Command(Command::Call(CallRequest::Respond {
                thread: ThreadId(String::from("th_1")),
                id,
                decision: Decision::Accept,
                reply: 2,
            })));
        assert_eq!(
            [first, again].map(|effects| match effects.as_slice() {
                [Effect::Reply(_, Err(failure))] => failure.describe(&FAILURE_TEXT),
                _ => String::from("not one failed reply"),
            }),
            [
                "could not encode or decode a codex message: key must be a string",
                "no open approval with id ap_1",
            ]
            .map(String::from)
        );
    }

    #[test]
    fn a_request_codex_does_not_serve_is_rejected_and_passed_on() {
        let mut server = Server::<u8>::default();
        let seen: Vec<Step> = server.step(line(&json!({
            "id": 9,
            "method": "item/tool/call",
            "params": { "threadId": "th_1" },
        })));
        assert_eq!(
            &seen[..2],
            [
                Effect::Log(Note::Unsupported {
                    provider: crate::PROVIDER,
                    request: String::from("item/tool/call"),
                }),
                written(
                    Outbound::Reject {
                        rpc_id: RequestId::Integer(9),
                        message: String::from(
                            "conductor does not handle item/tool/call"
                        ),
                    },
                    Then::Logged(String::from("rejecting item/tool/call"))
                )
            ]
        );
    }

    #[test]
    fn a_failed_request_write_stops_the_server_and_fails_with_its_reason() {
        let mut server = Server::default();
        let seen = walk(
            &mut server,
            Vec::from([
                wrote(sent(1, "model/list", 1)),
                Input::Wrote {
                    then: sent(2, "turn/start", 2),
                    outcome: Err("broken pipe"),
                },
                Input::Stopped {
                    exit: exit(),
                    stderr: Some(String::from("bad config")),
                    then: After::Protocol((2, "broken pipe")),
                },
                request(Call::ModelList, 3),
                Input::Command(Command::Shutdown('a')),
                Input::Closed,
            ]),
        );
        let refused = || Failure::Refused(String::from("bad config"));
        assert_eq!(
            &seen[1..],
            [
                Vec::from([Effect::Stop(After::Protocol((2, "broken pipe")))]),
                Vec::from([
                    Effect::Reply(1, Err(refused())),
                    Effect::Emit(CodexEvent::Exited { status: exit() }),
                    Effect::Reply(2, Err(refused())),
                ]),
                Vec::from([Effect::Reply(3, Err(refused()))]),
                Vec::from([Effect::Report('a', exit())]),
                Vec::from([Effect::End]),
            ]
        );
    }

    #[test]
    fn without_a_reason_a_failed_request_keeps_its_error() {
        let mut server = Server::<u8>::default();
        let seen: Vec<Step> = server.step(Input::Stopped {
            exit: exit(),
            stderr: None,
            then: After::Protocol((1, "broken pipe")),
        });
        assert_eq!(
            seen,
            Vec::from([
                Effect::Emit(CodexEvent::Exited { status: exit() }),
                Effect::Reply(1, Err(Failure::Io("broken pipe"))),
            ])
        );
    }

    #[test]
    fn stdout_ending_stops_the_server_and_an_answer_write_failure_is_reported()
    {
        let mut server = Server::<u8>::default();
        let seen: Vec<Vec<Step>> = walk(
            &mut server,
            Vec::from([
                Input::Read(Ok(None)),
                Input::Read(Err(String::from("reset"))),
                Input::Wrote {
                    then: Then::Answer(1),
                    outcome: Err("broken pipe"),
                },
                Input::Wrote {
                    then: Then::Logged(String::from("rejecting x")),
                    outcome: Err("broken pipe"),
                },
                line(&json!({ "method": "thread/started", "params": {} })),
            ]),
        );
        assert_eq!(
            &seen[..4],
            [
                Vec::from([Effect::Stop(After::Nothing)]),
                Vec::from([
                    Effect::Log(Note::Unreadable {
                        provider: crate::PROVIDER,
                        error: String::from("reset"),
                    }),
                    Effect::Stop(After::Nothing)
                ]),
                Vec::from([Effect::Reply(1, Err(Failure::Io("broken pipe")))]),
                Vec::from([Effect::Log(Note::Unwritten {
                    provider: crate::PROVIDER,
                    writing: String::from("rejecting x"),
                    error: String::from("codex i/o: broken pipe"),
                })]),
            ]
        );
    }

    #[test]
    fn failures_read_as_the_driver_errors_they_become() {
        let failures: [Failure<&str>; 4] = [
            Failure::Exited,
            Failure::Refused(String::from("bad config")),
            Failure::Io("broken pipe"),
            Failure::Protocol(Problem::DecisionNotApplicable(ApprovalId(
                String::from("7"),
            ))),
        ];
        assert_eq!(
            failures.map(|failure| failure.describe(&FAILURE_TEXT)),
            [
                "the codex app-server has exited",
                "the codex app-server has exited: bad config",
                "codex i/o: broken pipe",
                "approval 7 cannot be answered with that decision",
            ]
            .map(String::from)
        );
    }
    #[test]
    fn failures_are_equal_only_in_kind_and_contents() {
        type Failed = Failure<&'static str>;
        let json = |text: &str| {
            serde_json::from_str::<Value>(text).err().map(Failed::Json)
        };
        let rpc = |method: &str, message: &str| {
            Failed::Protocol(Problem::Rpc {
                method: String::from(method),
                message: String::from(message),
            })
        };
        assert_eq!(Failed::Exited, Failed::Exited);
        assert_ne!(Failed::Exited, Failed::Io("broken pipe"));
        assert_eq!(
            Failed::Refused(String::from("no")),
            Failed::Refused(String::from("no"))
        );
        assert_ne!(
            Failed::Refused(String::from("no")),
            Failed::Refused(String::from("later"))
        );
        assert_eq!(Failed::Io("broken pipe"), Failed::Io("broken pipe"));
        assert_ne!(Failed::Io("broken pipe"), Failed::Io("closed"));
        assert_eq!(json("{"), json("{"));
        assert_ne!(json("{"), json("["));
        assert_eq!(rpc("turn/start", "no"), rpc("turn/start", "no"));
        assert_ne!(rpc("turn/start", "no"), rpc("thread/start", "no"));
        assert_ne!(rpc("turn/start", "no"), rpc("turn/start", "busy"));
        let id = |text: &str| ApprovalId(String::from(text));
        assert_eq!(
            Failed::Protocol(Problem::UnknownApproval(id("a"))),
            Failed::Protocol(Problem::UnknownApproval(id("a")))
        );
        assert_ne!(
            Failed::Protocol(Problem::UnknownApproval(id("a"))),
            Failed::Protocol(Problem::DecisionNotApplicable(id("a")))
        );
        assert_ne!(
            Failed::Protocol(Problem::DecisionNotApplicable(id("a"))),
            Failed::Protocol(Problem::DecisionNotApplicable(id("b")))
        );
    }
}
