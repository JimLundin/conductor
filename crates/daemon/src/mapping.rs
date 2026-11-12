//! Agent events to protocol events. Pure: a provider event in, the steps the
//! session applies out. Request ids and the turn belong to the session.

use agents::claude::{
    AfterDeny, Block, ClaudeEvent, Outcome, PermissionDecision, PermissionRequestId, Role,
    ToolDetail,
};
use agents::codex::{ApprovalId, ApprovalKind, CodexEvent, Item, ItemPhase, ThreadEvent, TurnId};
use proto::{
    ClientName, CloseReason, Decision, EventKind, ToolCallId, ToolKind, ToolOutcome, ToolPhase,
    TurnOutcome,
};

/// The provider's own id for an open request, which `agents` answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeRequest {
    Claude(PermissionRequestId),
    Codex(ApprovalId),
}

/// What the provider needs to interrupt the running turn: Claude interrupts
/// the session, Codex the turn by its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeTurn {
    Claude,
    Codex(TurnId),
}

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
    /// The session runs the turn, and sends an interrupt it held.
    TurnStarted(NativeTurn),
    /// The agent process is gone: the session logs this and ends.
    Closed(CloseReason),
}

pub(crate) fn claude_steps(event: ClaudeEvent) -> Vec<Step> {
    match event {
        ClaudeEvent::TurnStarted { .. } => Vec::from([Step::TurnStarted(NativeTurn::Claude)]),
        ClaudeEvent::Message { role, content } => content
            .into_iter()
            .filter_map(|block| claude_block(role, block))
            .collect(),
        ClaudeEvent::StreamDelta { text } => {
            Vec::from([Step::Emit(EventKind::AssistantDelta { text })])
        }
        ClaudeEvent::PermissionRequested {
            id,
            tool_name,
            detail,
            tool_use_id,
        } => Vec::from([Step::Open {
            native: NativeRequest::Claude(id),
            call: ToolCallId(tool_use_id),
            tool: tool_kind(tool_name, detail),
        }]),
        ClaudeEvent::PermissionCancelled { id } => {
            Vec::from([Step::Withdraw(NativeRequest::Claude(id))])
        }
        ClaudeEvent::TurnFinished {
            subtype,
            outcome,
            terminal_reason,
            ..
        } => Vec::from([Step::TurnEnded(
            match (outcome, terminal_reason.as_deref()) {
                (Outcome::Succeeded, _) => TurnOutcome::Completed,
                (Outcome::Failed, Some("aborted_streaming" | "aborted_tools")) => {
                    TurnOutcome::Interrupted
                }
                (Outcome::Failed, _) => TurnOutcome::Failed { message: subtype },
            },
        )]),
        ClaudeEvent::Exited { status } => Vec::from([Step::Closed(CloseReason::Exited {
            status: status.describe(),
        })]),
        ClaudeEvent::Unknown(frame) => {
            tracing::trace!(%frame, "unmapped claude frame");
            Vec::new()
        }
    }
}

fn claude_block(role: Role, block: Block) -> Option<Step> {
    match (role, block) {
        (Role::Assistant, Block::Text(text)) => {
            Some(Step::Emit(EventKind::AssistantMessage { text }))
        }
        (Role::Assistant, Block::ToolUse { id, name, detail }) => {
            Some(Step::Emit(EventKind::ToolCall {
                call: ToolCallId(id),
                phase: ToolPhase::Started {
                    tool: tool_kind(name, detail),
                },
            }))
        }
        (
            Role::User,
            Block::ToolResult {
                tool_use_id,
                outcome,
                ..
            },
        ) => Some(Step::ToolFinished {
            call: ToolCallId(tool_use_id),
            outcome: match outcome {
                Outcome::Succeeded => ToolOutcome::Succeeded,
                Outcome::Failed => ToolOutcome::Failed,
            },
        }),
        (Role::User, Block::Text(_) | Block::ToolUse { .. })
        | (Role::Assistant, Block::ToolResult { .. })
        | (_, Block::Thinking(_) | Block::Other(_)) => None,
    }
}

fn tool_kind(name: String, detail: ToolDetail) -> ToolKind {
    match (name.as_str(), detail) {
        ("Edit" | "Write" | "MultiEdit", ToolDetail::Path(path)) => ToolKind::FileChange {
            paths: Vec::from([path]),
        },
        (_, ToolDetail::Command(command)) => ToolKind::Command { command },
        (_, ToolDetail::Path(detail) | ToolDetail::Text(detail)) => {
            ToolKind::Other { name, detail }
        }
    }
}

pub(crate) fn claude_answer(decision: Decision, by: &ClientName) -> PermissionDecision {
    match decision {
        Decision::Approve => PermissionDecision::Allow,
        Decision::Deny => PermissionDecision::Deny {
            message: format!("Denied by {}", by.0),
            then: AfterDeny::Continue,
        },
    }
}

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
        ThreadEvent::TurnStarted { turn } => {
            Vec::from([Step::TurnStarted(NativeTurn::Codex(turn))])
        }
        ThreadEvent::AgentDelta { text, .. } => {
            Vec::from([Step::Emit(EventKind::AssistantDelta { text })])
        }
        ThreadEvent::AgentMessage { text, .. } => {
            Vec::from([Step::Emit(EventKind::AssistantMessage { text })])
        }
        ThreadEvent::Item {
            phase: ItemPhase::Started,
            item,
            ..
        } => Vec::from([Step::Emit(codex_item(item))]),
        ThreadEvent::Item {
            phase: ItemPhase::Completed,
            item: Item::Command { id, status, .. } | Item::FileChange { id, status, .. },
            ..
        } => Vec::from([Step::ToolFinished {
            call: ToolCallId(id),
            outcome: codex_outcome(&status),
        }]),
        ThreadEvent::ApprovalRequested {
            id,
            item,
            kind: ApprovalKind::Command { command, .. },
            ..
        } => codex_open(
            id,
            item,
            ToolKind::Command {
                command: command.unwrap_or_default(),
            },
        ),
        // The request names no paths; the tool card the item started has them.
        ThreadEvent::ApprovalRequested {
            id,
            item,
            kind: ApprovalKind::FileChange { .. },
            ..
        } => codex_open(id, item, ToolKind::FileChange { paths: Vec::new() }),
        ThreadEvent::ApprovalRequested {
            id,
            item,
            kind: ApprovalKind::Permissions { reason, .. },
            ..
        } => codex_open(
            id,
            item,
            ToolKind::Other {
                name: String::from("permissions"),
                detail: reason.unwrap_or_default(),
            },
        ),
        ThreadEvent::ApprovalRequested {
            kind: ApprovalKind::UserInput { .. },
            ..
        } => Vec::from([Step::Emit(EventKind::Error {
            message: String::from(
                "Codex asked a question. Questions arrive in a later slice; interrupt to continue.",
            ),
        })]),
        ThreadEvent::ApprovalCancelled { id } => {
            Vec::from([Step::Withdraw(NativeRequest::Codex(id))])
        }
        ThreadEvent::TurnCompleted { outcome, .. } => Vec::from([Step::TurnEnded(match outcome {
            agents::codex::TurnOutcome::Completed => TurnOutcome::Completed,
            agents::codex::TurnOutcome::Interrupted => TurnOutcome::Interrupted,
            agents::codex::TurnOutcome::Failed { message } => TurnOutcome::Failed {
                message: message.unwrap_or_else(|| String::from("the turn failed")),
            },
        })]),
        ThreadEvent::Unknown(frame) => {
            tracing::trace!(%frame, "unmapped codex thread frame");
            Vec::new()
        }
    }
}

fn codex_item(item: Item) -> EventKind {
    let (id, tool) = match item {
        Item::Command { id, command, .. } => (id, ToolKind::Command { command }),
        Item::FileChange { id, changes, .. } => (
            id,
            ToolKind::FileChange {
                paths: changes.into_iter().map(|change| change.path).collect(),
            },
        ),
    };
    EventKind::ToolCall {
        call: ToolCallId(id),
        phase: ToolPhase::Started { tool },
    }
}

/// Codex's item `status`, as a tool call's outcome.
fn codex_outcome(status: &str) -> ToolOutcome {
    match status {
        "completed" => ToolOutcome::Succeeded,
        "declined" => ToolOutcome::Declined,
        _ => ToolOutcome::Failed,
    }
}

fn codex_open(id: ApprovalId, item: String, tool: ToolKind) -> Vec<Step> {
    Vec::from([Step::Open {
        native: NativeRequest::Codex(id),
        call: ToolCallId(item),
        tool,
    }])
}

pub(crate) fn codex_answer(decision: Decision) -> agents::codex::Decision {
    match decision {
        Decision::Approve => agents::codex::Decision::Accept,
        Decision::Deny => agents::codex::Decision::Decline,
    }
}

#[cfg(test)]
mod tests {
    use agents::Exit;
    use agents::claude::{
        Block, ClaudeEvent, Outcome, PermissionRequestId, Role, SessionId, ToolDetail,
    };
    use agents::codex::{
        ApprovalId, ApprovalKind, ChangeKind, CodexEvent, FileChange, Item, ItemPhase, ThreadEvent,
        ThreadId, TurnId,
    };
    use proto::{
        CloseReason, EventKind, ToolCallId, ToolKind, ToolOutcome, ToolPhase, TurnOutcome,
    };

    use super::{NativeRequest, NativeTurn, Step, claude_steps, codex_steps};

    struct Case {
        event: ClaudeEvent,
        steps: Vec<Step>,
    }

    fn finished(outcome: Outcome, terminal_reason: Option<&str>) -> ClaudeEvent {
        ClaudeEvent::TurnFinished {
            subtype: String::from("error_during_execution"),
            outcome,
            terminal_reason: terminal_reason.map(String::from),
            session_id: SessionId(String::from("s")),
        }
    }

    fn tool_use(name: &str, detail: ToolDetail) -> Block {
        Block::ToolUse {
            id: String::from("toolu_1"),
            name: String::from(name),
            detail,
        }
    }

    fn started(tool: ToolKind) -> Step {
        Step::Emit(EventKind::ToolCall {
            call: ToolCallId(String::from("toolu_1")),
            phase: ToolPhase::Started { tool },
        })
    }

    #[test]
    fn each_claude_event_maps_to_its_steps() {
        let cases = Vec::from([
            Case {
                event: ClaudeEvent::Message {
                    role: Role::Assistant,
                    content: Vec::from([
                        Block::Thinking(String::from("hmm")),
                        tool_use("Read", ToolDetail::Path(String::from("b.txt"))),
                        tool_use("Bash", ToolDetail::Command(String::from("ls"))),
                    ]),
                },
                steps: Vec::from([
                    started(ToolKind::Other {
                        name: String::from("Read"),
                        detail: String::from("b.txt"),
                    }),
                    started(ToolKind::Command {
                        command: String::from("ls"),
                    }),
                ]),
            },
            Case {
                event: ClaudeEvent::PermissionCancelled {
                    id: PermissionRequestId(String::from("p1")),
                },
                steps: Vec::from([Step::Withdraw(NativeRequest::Claude(PermissionRequestId(
                    String::from("p1"),
                )))]),
            },
            Case {
                event: finished(Outcome::Failed, Some("aborted_tools")),
                steps: Vec::from([Step::TurnEnded(TurnOutcome::Interrupted)]),
            },
            Case {
                event: finished(Outcome::Failed, None),
                steps: Vec::from([Step::TurnEnded(TurnOutcome::Failed {
                    message: String::from("error_during_execution"),
                })]),
            },
            Case {
                event: ClaudeEvent::Exited {
                    status: Exit::Unknown(String::from("gone")),
                },
                steps: Vec::from([Step::Closed(CloseReason::Exited {
                    status: String::from("unknown exit (gone)"),
                })]),
            },
        ]);
        let (events, expected) = cases
            .into_iter()
            .map(|case| (case.event, case.steps))
            .unzip::<ClaudeEvent, Vec<Step>, Vec<ClaudeEvent>, Vec<Vec<Step>>>();
        assert_eq!(
            events
                .into_iter()
                .map(claude_steps)
                .collect::<Vec<Vec<Step>>>(),
            expected
        );
    }

    struct CodexCase {
        event: ThreadEvent,
        steps: Vec<Step>,
    }

    fn turn() -> TurnId {
        TurnId(String::from("tu_1"))
    }

    fn asked(kind: ApprovalKind) -> ThreadEvent {
        ThreadEvent::ApprovalRequested {
            id: ApprovalId(String::from("0")),
            turn: turn(),
            item: String::from("exec-1"),
            kind,
            options: Vec::new(),
        }
    }

    #[test]
    fn each_codex_event_maps_to_its_steps() {
        let cases = Vec::from([
            CodexCase {
                event: ThreadEvent::TurnStarted { turn: turn() },
                steps: Vec::from([Step::TurnStarted(NativeTurn::Codex(turn()))]),
            },
            CodexCase {
                event: ThreadEvent::Item {
                    turn: turn(),
                    phase: ItemPhase::Started,
                    item: Item::FileChange {
                        id: String::from("patch-1"),
                        changes: Vec::from([FileChange {
                            path: String::from("a.txt"),
                            kind: ChangeKind::Add,
                        }]),
                        status: String::from("inProgress"),
                    },
                },
                steps: Vec::from([Step::Emit(EventKind::ToolCall {
                    call: ToolCallId(String::from("patch-1")),
                    phase: ToolPhase::Started {
                        tool: ToolKind::FileChange {
                            paths: Vec::from([String::from("a.txt")]),
                        },
                    },
                })]),
            },
            CodexCase {
                event: ThreadEvent::Item {
                    turn: turn(),
                    phase: ItemPhase::Completed,
                    item: Item::Command {
                        id: String::from("exec-1"),
                        command: String::from("ls"),
                        status: String::from("declined"),
                        exit_code: None,
                    },
                },
                steps: Vec::from([Step::ToolFinished {
                    call: ToolCallId(String::from("exec-1")),
                    outcome: ToolOutcome::Declined,
                }]),
            },
            CodexCase {
                event: asked(ApprovalKind::Command {
                    command: Some(String::from("ls")),
                    cwd: None,
                    reason: None,
                }),
                steps: Vec::from([Step::Open {
                    native: NativeRequest::Codex(ApprovalId(String::from("0"))),
                    call: ToolCallId(String::from("exec-1")),
                    tool: ToolKind::Command {
                        command: String::from("ls"),
                    },
                }]),
            },
            CodexCase {
                event: asked(ApprovalKind::UserInput {
                    questions: serde_json::Value::Null,
                }),
                steps: Vec::from([Step::Emit(EventKind::Error {
                    message: String::from(
                        "Codex asked a question. Questions arrive in a later slice; interrupt to continue.",
                    ),
                })]),
            },
            CodexCase {
                event: ThreadEvent::TurnCompleted {
                    turn: turn(),
                    outcome: agents::codex::TurnOutcome::Failed { message: None },
                },
                steps: Vec::from([Step::TurnEnded(TurnOutcome::Failed {
                    message: String::from("the turn failed"),
                })]),
            },
        ]);
        let (events, expected) = cases
            .into_iter()
            .map(|case| (case.event, case.steps))
            .unzip::<ThreadEvent, Vec<Step>, Vec<ThreadEvent>, Vec<Vec<Step>>>();
        assert_eq!(
            events
                .into_iter()
                .map(|event| codex_steps(CodexEvent::Thread {
                    thread: ThreadId(String::from("th_1")),
                    event,
                }))
                .collect::<Vec<Vec<Step>>>(),
            expected
        );
    }
}
