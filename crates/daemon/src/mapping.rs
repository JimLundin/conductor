//! Agent events to protocol events. Pure: a provider event in, the steps the
//! session applies out. Request ids and the turn belong to the session.

use agents::claude::{
    AfterDeny, Block, ClaudeEvent, Outcome, PermissionDecision, Role, ToolDetail,
};
use agents::codex::{
    self, ApprovalKind, ChangeKind, CodexEvent, Item, ItemPhase, PermissionAsk, ThreadEvent,
};
use proto::{
    Change, ClientName, CloseReason, Decision, EventKind, FileChange, ToolCallId, ToolKind,
    ToolOutcome, ToolPhase, TurnOutcome,
};

/// An open request's id as the provider gave it. Only the session's own
/// agent reads it, as its provider's id, so it can't name another
/// provider's request.
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
        ClaudeEvent::TurnStarted { session_id } => {
            Vec::from([Step::TurnStarted(NativeTurn(session_id.0))])
        }
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
            native: NativeRequest(id.0),
            call: ToolCallId(tool_use_id),
            tool: tool_kind(tool_name, detail),
            reason: None,
        }]),
        ClaudeEvent::PermissionCancelled { id } => Vec::from([Step::Withdraw(NativeRequest(id.0))]),
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
        ("Write", ToolDetail::Path(path)) => one_file(path, Change::Write),
        ("Edit" | "MultiEdit", ToolDetail::Path(path)) => one_file(path, Change::Edit),
        (_, ToolDetail::Command(command)) => ToolKind::Command { command },
        (_, ToolDetail::Path(detail) | ToolDetail::Text(detail)) => {
            ToolKind::Other { name, detail }
        }
    }
}

fn one_file(path: String, change: Change) -> ToolKind {
    ToolKind::FileChange {
        files: Vec::from([FileChange { path, change }]),
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
        ThreadEvent::TurnStarted { turn } => Vec::from([Step::TurnStarted(NativeTurn(turn.0))]),
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
        ThreadEvent::ApprovalRequested { id, item, kind, .. } => match approval_tool(kind) {
            Some((tool, reason)) => Vec::from([Step::Open {
                native: NativeRequest(id.0),
                call: ToolCallId(item),
                tool,
                reason,
            }]),
            None => Vec::from([Step::Emit(EventKind::Error {
                message: String::from(
                    "Codex asked a question. Questions arrive in a later slice; interrupt to continue.",
                ),
            })]),
        },
        ThreadEvent::ApprovalCancelled { id } => Vec::from([Step::Withdraw(NativeRequest(id.0))]),
        ThreadEvent::TurnCompleted { outcome, .. } => Vec::from([Step::TurnEnded(match outcome {
            codex::TurnOutcome::Completed => TurnOutcome::Completed,
            codex::TurnOutcome::Interrupted => TurnOutcome::Interrupted,
            codex::TurnOutcome::Failed { message } => TurnOutcome::Failed {
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
                files: changes
                    .into_iter()
                    .map(|file| FileChange {
                        path: file.path,
                        change: match file.kind {
                            ChangeKind::Add => Change::Write,
                            ChangeKind::Update => Change::Edit,
                            ChangeKind::Delete => Change::Delete,
                        },
                    })
                    .collect(),
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

/// The tool an approval asks about, and Codex's reason for asking. A
/// question asks about none.
fn approval_tool(kind: ApprovalKind) -> Option<(ToolKind, Option<String>)> {
    match kind {
        ApprovalKind::Command {
            command, reason, ..
        } => Some((
            ToolKind::Command {
                command: command.unwrap_or_default(),
            },
            reason,
        )),
        // The request names no paths; the tool card the item started has them.
        ApprovalKind::FileChange { reason, .. } => {
            Some((ToolKind::FileChange { files: Vec::new() }, reason))
        }
        ApprovalKind::Permissions { reason, asks } => Some((
            ToolKind::Other {
                name: String::from("permissions"),
                detail: asks
                    .into_iter()
                    .map(|ask| match ask {
                        PermissionAsk::Network => String::from("network access"),
                        PermissionAsk::Read(places) => format!("read {}", places.join(", ")),
                        PermissionAsk::Write(places) => format!("write to {}", places.join(", ")),
                    })
                    .collect::<Vec<String>>()
                    .join("; "),
            },
            reason,
        )),
        ApprovalKind::UserInput { .. } => None,
    }
}

pub(crate) fn codex_answer(decision: Decision) -> codex::Decision {
    match decision {
        Decision::Approve => codex::Decision::Accept,
        Decision::Deny => codex::Decision::Decline,
    }
}

#[cfg(test)]
mod tests {
    use agents::Exit;
    use agents::claude::{
        Block, ClaudeEvent, Outcome, PermissionRequestId, Role, SessionId, ToolDetail,
    };
    use agents::codex::{
        self, ApprovalId, ApprovalKind, ChangeKind, CodexEvent, FileChange, Item, ItemPhase,
        PermissionAsk, ThreadEvent, ThreadId, TurnId,
    };
    use proto::{Change, CloseReason, EventKind, ToolCallId, ToolKind, ToolPhase, TurnOutcome};

    use super::{NativeRequest, Step, claude_steps, codex_steps};

    /// The steps one provider event maps to, and the steps it should.
    struct Case {
        got: Vec<Step>,
        want: Vec<Step>,
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

    fn started(id: &str, tool: ToolKind) -> Step {
        Step::Emit(EventKind::ToolCall {
            call: ToolCallId(String::from(id)),
            phase: ToolPhase::Started { tool },
        })
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

    #[test]
    fn each_provider_event_maps_to_its_steps() {
        let cases = Vec::from([
            Case {
                got: claude_steps(ClaudeEvent::Message {
                    role: Role::Assistant,
                    content: Vec::from([
                        Block::Thinking(String::from("hmm")),
                        tool_use("Read", ToolDetail::Path(String::from("b.txt"))),
                        tool_use("Bash", ToolDetail::Command(String::from("ls"))),
                        tool_use("Write", ToolDetail::Path(String::from("c.txt"))),
                        tool_use("Edit", ToolDetail::Path(String::from("d.txt"))),
                    ]),
                }),
                want: Vec::from([
                    started(
                        "toolu_1",
                        ToolKind::Other {
                            name: String::from("Read"),
                            detail: String::from("b.txt"),
                        },
                    ),
                    started(
                        "toolu_1",
                        ToolKind::Command {
                            command: String::from("ls"),
                        },
                    ),
                    started(
                        "toolu_1",
                        ToolKind::FileChange {
                            files: Vec::from([proto::FileChange {
                                path: String::from("c.txt"),
                                change: Change::Write,
                            }]),
                        },
                    ),
                    started(
                        "toolu_1",
                        ToolKind::FileChange {
                            files: Vec::from([proto::FileChange {
                                path: String::from("d.txt"),
                                change: Change::Edit,
                            }]),
                        },
                    ),
                ]),
            },
            Case {
                got: claude_steps(ClaudeEvent::PermissionCancelled {
                    id: PermissionRequestId(String::from("p1")),
                }),
                want: Vec::from([Step::Withdraw(NativeRequest(String::from("p1")))]),
            },
            Case {
                got: claude_steps(finished(Outcome::Failed, Some("aborted_tools"))),
                want: Vec::from([Step::TurnEnded(TurnOutcome::Interrupted)]),
            },
            Case {
                got: claude_steps(finished(Outcome::Failed, None)),
                want: Vec::from([Step::TurnEnded(TurnOutcome::Failed {
                    message: String::from("error_during_execution"),
                })]),
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
                got: on_thread(ThreadEvent::Item {
                    turn: turn(),
                    phase: ItemPhase::Started,
                    item: Item::FileChange {
                        id: String::from("patch-1"),
                        changes: Vec::from([
                            FileChange {
                                path: String::from("a.txt"),
                                kind: ChangeKind::Add,
                            },
                            FileChange {
                                path: String::from("b.txt"),
                                kind: ChangeKind::Update,
                            },
                            FileChange {
                                path: String::from("c.txt"),
                                kind: ChangeKind::Delete,
                            },
                        ]),
                        status: String::from("inProgress"),
                    },
                }),
                want: Vec::from([started(
                    "patch-1",
                    ToolKind::FileChange {
                        files: Vec::from([
                            proto::FileChange {
                                path: String::from("a.txt"),
                                change: Change::Write,
                            },
                            proto::FileChange {
                                path: String::from("b.txt"),
                                change: Change::Edit,
                            },
                            proto::FileChange {
                                path: String::from("c.txt"),
                                change: Change::Delete,
                            },
                        ]),
                    },
                )]),
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
                }]),
            },
            Case {
                got: on_thread(ThreadEvent::ApprovalRequested {
                    id: ApprovalId(String::from("0")),
                    turn: turn(),
                    item: String::from("exec-1"),
                    kind: ApprovalKind::UserInput {
                        questions: serde_json::Value::Null,
                    },
                    options: Vec::new(),
                }),
                want: Vec::from([Step::Emit(EventKind::Error {
                    message: String::from(
                        "Codex asked a question. Questions arrive in a later slice; interrupt to continue.",
                    ),
                })]),
            },
            Case {
                got: on_thread(ThreadEvent::TurnCompleted {
                    turn: turn(),
                    outcome: codex::TurnOutcome::Failed { message: None },
                }),
                want: Vec::from([Step::TurnEnded(TurnOutcome::Failed {
                    message: String::from("the turn failed"),
                })]),
            },
        ]);
        let (got, want) = cases
            .into_iter()
            .map(|case| (case.got, case.want))
            .unzip::<Vec<Step>, Vec<Step>, Vec<Vec<Step>>, Vec<Vec<Step>>>();
        assert_eq!(got, want);
    }
}
