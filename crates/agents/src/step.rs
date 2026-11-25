use crate::prelude::*;
use crate::{
    Answer, ChecklistEntry, CloseReason, Decision, Output, Question,
    SubagentActivity, SubagentId, SubagentOutcome, ToolCallId, ToolKind,
    ToolOutcome, ToolPhase, TurnOutcome,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRequest(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeTurn(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Emit(Output),
    Open {
        agent: Option<SubagentId>,
        native: NativeRequest,
        call: ToolCallId,
        tool: ToolKind,
        reason: Option<String>,
        for_session: Option<String>,
    },
    Ask {
        agent: Option<SubagentId>,
        native: NativeRequest,
        questions: Vec<Question>,
    },
    ToolFinished {
        call: ToolCallId,
        outcome: ToolOutcome,
    },
    Withdraw(NativeRequest),
    TurnEnded(TurnOutcome),
    TurnStarted(NativeTurn),
    Closed(CloseReason),
    Checklist(Vec<ChecklistEntry>),
    SubagentStarted {
        agent: SubagentId,
        call: ToolCallId,
        description: String,
        kind: Option<String>,
    },
    SubagentKind {
        agent: SubagentId,
        kind: String,
    },
    SubagentChecklist {
        agent: SubagentId,
        steps: Vec<ChecklistEntry>,
    },
    SubagentActivity {
        agent: SubagentId,
        activity: SubagentActivity,
    },
    SubagentEnded {
        agent: SubagentId,
        outcome: SubagentOutcome,
        summary: Option<String>,
    },
}

impl Step {
    #[must_use]
    pub fn for_subagent(self, agent: SubagentId) -> Option<Self> {
        let activity = match self {
            Self::Emit(Output::AssistantMessage { text }) => {
                SubagentActivity::Message { text }
            }
            Self::Emit(Output::Reasoning { text }) => {
                SubagentActivity::Reasoning { text }
            }
            Self::Emit(Output::ToolCall { call, phase }) => {
                SubagentActivity::ToolCall { call, phase }
            }
            Self::ToolFinished { call, outcome } => {
                SubagentActivity::ToolCall {
                    call,
                    phase: ToolPhase::Finished { outcome },
                }
            }
            Self::Checklist(steps) => {
                return Some(Self::SubagentChecklist { agent, steps });
            }
            Self::Open {
                native,
                call,
                tool,
                reason,
                for_session,
                ..
            } => {
                return Some(Self::Open {
                    agent: Some(agent),
                    native,
                    call,
                    tool,
                    reason,
                    for_session,
                });
            }
            Self::Ask {
                native, questions, ..
            } => {
                return Some(Self::Ask {
                    agent: Some(agent),
                    native,
                    questions,
                });
            }
            Self::Withdraw(_) => return Some(self),
            _ => return None,
        };
        Some(Self::SubagentActivity { agent, activity })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Decide(Decision),
    Answer(Vec<Answer>),
}

#[must_use]
pub fn reasoning(text: String) -> Option<Step> {
    Some(text)
        .filter(|piece| !piece.is_empty())
        .map(|piece| Step::Emit(Output::Reasoning { text: piece }))
}

#[must_use]
pub fn reasoning_delta(text: String) -> Option<Step> {
    Some(text)
        .filter(|piece| !piece.is_empty())
        .map(|piece| Step::Emit(Output::ReasoningDelta { text: piece }))
}

#[cfg(test)]
mod tests {
    use super::{Step, reasoning, reasoning_delta};
    use crate::Output;

    #[test]
    fn reasoning_shows_only_when_it_has_text() {
        let text = || String::from("thinking");
        assert_eq!(
            [reasoning(text()), reasoning(String::new())],
            [Some(Step::Emit(Output::Reasoning { text: text() })), None]
        );
        assert_eq!(
            [reasoning_delta(text()), reasoning_delta(String::new())],
            [
                Some(Step::Emit(Output::ReasoningDelta { text: text() })),
                None
            ]
        );
    }
}
