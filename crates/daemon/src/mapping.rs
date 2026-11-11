//! Agent events to protocol events. Pure: a provider event in, the steps the
//! session applies out. Request ids and the turn belong to the session.

use agents::claude::{
    AfterDeny, Block, ClaudeEvent, Outcome, PermissionDecision, PermissionRequestId, Role,
    ToolDetail,
};
use proto::{
    ClientName, CloseReason, Decision, EventKind, ToolCallId, ToolKind, ToolOutcome, ToolPhase,
    TurnOutcome,
};

/// The provider's own id for an open request, which `agents` answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeRequest {
    Claude(PermissionRequestId),
}

/// What the provider needs to interrupt the running turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeTurn {
    Claude,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    Emit(EventKind),
    /// The session assigns the `RequestId`, stores it open and logs
    /// `RequestOpened`.
    Open {
        native: NativeRequest,
        tool: String,
        detail: String,
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
            .map(Step::Emit)
            .collect(),
        ClaudeEvent::StreamDelta { text } => {
            Vec::from([Step::Emit(EventKind::AssistantDelta { text })])
        }
        ClaudeEvent::PermissionRequested {
            id,
            tool_name,
            detail,
        } => Vec::from([Step::Open {
            native: NativeRequest::Claude(id),
            tool: tool_name,
            detail: detail_text(detail),
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

fn claude_block(role: Role, block: Block) -> Option<EventKind> {
    match (role, block) {
        (Role::Assistant, Block::Text(text)) => Some(EventKind::AssistantMessage { text }),
        (Role::Assistant, Block::ToolUse { id, name, detail }) => Some(EventKind::ToolCall {
            call: ToolCallId(id),
            phase: ToolPhase::Started {
                tool: tool_kind(name, detail),
            },
        }),
        (
            Role::User,
            Block::ToolResult {
                tool_use_id,
                outcome,
                ..
            },
        ) => Some(EventKind::ToolCall {
            call: ToolCallId(tool_use_id),
            phase: ToolPhase::Finished {
                outcome: match outcome {
                    Outcome::Succeeded => ToolOutcome::Succeeded,
                    Outcome::Failed => ToolOutcome::Failed,
                },
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

fn detail_text(detail: ToolDetail) -> String {
    match detail {
        ToolDetail::Command(text) | ToolDetail::Path(text) | ToolDetail::Text(text) => text,
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
