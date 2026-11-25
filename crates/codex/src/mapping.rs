//! Codex events to the agent API's steps. Pure: a provider event in, the
//! steps the session applies out.

use agents::{
    CloseReason, Decision as Chosen, Output, ToolCallId, ToolKind, ToolPhase,
};
use agents::{
    NativeRequest, NativeTurn, Note, Reply, Step, reasoning, reasoning_delta,
};

use crate::prelude::*;
use crate::{ApprovalKind, CodexEvent, Decision, PermissionAsk, ThreadEvent};

const SECRET_DECLINED: &str = "Codex asked for an answer it would keep secret. Conductor can't keep answers secret, so it declined the question.";

#[must_use]
pub fn codex_steps(event: CodexEvent) -> Vec<Step> {
    match event {
        CodexEvent::Thread {
            event: thread_event,
            ..
        } => codex_thread_steps(thread_event),
        CodexEvent::Exited { status } => {
            Vec::from([Step::Closed(CloseReason::Exited {
                status: status.describe(),
            })])
        }
        CodexEvent::Unknown(_) => Vec::new(),
    }
}

/// What an event that maps to no step is logged as.
#[must_use]
pub fn codex_note(event: &CodexEvent) -> Option<Note> {
    match event {
        CodexEvent::Unknown(_) => Some(Note::unmapped(
            crate::PROVIDER,
            String::from("unknown"),
            None,
        )),
        CodexEvent::Thread {
            thread,
            event: ThreadEvent::SettingsChanged(settings),
        } => Some(Note::ThreadSettings {
            provider: crate::PROVIDER,
            thread: thread.0.clone(),
            settings: format!("{settings:?}"),
        }),
        CodexEvent::Thread {
            thread,
            event: ThreadEvent::Unknown(_),
        } => Some(Note::unmapped(
            crate::PROVIDER,
            String::from("unknown"),
            Some(thread.0.clone()),
        )),
        CodexEvent::Thread { .. } | CodexEvent::Exited { .. } => None,
    }
}

pub fn codex_thread_steps(event: ThreadEvent) -> Vec<Step> {
    match event {
        ThreadEvent::TurnStarted { turn } => {
            Vec::from([Step::TurnStarted(NativeTurn(turn.0))])
        }
        ThreadEvent::AgentDelta { text, .. } => {
            Vec::from([Step::Emit(Output::AssistantDelta { text })])
        }
        ThreadEvent::AgentMessage { text, .. } => {
            Vec::from([Step::Emit(Output::AssistantMessage { text })])
        }
        ThreadEvent::ReasoningDelta { text, .. } => {
            reasoning_delta(text).into_iter().collect()
        }
        ThreadEvent::Reasoning { text, .. } => {
            reasoning(text).into_iter().collect()
        }
        ThreadEvent::ToolCall {
            call,
            phase: ToolPhase::Started { tool },
            ..
        } => Vec::from([Step::Emit(Output::ToolCall {
            call,
            phase: ToolPhase::Started { tool },
        })]),
        ThreadEvent::ToolCall {
            call,
            phase: ToolPhase::Finished { outcome },
            ..
        } => Vec::from([Step::ToolFinished { call, outcome }]),
        ThreadEvent::ApprovalRequested { id, item, kind, .. } => {
            Vec::from([approval_step(
                NativeRequest(id.0),
                ToolCallId(item),
                kind,
            )])
        }
        ThreadEvent::ApprovalCancelled { id } => {
            Vec::from([Step::Withdraw(NativeRequest(id.0))])
        }
        ThreadEvent::SecretQuestionDeclined { .. } => {
            Vec::from([Step::Emit(Output::Error {
                message: String::from(SECRET_DECLINED),
            })])
        }
        ThreadEvent::TurnCompleted { outcome, .. } => {
            Vec::from([Step::TurnEnded(outcome)])
        }
        ThreadEvent::PlanUpdated { steps, .. } => {
            Vec::from([Step::Checklist(steps)])
        }
        ThreadEvent::Started
        | ThreadEvent::Child(_)
        | ThreadEvent::SettingsChanged(_)
        | ThreadEvent::Unknown(_) => Vec::new(),
    }
}

fn approval_step(
    native: NativeRequest,
    call: ToolCallId,
    kind: ApprovalKind,
) -> Step {
    match kind {
        ApprovalKind::Command {
            command: asked,
            reason,
            ..
        } => {
            let command = asked.unwrap_or_default();
            Step::Open {
                agent: None,
                native,
                call,
                tool: ToolKind::Command {
                    command: command.clone(),
                },
                reason,
                for_session: Some(command),
            }
        }
        ApprovalKind::FileChange { reason } => Step::Open {
            agent: None,
            native,
            call,
            tool: ToolKind::FileChange { files: Vec::new() },
            reason,
            for_session: None,
        },
        ApprovalKind::Permissions { reason, asks } => {
            let detail = asks
                .into_iter()
                .map(|ask| match ask {
                    PermissionAsk::Network => String::from("network access"),
                    PermissionAsk::Read(places) => {
                        format!("read {}", places.join(", "))
                    }
                    PermissionAsk::Write(places) => {
                        format!("write to {}", places.join(", "))
                    }
                })
                .collect::<Vec<String>>()
                .join("; ");
            Step::Open {
                agent: None,
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
        ApprovalKind::UserInput { questions } => Step::Ask {
            agent: None,
            native,
            questions,
        },
    }
}

#[must_use]
pub fn codex_answer(reply: Reply) -> Decision {
    match reply {
        Reply::Decide(Chosen::Approve) => Decision::Accept,
        Reply::Decide(Chosen::ApproveForSession) => Decision::AcceptForSession,
        Reply::Decide(Chosen::Deny) => Decision::Decline,
        Reply::Answer(answers) => Decision::Answer(answers),
    }
}

#[cfg(test)]
mod tests {
    use agents::{
        Answer, ChecklistEntry, ChecklistStatus, Decision as Chosen, Output,
        Question, QuestionOption, Selection, ToolCallId, ToolKind,
    };
    use agents::{NativeRequest, Reply, Step};

    use super::{codex_answer, codex_steps};
    use crate::{
        ApprovalId, ApprovalKind, CodexEvent, Decision, PermissionAsk,
        ThreadEvent, ThreadId, TurnId,
    };

    struct Case {
        got: Vec<Step>,
        want: Vec<Step>,
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
    fn each_reply_maps_to_codexs_answer() {
        let answers = Vec::from([Answer(Vec::from([String::from("Blue")]))]);
        let replies = Vec::from([
            Reply::Decide(Chosen::Approve),
            Reply::Decide(Chosen::ApproveForSession),
            Reply::Decide(Chosen::Deny),
            Reply::Answer(answers.clone()),
        ]);
        assert_eq!(
            replies
                .into_iter()
                .map(codex_answer)
                .collect::<Vec<Decision>>(),
            Vec::from([
                Decision::Accept,
                Decision::AcceptForSession,
                Decision::Decline,
                Decision::Answer(answers),
            ])
        );
    }

    #[test]
    fn reasoning_and_plans_map_to_their_steps() {
        assert_cases([
            Case {
                got: on_thread(ThreadEvent::ReasoningDelta {
                    turn: turn(),
                    text: String::from("**Plan**"),
                }),
                want: Vec::from([Step::Emit(Output::ReasoningDelta {
                    text: String::from("**Plan**"),
                })]),
            },
            Case {
                got: on_thread(ThreadEvent::Reasoning {
                    turn: turn(),
                    text: String::from("**Plan**\n\nList it."),
                }),
                want: Vec::from([Step::Emit(Output::Reasoning {
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
                got: on_thread(ThreadEvent::PlanUpdated {
                    turn: turn(),
                    steps: Vec::from([step()]),
                }),
                want: Vec::from([Step::Checklist(Vec::from([step()]))]),
            },
        ]);
    }

    #[test]
    fn codex_requests_map_to_their_steps() {
        assert_cases([
            Case {
                got: on_thread(ThreadEvent::ApprovalRequested {
                    id: ApprovalId(String::from("integer:1")),
                    turn: turn(),
                    item: String::from("exec-2"),
                    kind: ApprovalKind::Command {
                        command: Some(String::from("touch ~/b.txt")),
                        cwd: None,
                        reason: Some(String::from(
                            "needs to write outside the workspace",
                        )),
                    },
                }),
                want: Vec::from([Step::Open {
                    agent: None,
                    native: NativeRequest(String::from("integer:1")),
                    call: ToolCallId(String::from("exec-2")),
                    tool: ToolKind::Command {
                        command: String::from("touch ~/b.txt"),
                    },
                    reason: Some(String::from(
                        "needs to write outside the workspace",
                    )),
                    for_session: Some(String::from("touch ~/b.txt")),
                }]),
            },
            Case {
                got: on_thread(ThreadEvent::ApprovalRequested {
                    id: ApprovalId(String::from("integer:2")),
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
                }),
                want: Vec::from([Step::Open {
                    agent: None,
                    native: NativeRequest(String::from("integer:2")),
                    call: ToolCallId(String::from("exec-3")),
                    tool: ToolKind::Other {
                        name: String::from("permissions"),
                        detail: String::from(
                            "network access; write to /home/u/.cargo, /tmp",
                        ),
                    },
                    reason: Some(String::from("to fetch the crate")),
                    for_session: Some(String::from(
                        "network access; write to /home/u/.cargo, /tmp",
                    )),
                }]),
            },
            Case {
                got: on_thread(ThreadEvent::ApprovalRequested {
                    id: ApprovalId(String::from("integer:0")),
                    turn: turn(),
                    item: String::from("exec-1"),
                    kind: ApprovalKind::UserInput {
                        questions: Vec::from([color()]),
                    },
                }),
                want: Vec::from([Step::Ask {
                    agent: None,
                    native: NativeRequest(String::from("integer:0")),
                    questions: Vec::from([color()]),
                }]),
            },
            Case {
                got: on_thread(ThreadEvent::SecretQuestionDeclined {
                    turn: turn(),
                }),
                want: Vec::from([Step::Emit(Output::Error {
                    message: String::from(
                        "Codex asked for an answer it would keep secret. Conductor can't keep answers secret, so it declined the question.",
                    ),
                })]),
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
