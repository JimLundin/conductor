//! Claude events to the agent API's steps. Pure: a provider event in, the
//! steps the session applies out.

use agents::{
    NativeRequest, NativeTurn, Reply, Step, reasoning, reasoning_delta,
};
use proto::{
    ClientName, CloseReason, Decision, EventKind, ToolCallId, ToolPhase,
};

use crate::prelude::*;
use crate::{
    ASK_USER_QUESTION, AfterDeny, Block, ClaudeEvent, PermissionDecision, Role,
};

#[must_use]
pub fn claude_steps(event: ClaudeEvent) -> Vec<Step> {
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
        ClaudeEvent::ThinkingDelta { text } => {
            reasoning_delta(text).into_iter().collect()
        }
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
        ClaudeEvent::QuestionAsked { id, questions, .. } => {
            Vec::from([Step::Ask {
                native: NativeRequest(id.0),
                questions,
            }])
        }
        ClaudeEvent::PermissionCancelled { id } => {
            Vec::from([Step::Withdraw(NativeRequest(id.0))])
        }
        ClaudeEvent::TurnFinished {
            outcome,
            last_message,
            ..
        } => Vec::from([
            Step::HistoryBoundary(last_message.map(|message| message.0)),
            Step::TurnEnded(outcome),
        ]),
        ClaudeEvent::Exited { status } => {
            Vec::from([Step::Closed(CloseReason::Exited {
                status: status.describe(),
            })])
        }
        ClaudeEvent::Unknown(_) => Vec::new(),
    }
}

/// A question's `AskUserQuestion` call is not shown as a tool call: its
/// `QuestionAsked` shows it. Nor is a `TodoWrite` call: the session's tasks
/// show it. Their results then finish a call no client saw start, which a
/// client ignores.
fn claude_block(role: Role, block: Block) -> Option<Step> {
    match (role, block) {
        (Role::Assistant, Block::Text(text)) => {
            Some(Step::Emit(EventKind::AssistantMessage { text }))
        }
        (Role::Assistant, Block::Thinking(text)) => reasoning(text),
        (Role::Assistant, Block::Checklist(steps)) => {
            Some(Step::Checklist(steps))
        }
        (Role::Assistant, Block::ToolUse { id, name, tool }) => {
            match name.as_str() {
                ASK_USER_QUESTION => None,
                _ => Some(Step::Emit(EventKind::ToolCall {
                    call: ToolCallId(id),
                    phase: ToolPhase::Started { tool },
                })),
            }
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
            outcome,
        }),
        (
            Role::User,
            Block::Text(_)
            | Block::ToolUse { .. }
            | Block::Thinking(_)
            | Block::Checklist(_),
        )
        | (Role::Assistant, Block::ToolResult { .. })
        | (_, Block::Other(_)) => None,
    }
}

#[must_use]
pub fn claude_answer(reply: Reply, by: &ClientName) -> PermissionDecision {
    match reply {
        Reply::Decide(Decision::Approve) => PermissionDecision::Allow,
        Reply::Decide(Decision::ApproveForSession) => {
            PermissionDecision::AllowForSession
        }
        Reply::Decide(Decision::Deny) => PermissionDecision::Deny {
            message: format!("Denied by {}", by.0),
            then: AfterDeny::Continue,
        },
        Reply::Answer(answers) => PermissionDecision::Answer(answers),
    }
}

#[cfg(test)]
mod tests {
    use agents::{Exit, NativeRequest, PlanStep, Reply, Step};
    use proto::{
        Answer, ClientName, CloseReason, Decision, EventKind, Question,
        QuestionOption, Selection, TaskStatus, ToolCallId, ToolKind,
    };

    use super::{claude_answer, claude_steps};
    use crate::{
        AfterDeny, Block, ClaudeEvent, PermissionDecision, PermissionRequestId,
        Role,
    };

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

    fn step() -> PlanStep {
        PlanStep {
            text: String::from("write a.txt"),
            status: TaskStatus::InProgress,
        }
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
    fn each_reply_maps_to_claudes_answer() {
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
                .into_iter()
                .map(|reply| claude_answer(reply, &by))
                .collect::<Vec<PermissionDecision>>(),
            Vec::from([
                PermissionDecision::Allow,
                PermissionDecision::AllowForSession,
                PermissionDecision::Deny {
                    message: String::from("Denied by phone"),
                    then: AfterDeny::Continue,
                },
                PermissionDecision::Answer(answers),
            ])
        );
    }

    #[test]
    fn reasoning_maps_to_its_steps() {
        assert_cases([
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
        ]);
    }

    #[test]
    fn claude_events_and_plans_map_to_their_steps() {
        assert_cases([
            Case {
                got: claude_steps(ClaudeEvent::PermissionCancelled {
                    id: PermissionRequestId(String::from("p1")),
                }),
                want: Vec::from([Step::Withdraw(NativeRequest(String::from(
                    "p1",
                )))]),
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
                got: claude_steps(ClaudeEvent::Message {
                    uuid: None,
                    role: Role::Assistant,
                    content: Vec::from([Block::Checklist(Vec::from([step()]))]),
                }),
                want: Vec::from([Step::Checklist(Vec::from([step()]))]),
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
        ]);
    }

    fn assert_cases<const N: usize>(cases: [Case; N]) {
        let (got, want) = cases
            .into_iter()
            .map(|case| (case.got, case.want))
            .unzip::<Vec<Step>, Vec<Step>, Vec<Vec<Step>>, Vec<Vec<Step>>>(
        );
        assert_eq!(got, want);
    }
}
