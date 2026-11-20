//! What a session does with an agent's event, and how it answers one.
//! Request ids and the turn belong to the session.

use proto::{
    Answer, CloseReason, Decision, EventKind, Question, SubagentActivity,
    SubagentId, SubagentOutcome, ToolCallId, ToolKind, ToolOutcome,
    TurnOutcome,
};

use crate::PlanStep;
use crate::prelude::*;

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
    /// The agent rewrote its checklist: the session syncs its tasks to it.
    Checklist(Vec<PlanStep>),
    /// The agent's tool call `call` started a sub-agent, which runs inside
    /// the session: the session logs it.
    SubagentStarted {
        agent: SubagentId,
        call: ToolCallId,
        description: String,
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
        .map(|piece| Step::Emit(EventKind::Reasoning { text: piece }))
}

/// A piece of reasoning as it streams. Claude streams one empty piece for
/// a block with no text, which would show reasoning that never comes.
#[must_use]
pub fn reasoning_delta(text: String) -> Option<Step> {
    Some(text)
        .filter(|piece| !piece.is_empty())
        .map(|piece| Step::Emit(EventKind::ReasoningDelta { text: piece }))
}

#[cfg(test)]
mod tests {
    use proto::EventKind;

    use super::{Step, reasoning, reasoning_delta};

    #[test]
    fn reasoning_shows_only_when_it_has_text() {
        let text = || String::from("thinking");
        assert_eq!(
            [reasoning(text()), reasoning(String::new())],
            [
                Some(Step::Emit(EventKind::Reasoning { text: text() })),
                None
            ]
        );
        assert_eq!(
            [reasoning_delta(text()), reasoning_delta(String::new())],
            [
                Some(Step::Emit(EventKind::ReasoningDelta { text: text() })),
                None
            ]
        );
    }
}
