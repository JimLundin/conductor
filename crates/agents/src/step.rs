//! What a session does with an agent's event, and how it answers one.
//! Request ids and the turn belong to the session.

use crate::prelude::*;
use crate::{
    Answer, ChecklistEntry, CloseReason, Decision, Output, Question,
    SubagentActivity, SubagentId, SubagentOutcome, ToolCallId, ToolKind,
    ToolOutcome, ToolPhase, TurnOutcome,
};

/// An open request's id as the provider gave it. The type doesn't say which
/// provider: a session's steps come only from its own agent's events, so its
/// agent reads the id back as its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRequest(pub String);

/// The running turn as the provider interrupts it, by the provider's id:
/// Claude's session, Codex's turn. Read like [`NativeRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeTurn(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// The session logs what the agent produced.
    Emit(Output),
    /// The session assigns the `RequestId`, stores it open and logs
    /// `RequestOpened`. `call` names the tool call the request asks about.
    /// `for_session` describes what approving for the session allows, if
    /// the agent offers it.
    Open {
        agent: Option<SubagentId>,
        native: NativeRequest,
        call: ToolCallId,
        tool: ToolKind,
        reason: Option<String>,
        for_session: Option<String>,
    },
    /// The session assigns the `RequestId`, stores the question open and
    /// logs `QuestionAsked`.
    Ask {
        agent: Option<SubagentId>,
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
    /// The session runs the turn, and sends an interrupt it held.
    TurnStarted(NativeTurn),
    /// The agent process is gone: the session logs this and waits for a
    /// `Continue`.
    Closed(CloseReason),
    /// The whole checklist, replacing the previous snapshot.
    Checklist(Vec<ChecklistEntry>),
    /// The agent's tool call `call` started a sub-agent, which runs inside
    /// the session: the session logs it.
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
    /// The session logs what the sub-agent did, a tool call of its that
    /// failed after the user denied it as `Declined`.
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

/// A reply to an open request: a decision for an approval, answers for a
/// question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Decide(Decision),
    Answer(Vec<Answer>),
}

/// A finished stretch of reasoning. Without its text (Claude's signature
/// alone, a Codex item with no summary) there is nothing to show.
#[must_use]
pub fn reasoning(text: String) -> Option<Step> {
    Some(text)
        .filter(|piece| !piece.is_empty())
        .map(|piece| Step::Emit(Output::Reasoning { text: piece }))
}

/// A piece of reasoning as it streams. Claude streams one empty piece for
/// a block with no text, which would show reasoning that never comes.
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
