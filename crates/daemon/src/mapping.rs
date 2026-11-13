//! Agent events to protocol events. Pure: a provider event in, the steps the
//! session applies out. Request ids and the turn belong to the session.

use agents::claude::{ASK_USER_QUESTION, AfterDeny, Block, ClaudeEvent, PermissionDecision, Role};
use agents::codex::{self, ApprovalKind, CodexEvent, PermissionAsk, ThreadEvent};
use proto::{
    ClientName, CloseReason, Decision, EventKind, Question, ToolCallId, ToolKind, ToolOutcome,
    ToolPhase, TurnOutcome,
};

use crate::session::Reply;

/// An open request's id as the provider gave it. The type doesn't say which
/// provider: a session's steps come only from its own agent's events
/// (`Agent::next`), so its agent reads the id back as its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeRequest(pub(crate) String);

/// The running turn as the provider interrupts it, by the provider's id:
/// Claude's session, Codex's turn. Read like [`NativeRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NativeTurn(pub(crate) String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    Emit(EventKind),
    /// The session assigns the `RequestId`, stores it open and logs
    /// `RequestOpened`.
    Open {
        native: NativeRequest,
        /// The tool call the request asks about.
        call: ToolCallId,
        tool: ToolKind,
        reason: Option<String>,
        /// What approving for the session allows, if the agent offers it.
        for_session: Option<String>,
    },
    /// The session assigns the `RequestId`, stores the question open and
    /// logs `QuestionAsked`.
    Ask {
        native: NativeRequest,
        questions: Vec<Question>,
    },
    /// The session logs the call finished, as `Declined` if it failed after
    /// the user denied it.
    ToolFinished {
        call: ToolCallId,
        outcome: ToolOutcome,
    },
    /// The session marks the request withdrawn and logs it.
    Withdraw(NativeRequest),
    /// The session goes idle and logs `TurnCompleted`.
    TurnEnded(TurnOutcome),
    HistoryBoundary(Option<String>),
    /// The session runs the turn, and sends an interrupt it held.
    TurnStarted(NativeTurn),
    /// The agent process is gone: the session logs this and waits for a
    /// `Continue`.
    Closed(CloseReason),
}

pub(crate) fn claude_steps(event: ClaudeEvent) -> Vec<Step> {
    match event {
        ClaudeEvent::TurnStarted { session_id } => {
            Vec::from([Step::TurnStarted(NativeTurn(session_id.0))])
        }
        ClaudeEvent::Message { role, content, .. } => content
            .into_iter()
            .filter_map(|block| claude_block(role, block))
            .collect(),
        ClaudeEvent::StreamDelta { text } => {
            Vec::from([Step::Emit(EventKind::AssistantDelta { text })])
        }
        ClaudeEvent::ThinkingDelta { text } => reasoning_delta(text).into_iter().collect(),
        ClaudeEvent::PermissionRequested {
            id,
            tool,
            tool_name: _,
            tool_use_id,
            for_session,
        } => Vec::from([Step::Open {
            native: NativeRequest(id.0),
            call: ToolCallId(tool_use_id),
            tool,
            reason: None,
            for_session,
        }]),
        ClaudeEvent::QuestionAsked { id, questions, .. } => Vec::from([Step::Ask {
            native: NativeRequest(id.0),
            questions,
        }]),
        ClaudeEvent::PermissionCancelled { id } => Vec::from([Step::Withdraw(NativeRequest(id.0))]),
        ClaudeEvent::TurnFinished {
            outcome,
            last_message,
            ..
        } => Vec::from([
            Step::HistoryBoundary(last_message.map(|message| message.0)),
            Step::TurnEnded(outcome),
        ]),
        ClaudeEvent::Exited { status } => Vec::from([Step::Closed(CloseReason::Exited {
            status: status.describe(),
        })]),
        ClaudeEvent::Unknown(frame) => {
            tracing::trace!(%frame, "unmapped claude frame");
            Vec::new()
        }
    }
}

/// A question's `AskUserQuestion` call is not shown as a tool call: its
/// `QuestionAsked` shows it. Its result then finishes a call no client saw
/// start, which a client ignores.
fn claude_block(role: Role, block: Block) -> Option<Step> {
    match (role, block) {
        (Role::Assistant, Block::Text(text)) => {
            Some(Step::Emit(EventKind::AssistantMessage { text }))
        }
        (Role::Assistant, Block::Thinking(text)) => reasoning(text),
        (Role::Assistant, Block::ToolUse { id, name, tool }) => match name.as_str() {
            ASK_USER_QUESTION => None,
            _ => Some(Step::Emit(EventKind::ToolCall {
                call: ToolCallId(id),
                phase: ToolPhase::Started { tool },
            })),
        },
        (
            Role::User,
            Block::ToolResult {
                tool_use_id,
                outcome,
                ..
            },
        ) => Some(Step::ToolFinished {
            call: ToolCallId(tool_use_id),
            outcome,
        }),
        (Role::User, Block::Text(_) | Block::ToolUse { .. })
        | (Role::Assistant, Block::ToolResult { .. })
        | (Role::User, Block::Thinking(_))
        | (_, Block::Other(_)) => None,
    }
}

/// A finished stretch of reasoning. Without its text (Claude's signature
/// alone, a Codex item with no summary) there is nothing to show.
fn reasoning(text: String) -> Option<Step> {
    Some(text)
        .filter(|text| !text.is_empty())
        .map(|text| Step::Emit(EventKind::Reasoning { text }))
}

/// A piece of reasoning as it streams. Claude streams one empty piece for
/// a block with no text, which would show reasoning that never comes.
fn reasoning_delta(text: String) -> Option<Step> {
    Some(text)
        .filter(|text| !text.is_empty())
        .map(|text| Step::Emit(EventKind::ReasoningDelta { text }))
}

pub(crate) fn claude_answer(reply: Reply, by: &ClientName) -> PermissionDecision {
    match reply {
        Reply::Decide(Decision::Approve) => PermissionDecision::Allow,
        Reply::Decide(Decision::ApproveForSession) => PermissionDecision::AllowForSession,
        Reply::Decide(Decision::Deny) => PermissionDecision::Deny {
            message: format!("Denied by {}", by.0),
            then: AfterDeny::Continue,
        },
        Reply::Answer(answers) => PermissionDecision::Answer(answers),
    }
}

/// Why a Codex question went unasked. Every answer is stored and sent to
/// every client, so none can be kept secret.
const SECRET_DECLINED: &str = "Codex asked for an answer it would keep secret. Conductor can't keep answers secret, so it declined the question.";

pub(crate) fn codex_steps(event: CodexEvent) -> Vec<Step> {
    match event {
        CodexEvent::Thread { event, .. } => codex_thread_steps(event),
        CodexEvent::Exited { status } => Vec::from([Step::Closed(CloseReason::Exited {
            status: status.describe(),
        })]),
        CodexEvent::Unknown(frame) => {
            tracing::trace!(%frame, "unmapped codex frame");
            Vec::new()
        }
    }
}

fn codex_thread_steps(event: ThreadEvent) -> Vec<Step> {
    match event {
        ThreadEvent::Started => Vec::new(),
        ThreadEvent::TurnStarted { turn } => Vec::from([Step::TurnStarted(NativeTurn(turn.0))]),
        ThreadEvent::AgentDelta { text, .. } => {
            Vec::from([Step::Emit(EventKind::AssistantDelta { text })])
        }
        ThreadEvent::AgentMessage { text, .. } => {
            Vec::from([Step::Emit(EventKind::AssistantMessage { text })])
        }
        ThreadEvent::ReasoningDelta { text, .. } => reasoning_delta(text).into_iter().collect(),
        ThreadEvent::Reasoning { text, .. } => reasoning(text).into_iter().collect(),
        ThreadEvent::ToolCall {
            call,
            phase: ToolPhase::Started { tool },
            ..
        } => Vec::from([Step::Emit(EventKind::ToolCall {
            call,
            phase: ToolPhase::Started { tool },
        })]),
        ThreadEvent::ToolCall {
            call,
            phase: ToolPhase::Finished { outcome },
            ..
        } => Vec::from([Step::ToolFinished { call, outcome }]),
        ThreadEvent::ApprovalRequested { id, item, kind, .. } => {
            Vec::from([approval_step(NativeRequest(id.0), ToolCallId(item), kind)])
        }
        ThreadEvent::ApprovalCancelled { id } => Vec::from([Step::Withdraw(NativeRequest(id.0))]),
        ThreadEvent::SecretQuestionDeclined { .. } => Vec::from([Step::Emit(EventKind::Error {
            message: String::from(SECRET_DECLINED),
        })]),
        ThreadEvent::TurnCompleted { outcome, .. } => Vec::from([Step::TurnEnded(outcome)]),
        ThreadEvent::Unknown(frame) => {
            tracing::trace!(%frame, "unmapped codex thread frame");
            Vec::new()
        }
    }
}

/// A server request: a question, or an approval with the tool it asks
/// about, Codex's reason, and what `acceptForSession` allows. Codex takes
/// `acceptForSession` for a command, though `availableDecisions` omits it;
/// `s2` step 8 checks that it still does (PROTOCOL.md).
fn approval_step(native: NativeRequest, call: ToolCallId, kind: ApprovalKind) -> Step {
    match kind {
        ApprovalKind::Command {
            command, reason, ..
        } => {
            let command = command.unwrap_or_default();
            Step::Open {
                native,
                call,
                tool: ToolKind::Command {
                    command: command.clone(),
                },
                reason,
                for_session: Some(command),
            }
        }
        // The request names no paths; the tool card the item started has them.
        ApprovalKind::FileChange { reason, .. } => Step::Open {
            native,
            call,
            tool: ToolKind::FileChange { files: Vec::new() },
            reason,
            // No run has recorded a file change approval yet (PROTOCOL.md).
            for_session: None,
        },
        ApprovalKind::Permissions { reason, asks } => {
            let detail = asks
                .into_iter()
                .map(|ask| match ask {
                    PermissionAsk::Network => String::from("network access"),
                    PermissionAsk::Read(places) => format!("read {}", places.join(", ")),
                    PermissionAsk::Write(places) => format!("write to {}", places.join(", ")),
                })
                .collect::<Vec<String>>()
                .join("; ");
            Step::Open {
                native,
                call,
                tool: ToolKind::Other {
                    name: String::from("permissions"),
                    detail: detail.clone(),
                },
                reason,
                for_session: Some(detail),
            }
        }
        ApprovalKind::UserInput { questions } => Step::Ask { native, questions },
    }
}

pub(crate) fn codex_answer(reply: Reply) -> codex::Decision {
    match reply {
        Reply::Decide(Decision::Approve) => codex::Decision::Accept,
        Reply::Decide(Decision::ApproveForSession) => codex::Decision::AcceptForSession,
        Reply::Decide(Decision::Deny) => codex::Decision::Decline,
        Reply::Answer(answers) => codex::Decision::Answer(answers),
    }
}

#[cfg(test)]
mod tests {
    use agents::Exit;
    use agents::claude::{
        AfterDeny, Block, ClaudeEvent, PermissionDecision, PermissionRequestId, Role,
    };
    use agents::codex::{
        self, ApprovalId, ApprovalKind, CodexEvent, PermissionAsk, ThreadEvent, ThreadId, TurnId,
    };
    use proto::{
        Answer, ClientName, CloseReason, Decision, EventKind, Question, QuestionOption, Selection,
        ToolCallId, ToolKind,
    };

    use super::{NativeRequest, Step, claude_answer, claude_steps, codex_answer, codex_steps};
    use crate::session::Reply;

    /// The steps one provider event maps to, and the steps it should.
    struct Case {
        got: Vec<Step>,
        want: Vec<Step>,
    }

    fn tool_use(name: &str, tool: ToolKind) -> Block {
        Block::ToolUse {
            id: String::from("toolu_1"),
            name: String::from(name),
            tool,
        }
    }

    fn on_thread(event: ThreadEvent) -> Vec<Step> {
        codex_steps(CodexEvent::Thread {
            thread: ThreadId(String::from("th_1")),
            event,
        })
    }

    fn turn() -> TurnId {
        TurnId(String::from("tu_1"))
    }

    fn color() -> Question {
        Question {
            header: String::from("Color"),
            text: String::from("Which color?"),
            options: Vec::from([QuestionOption {
                label: String::from("Blue"),
                description: String::from("Cool"),
            }]),
            selection: Selection::One,
        }
    }

    #[test]
    fn each_reply_maps_to_each_providers_answer() {
        let by = ClientName(String::from("phone"));
        let answers = Vec::from([Answer(Vec::from([String::from("Blue")]))]);
        let replies = Vec::from([
            Reply::Decide(Decision::Approve),
            Reply::Decide(Decision::ApproveForSession),
            Reply::Decide(Decision::Deny),
            Reply::Answer(answers.clone()),
        ]);
        assert_eq!(
            replies
                .iter()
                .cloned()
                .map(|reply| claude_answer(reply, &by))
                .collect::<Vec<PermissionDecision>>(),
            Vec::from([
                PermissionDecision::Allow,
                PermissionDecision::AllowForSession,
                PermissionDecision::Deny {
                    message: String::from("Denied by phone"),
                    then: AfterDeny::Continue,
                },
                PermissionDecision::Answer(answers.clone()),
            ])
        );
        assert_eq!(
            replies
                .into_iter()
                .map(codex_answer)
                .collect::<Vec<codex::Decision>>(),
            Vec::from([
                codex::Decision::Accept,
                codex::Decision::AcceptForSession,
                codex::Decision::Decline,
                codex::Decision::Answer(answers),
            ])
        );
    }

    #[test]
    fn each_provider_event_maps_to_its_steps() {
        let cases = [
            Case {
                got: claude_steps(ClaudeEvent::ThinkingDelta {
                    text: String::from("hm"),
                }),
                want: Vec::from([Step::Emit(EventKind::ReasoningDelta {
                    text: String::from("hm"),
                })]),
            },
            Case {
                got: claude_steps(ClaudeEvent::ThinkingDelta {
                    text: String::new(),
                }),
                want: Vec::new(),
            },
            Case {
                got: on_thread(ThreadEvent::ReasoningDelta {
                    turn: turn(),
                    text: String::from("**Plan**"),
                }),
                want: Vec::from([Step::Emit(EventKind::ReasoningDelta {
                    text: String::from("**Plan**"),
                })]),
            },
            Case {
                got: on_thread(ThreadEvent::Reasoning {
                    turn: turn(),
                    text: String::from("**Plan**\n\nList it."),
                }),
                want: Vec::from([Step::Emit(EventKind::Reasoning {
                    text: String::from("**Plan**\n\nList it."),
                })]),
            },
            Case {
                got: on_thread(ThreadEvent::Reasoning {
                    turn: turn(),
                    text: String::new(),
                }),
                want: Vec::new(),
            },
            Case {
                got: claude_steps(ClaudeEvent::Message {
                    uuid: None,
                    role: Role::Assistant,
                    content: Vec::from([
                        Block::Thinking(String::from("hmm")),
                        Block::Thinking(String::new()),
                    ]),
                }),
                want: Vec::from([Step::Emit(EventKind::Reasoning {
                    text: String::from("hmm"),
                })]),
            },
            Case {
                got: claude_steps(ClaudeEvent::PermissionCancelled {
                    id: PermissionRequestId(String::from("p1")),
                }),
                want: Vec::from([Step::Withdraw(NativeRequest(String::from("p1")))]),
            },
            Case {
                got: claude_steps(ClaudeEvent::Exited {
                    status: Exit::Unknown(String::from("gone")),
                }),
                want: Vec::from([Step::Closed(CloseReason::Exited {
                    status: String::from("unknown exit (gone)"),
                })]),
            },
            Case {
                got: on_thread(ThreadEvent::ApprovalRequested {
                    id: ApprovalId(String::from("1")),
                    turn: turn(),
                    item: String::from("exec-2"),
                    kind: ApprovalKind::Command {
                        command: Some(String::from("touch ~/b.txt")),
                        cwd: None,
                        reason: Some(String::from("needs to write outside the workspace")),
                    },
                    options: Vec::new(),
                }),
                want: Vec::from([Step::Open {
                    native: NativeRequest(String::from("1")),
                    call: ToolCallId(String::from("exec-2")),
                    tool: ToolKind::Command {
                        command: String::from("touch ~/b.txt"),
                    },
                    reason: Some(String::from("needs to write outside the workspace")),
                    for_session: Some(String::from("touch ~/b.txt")),
                }]),
            },
            Case {
                got: on_thread(ThreadEvent::ApprovalRequested {
                    id: ApprovalId(String::from("2")),
                    turn: turn(),
                    item: String::from("exec-3"),
                    kind: ApprovalKind::Permissions {
                        reason: Some(String::from("to fetch the crate")),
                        asks: Vec::from([
                            PermissionAsk::Network,
                            PermissionAsk::Write(Vec::from([
                                String::from("/home/u/.cargo"),
                                String::from("/tmp"),
                            ])),
                        ]),
                    },
                    options: Vec::new(),
                }),
                want: Vec::from([Step::Open {
                    native: NativeRequest(String::from("2")),
                    call: ToolCallId(String::from("exec-3")),
                    tool: ToolKind::Other {
                        name: String::from("permissions"),
                        detail: String::from("network access; write to /home/u/.cargo, /tmp"),
                    },
                    reason: Some(String::from("to fetch the crate")),
                    for_session: Some(String::from(
                        "network access; write to /home/u/.cargo, /tmp",
                    )),
                }]),
            },
            Case {
                got: on_thread(ThreadEvent::ApprovalRequested {
                    id: ApprovalId(String::from("0")),
                    turn: turn(),
                    item: String::from("exec-1"),
                    kind: ApprovalKind::UserInput {
                        questions: Vec::from([color()]),
                    },
                    options: Vec::new(),
                }),
                want: Vec::from([Step::Ask {
                    native: NativeRequest(String::from("0")),
                    questions: Vec::from([color()]),
                }]),
            },
            Case {
                got: claude_steps(ClaudeEvent::Message {
                    uuid: None,
                    role: Role::Assistant,
                    content: Vec::from([tool_use(
                        "AskUserQuestion",
                        ToolKind::Other {
                            name: String::from("AskUserQuestion"),
                            detail: String::from("{}"),
                        },
                    )]),
                }),
                want: Vec::new(),
            },
            Case {
                got: claude_steps(ClaudeEvent::QuestionAsked {
                    id: PermissionRequestId(String::from("p2")),
                    questions: Vec::from([color()]),
                    tool_use_id: String::from("toolu_2"),
                }),
                want: Vec::from([Step::Ask {
                    native: NativeRequest(String::from("p2")),
                    questions: Vec::from([color()]),
                }]),
            },
            Case {
                got: claude_steps(ClaudeEvent::PermissionRequested {
                    id: PermissionRequestId(String::from("p3")),
                    tool_name: String::from("Bash"),
                    tool: ToolKind::Command {
                        command: String::from("touch t1.txt"),
                    },
                    tool_use_id: String::from("toolu_3"),
                    for_session: Some(String::from("Bash(touch t1.txt)")),
                }),
                want: Vec::from([Step::Open {
                    native: NativeRequest(String::from("p3")),
                    call: ToolCallId(String::from("toolu_3")),
                    tool: ToolKind::Command {
                        command: String::from("touch t1.txt"),
                    },
                    reason: None,
                    for_session: Some(String::from("Bash(touch t1.txt)")),
                }]),
            },
            Case {
                got: on_thread(ThreadEvent::SecretQuestionDeclined { turn: turn() }),
                want: Vec::from([Step::Emit(EventKind::Error {
                    message: String::from(
                        "Codex asked for an answer it would keep secret. Conductor can't keep answers secret, so it declined the question.",
                    ),
                })]),
            },
        ];
        let (got, want) = cases
            .into_iter()
            .map(|case| (case.got, case.want))
            .unzip::<Vec<Step>, Vec<Step>, Vec<Vec<Step>>, Vec<Vec<Step>>>();
        assert_eq!(got, want);
    }
}
