//! The decisions of the actor that owns the host's `codex app-server`, as a
//! state machine: routing each JSON-RPC response to the call that waits for
//! it, keeping Codex's open requests until they are answered or withdrawn,
//! and failing what waits once the server exits.
//!
//! `codex-cli` owns the process: it turns each stdout line and each
//! handle's command into an [`Input`], and performs each [`Effect`]. A
//! reply's channel is the opaque `R` and a shutdown's the opaque `S`, which
//! the machine hands back with the answer; `E` is why a write failed.

use core::fmt;

use agents::{Exit, Note};
use proto::Provider;
use serde_json::Value;

use crate::prelude::*;
use crate::{
    AnswerError, AnswerShape, ApprovalId, Call, CodexEvent, Decision,
    INITIALIZE, Inbound, Outbound, RequestId, ThreadEvent, ThreadId, answer,
    encode, parse_line,
};

/// What a handle asks of the server.
#[derive(Debug, Clone)]
pub enum Command<R, S> {
    Request {
        call: Call,
        reply: R,
    },
    /// Answers an open approval or user-input request exactly once.
    Respond {
        id: ApprovalId,
        decision: Decision,
        reply: R,
    },
    /// Closes stdin, waits briefly for a clean exit, then kills.
    Shutdown(S),
}

/// What happened, as the shell saw it.
#[derive(Debug)]
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
    /// The process [`Effect::Stop`] stopped has exited, with its last
    /// stderr line if it printed one.
    Stopped {
        exit: Exit,
        refusal: Option<String>,
        then: After<R, S, E>,
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

/// What follows once the process has stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum After<R, S, E> {
    Nothing,
    Shutdown(S),
    /// The request whose write failed, with why.
    Fail(R, E),
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
    Emit(CodexEvent),
    Log(Note),
    /// Answer a handle's command; an answer to Codex answers `Null`.
    Reply(R, Result<Value, Failure<E>>),
    /// Answer a shutdown with how the process ended.
    Report(S, Exit),
    /// Stop the process, then report its exit as [`Input::Stopped`].
    Stop(After<R, S, E>),
    /// End the actor.
    End,
}

/// Why a command failed.
#[derive(Debug)]
pub enum Failure<E> {
    Exited,
    /// The server exited, and its last stderr line says why.
    Refused(String),
    Io(E),
    Json(serde_json::Error),
    Rpc {
        method: String,
        message: String,
    },
    UnknownApproval(ApprovalId),
    DecisionNotApplicable(ApprovalId),
}

/// An encoding failure equals another with the same description.
impl<E: PartialEq> PartialEq for Failure<E> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Exited, Self::Exited) => true,
            (Self::Refused(left), Self::Refused(right)) => left == right,
            (Self::Io(left), Self::Io(right)) => left == right,
            (Self::Json(left), Self::Json(right)) => {
                left.to_string() == right.to_string()
            }
            (
                Self::Rpc { method, message },
                Self::Rpc {
                    method: other_method,
                    message: other_message,
                },
            ) => method == other_method && message == other_message,
            (Self::UnknownApproval(left), Self::UnknownApproval(right))
            | (
                Self::DecisionNotApplicable(left),
                Self::DecisionNotApplicable(right),
            ) => left == right,
            _ => false,
        }
    }
}

impl<E: fmt::Display> fmt::Display for Failure<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exited => f.write_str("the codex app-server has exited"),
            Self::Refused(reason) => {
                write!(f, "the codex app-server has exited: {reason}")
            }
            Self::Io(error) => write!(f, "codex i/o: {error}"),
            Self::Json(error) => {
                write!(f, "could not encode or decode a codex message: {error}")
            }
            Self::Rpc { method, message } => {
                write!(f, "codex rejected {method}: {message}")
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
    open: BTreeMap<ApprovalId, Open>,
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
    /// A server whose process is running.
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
            Input::Read(Err(error)) => logged(
                Note::Unreadable {
                    provider: Provider::Codex,
                    error,
                },
                self.stop(After::Nothing),
            ),
            Input::Command(command) => self.command(command),
            Input::Closed => self.stop(After::End),
            Input::Wrote { then, outcome } => self.wrote(then, outcome),
            Input::Stopped {
                exit,
                refusal,
                then,
            } => self.stopped(&exit, refusal, then),
        }
    }

    fn line<S, E: fmt::Display>(&mut self, line: &str) -> Vec<Effect<R, S, E>> {
        match parse_line(line) {
            Ok(inbound) => self.inbound(inbound),
            Err(error) => logged(
                Note::NotJson {
                    provider: Provider::Codex,
                    error: error.to_string(),
                    line: line.to_owned(),
                },
                Vec::from([Effect::Emit(CodexEvent::Unknown(Value::String(
                    line.to_owned(),
                )))]),
            ),
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
                    ApprovalId::of(&rpc_id),
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
                        provider: Provider::Codex,
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
                    provider: Provider::Codex,
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
        match self.open.remove(&id) {
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
        outcome: Result<Value, String>,
    ) -> Vec<Effect<R, S, E>> {
        let waiting = match id {
            RequestId::Integer(n) => self.pending.remove(n),
            RequestId::String(_) => None,
        };
        let Some((method, reply)) = waiting else {
            return Vec::from([Effect::Log(Note::UnknownResponse {
                provider: Provider::Codex,
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
            outcome.map_err(|message| Failure::Rpc { method, message }),
        );
        initialized.into_iter().chain([replied]).collect()
    }

    fn command<S, E: fmt::Display>(
        &mut self,
        command: Command<R, S>,
    ) -> Vec<Effect<R, S, E>> {
        match command {
            Command::Request { call, reply } => {
                self.next_id = self.next_id.saturating_add(1);
                let id = self.next_id;
                let method = String::from(call.method());
                self.write(
                    Outbound::Request { id, call },
                    Then::Request { id, method, reply },
                )
            }
            Command::Respond {
                id,
                decision,
                reply,
            } => match self.open.remove(&id) {
                None => Vec::from([Effect::Reply(
                    reply,
                    Err(Failure::UnknownApproval(id)),
                )]),
                Some(open) => {
                    let answered = answer(&open.shape, decision);
                    self.settle(id, open, answered, reply)
                }
            },
            Command::Shutdown(waiter) => self.stop(After::Shutdown(waiter)),
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
                    provider: Provider::Codex,
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
                self.open.insert(id.clone(), open);
                Vec::from([Effect::Reply(
                    reply,
                    Err(Failure::DecisionNotApplicable(id)),
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
            (None, Ok(line)) => Vec::from([Effect::Write {
                line: format!("{line}\n"),
                then,
            }]),
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
                Vec::from([Effect::Stop(After::Fail(reply, error))])
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
        match &self.exited {
            None => Vec::from([Effect::Stop(then)]),
            Some(exit) => self.after(then, exit),
        }
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
            .chain(self.after(then, exit))
            .collect()
    }

    fn after<S, E>(
        &self,
        then: After<R, S, E>,
        exit: &Exit,
    ) -> Vec<Effect<R, S, E>> {
        match then {
            After::Nothing => Vec::new(),
            After::Shutdown(waiter) => {
                Vec::from([Effect::Report(waiter, exit.clone())])
            }
            After::Fail(reply, error) => Vec::from([Effect::Reply(
                reply,
                Err(self
                    .refusal
                    .clone()
                    .map_or(Failure::Io(error), Failure::Refused)),
            )]),
            After::End => Vec::from([Effect::End]),
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
            provider: Provider::Codex,
            writing: context,
            error: failure.to_string(),
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
    use proto::Provider;
    use serde_json::{Value, json};

    use super::{After, Command, Effect, Failure, Input, Open, Server, Then};
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

    fn written(msg: Outbound, then: Then<u8>) -> Step {
        Effect::Write {
            line: encode(msg)
                .map_or_else(|error| error.to_string(), |line| line + "\n"),
            then,
        }
    }

    fn request(call: Call, reply: u8) -> Seen {
        Input::Command(Command::Request { call, reply })
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
        let mut server = Server::running();
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
                        Err(Failure::Rpc {
                            method: String::from("model/list"),
                            message: String::from("no"),
                        })
                    )]),
                    Vec::from([
                        written(
                            Outbound::Initialized,
                            Then::Logged(String::from("sending initialized"))
                        ),
                        Effect::Reply(1, Ok(json!({ "userAgent": "c/1" }))),
                    ]),
                    Vec::from([Effect::Log(Note::UnknownResponse {
                        provider: Provider::Codex,
                        request: String::from("Integer(1)"),
                    })]),
                    Vec::from([Effect::Log(Note::UnknownResponse {
                        provider: Provider::Codex,
                        request: String::from("String(\"1\")"),
                    })]),
                ][..]
            )
        );
    }

    #[test]
    fn an_approval_is_answered_once_and_a_withdrawn_one_is_cancelled() {
        let mut server = Server::running();
        let respond = |id: &str, decision, reply| {
            Input::Command(Command::Respond {
                id: ApprovalId(String::from(id)),
                decision,
                reply,
            })
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
                    Err(Failure::DecisionNotApplicable(ApprovalId(
                        String::from("7")
                    )))
                )]),
                Vec::from([
                    Effect::Log(Note::Answering {
                        provider: Provider::Codex,
                        approval: String::from("7"),
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
                    Err(Failure::UnknownApproval(ApprovalId(String::from(
                        "7"
                    ))))
                )]),
                Vec::new(),
                Vec::from([Effect::Emit(CodexEvent::Thread {
                    thread,
                    event: ThreadEvent::ApprovalCancelled {
                        id: ApprovalId(String::from("8")),
                    },
                })]),
                Vec::from([Effect::Reply(
                    4,
                    Err(Failure::UnknownApproval(ApprovalId(String::from(
                        "8"
                    ))))
                )]),
            ]
        );
    }

    #[test]
    fn an_answer_that_does_not_encode_fails_and_closes_the_approval() {
        let mut server = Server::running();
        let id = ApprovalId(String::from("ap_1"));
        let open = Open {
            rpc_id: RequestId::Integer(3),
            thread: ThreadId(String::from("th_1")),
            shape: AnswerShape::Command,
        };
        let failure = serde_json::to_value(BTreeMap::from([((1, 2), 0)]))
            .map_or_else(AnswerError::Encode, |_| AnswerError::NotApplicable);
        let first: Vec<Step> = server.settle(id.clone(), open, Err(failure), 1);
        let again: Vec<Step> = server.step(Input::Command(Command::Respond {
            id,
            decision: Decision::Accept,
            reply: 2,
        }));
        assert_eq!(
            [first, again].map(|effects| match effects.as_slice() {
                [Effect::Reply(_, Err(failure))] => failure.to_string(),
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
        let mut server = Server::<u8>::running();
        let seen: Vec<Step> = server.step(line(&json!({
            "id": 9,
            "method": "item/tool/call",
            "params": { "threadId": "th_1" },
        })));
        assert_eq!(
            &seen[..2],
            [
                Effect::Log(Note::Unsupported {
                    provider: Provider::Codex,
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
        let mut server = Server::running();
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
                    refusal: Some(String::from("bad config")),
                    then: After::Fail(2, "broken pipe"),
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
                Vec::from([Effect::Stop(After::Fail(2, "broken pipe"))]),
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
        let mut server = Server::<u8>::running();
        let seen: Vec<Step> = server.step(Input::Stopped {
            exit: exit(),
            refusal: None,
            then: After::Fail(1, "broken pipe"),
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
        let mut server = Server::<u8>::running();
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
                        provider: Provider::Codex,
                        error: String::from("reset"),
                    }),
                    Effect::Stop(After::Nothing)
                ]),
                Vec::from([Effect::Reply(1, Err(Failure::Io("broken pipe")))]),
                Vec::from([Effect::Log(Note::Unwritten {
                    provider: Provider::Codex,
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
            Failure::DecisionNotApplicable(ApprovalId(String::from("7"))),
        ];
        assert_eq!(
            failures.map(|failure| failure.to_string()),
            [
                "the codex app-server has exited",
                "the codex app-server has exited: bad config",
                "codex i/o: broken pipe",
                "approval 7 cannot be answered with that decision",
            ]
            .map(String::from)
        );
    }
}
