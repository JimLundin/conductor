//! Claude events to the agent API's steps. Pure: a provider event in, the
//! steps the session applies out.

use agents::{
    NativeRequest, NativeTurn, Reply, Step, reasoning, reasoning_delta,
};
use proto::{
    ClientName, CloseReason, Decision, EventKind, SubagentId, ToolCallId,
    ToolPhase,
};

use crate::prelude::*;
use crate::{ASK_USER_QUESTION, Block, ClaudeEvent, PermissionDecision, Role};

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
            agent,
            id,
            tool,
            tool_name: _,
            tool_use_id,
            for_session,
        } => Vec::from([Step::Open {
            agent: agent.map(SubagentId),
            native: NativeRequest(id.0),
            call: ToolCallId(tool_use_id),
            tool,
            reason: None,
            for_session,
        }]),
        ClaudeEvent::QuestionAsked {
            agent,
            id,
            questions,
            ..
        } => Vec::from([Step::Ask {
            agent: agent.map(SubagentId),
            native: NativeRequest(id.0),
            questions,
        }]),
        ClaudeEvent::PermissionCancelled { id } => {
            Vec::from([Step::Withdraw(NativeRequest(id.0))])
        }
        ClaudeEvent::TurnFinished { outcome, .. } => {
            Vec::from([Step::TurnEnded(outcome)])
        }
        ClaudeEvent::SubagentStarted {
            agent,
            tool_use_id,
            description,
            kind,
        } => Vec::from([Step::SubagentStarted {
            agent: SubagentId(agent),
            call: ToolCallId(tool_use_id),
            description,
            kind,
        }]),
        ClaudeEvent::SubagentMessage {
            agent,
            role,
            content,
        } => content
            .into_iter()
            .filter_map(|block| claude_block(role, block))
            .filter_map(|step| step.for_subagent(SubagentId(agent.clone())))
            .collect(),
        ClaudeEvent::SubagentEnded {
            agent,
            outcome,
            summary,
        } => Vec::from([Step::SubagentEnded {
            agent: SubagentId(agent),
            outcome,
            summary,
        }]),
        ClaudeEvent::Exited { status } => {
            Vec::from([Step::Closed(CloseReason::Exited {
                status: status.describe(),
            })])
        }
        ClaudeEvent::Unknown(_)
        | ClaudeEvent::UnidentifiedSubagentMessage { .. } => Vec::new(),
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
        },
        Reply::Answer(answers) => PermissionDecision::Answer(answers),
    }
}

#[cfg(test)]
mod tests {
    use agents::{Exit, NativeRequest, Reply, Step};
    use proto::{
        Answer, ChecklistEntry, ChecklistStatus, ClientName, CloseReason,
        Decision, EventKind, Question, QuestionOption, Selection,
        SubagentActivity, SubagentId, SubagentOutcome, ToolCallId, ToolKind,
        ToolOutcome, ToolPhase,
    };

    use super::{claude_answer, claude_steps};
    use crate::{
        Block, ClaudeEvent, Inbound, PermissionDecision, PermissionRequestId,
        Role, parse_line,
    };

    /// `probe-subagent-todo.jsonl`: the main agent ran one sub-agent, which
    /// called `TodoWrite` twice and reported "ok".
    const PROBE: &str =
        include_str!("../../../fixtures/claude/probe-subagent-todo.jsonl");

    fn agent() -> SubagentId {
        SubagentId(String::from("af19920e77caaf1de"))
    }

    fn activity(activity: SubagentActivity) -> Step {
        Step::SubagentActivity {
            agent: agent(),
            activity,
        }
    }

    fn subagent_finished(call: &str) -> Step {
        activity(SubagentActivity::ToolCall {
            call: ToolCallId(String::from(call)),
            phase: ToolPhase::Finished {
                outcome: ToolOutcome::Succeeded,
            },
        })
    }

    fn checklist(status: ChecklistStatus) -> Vec<ChecklistEntry> {
        ["alpha", "beta"]
            .into_iter()
            .map(|text| ChecklistEntry {
                text: String::from(text),
                status,
            })
            .collect()
    }

    #[test]
    fn a_recorded_subagent_maps_to_its_start_its_calls_and_its_end()
    -> Result<(), serde_json::Error> {
        let steps = PROBE
            .lines()
            .map(parse_line)
            .collect::<Result<Vec<Inbound>, serde_json::Error>>()?
            .into_iter()
            .filter_map(|inbound| match inbound {
                Inbound::Event(event) => Some(event),
                Inbound::Permission { .. }
                | Inbound::ControlResponse { .. }
                | Inbound::UnsupportedControlRequest { .. } => None,
            })
            .flat_map(claude_steps)
            .filter(|step| {
                matches!(
                    step,
                    Step::SubagentStarted { .. }
                        | Step::SubagentActivity { .. }
                        | Step::SubagentEnded { .. }
                        | Step::SubagentChecklist { .. }
                        | Step::Checklist(_)
                )
            })
            .collect::<Vec<Step>>();
        assert_eq!(
            steps,
            Vec::from([
                Step::SubagentStarted {
                    kind: Some(String::from("general-purpose")),
                    agent: agent(),
                    call: ToolCallId(String::from(
                        "toolu_015av4j2jHs5CwVCv76qxYf2"
                    )),
                    description: String::from("Run TodoWrite checklist task"),
                },
                Step::SubagentChecklist {
                    agent: agent(),
                    steps: checklist(ChecklistStatus::Pending),
                },
                subagent_finished("toolu_01WSCsnAQWyaEonxDVnmncap"),
                Step::SubagentChecklist {
                    agent: agent(),
                    steps: checklist(ChecklistStatus::Completed),
                },
                subagent_finished("toolu_01H4nEBx6jVuR4PwHBWBgvPw"),
                Step::SubagentEnded {
                    agent: agent(),
                    outcome: SubagentOutcome::Finished,
                    summary: Some(String::from("ok")),
                },
            ])
        );
        Ok(())
    }

    #[test]
    fn a_subagents_messages_reasoning_and_calls_are_its_own() {
        let bash = ToolKind::Command {
            command: String::from("ls"),
        };
        assert_eq!(
            claude_steps(ClaudeEvent::SubagentMessage {
                agent: agent().0,
                role: Role::Assistant,
                content: Vec::from([
                    Block::Thinking(String::new()),
                    Block::Thinking(String::from("hm")),
                    Block::Text(String::from("looking")),
                    tool_use("Bash", bash.clone()),
                    tool_use(
                        "AskUserQuestion",
                        ToolKind::Other {
                            name: String::from("AskUserQuestion"),
                            detail: String::from("{}"),
                        },
                    ),
                    Block::Checklist(Vec::from([step()])),
                ]),
            }),
            Vec::from([
                activity(SubagentActivity::Reasoning {
                    text: String::from("hm"),
                }),
                activity(SubagentActivity::Message {
                    text: String::from("looking"),
                }),
                activity(SubagentActivity::ToolCall {
                    call: ToolCallId(String::from("toolu_1")),
                    phase: ToolPhase::Started { tool: bash },
                }),
                Step::SubagentChecklist {
                    agent: agent(),
                    steps: Vec::from([step()]),
                },
            ])
        );
    }

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

    fn step() -> ChecklistEntry {
        ChecklistEntry {
            text: String::from("write a.txt"),
            status: ChecklistStatus::InProgress,
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
                    role: Role::Assistant,
                    content: Vec::from([Block::Checklist(Vec::from([step()]))]),
                }),
                want: Vec::from([Step::Checklist(Vec::from([step()]))]),
            },
            Case {
                got: claude_steps(ClaudeEvent::QuestionAsked {
                    agent: None,
                    id: PermissionRequestId(String::from("p2")),
                    questions: Vec::from([color()]),
                    tool_use_id: String::from("toolu_2"),
                }),
                want: Vec::from([Step::Ask {
                    agent: None,
                    native: NativeRequest(String::from("p2")),
                    questions: Vec::from([color()]),
                }]),
            },
            Case {
                got: claude_steps(ClaudeEvent::PermissionRequested {
                    agent: None,
                    id: PermissionRequestId(String::from("p3")),
                    tool_name: String::from("Bash"),
                    tool: ToolKind::Command {
                        command: String::from("touch t1.txt"),
                    },
                    tool_use_id: String::from("toolu_3"),
                    for_session: Some(String::from("Bash(touch t1.txt)")),
                }),
                want: Vec::from([Step::Open {
                    agent: None,
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
