//! The protocol a session speaks with its agent, whichever provider runs
//! it.
//!
//! The session sends [`Command`]s and waits for each one's [`Outcome`];
//! meanwhile the agent reports [`Step`]s. Each provider translates the
//! commands into its own wire and its wire into these events (`claude`,
//! `codex`).

mod lifecycle;

pub use lifecycle::{Lifecycle, NOT_RUNNING, Report};

use proto::{ClientName, Effort, HistoryAnchor, HostPath, ModelId, SessionId};

use crate::prelude::*;
use crate::{NativeRequest, NativeTurn, Reply, Step};

/// How to start an agent for `session`. The provider resumes `history` if
/// it can, else `conversation`, else starts a new conversation; each
/// provider decides how to fall back when the one it resumes is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    pub session: SessionId,
    pub directory: HostPath,
    pub model: Option<ModelId>,
    pub effort: Option<Effort>,
    /// The point in the agent's history the session restores, if it knows
    /// one.
    pub history: Option<HistoryAnchor>,
    /// The conversation the session saved once a turn started in it.
    pub conversation: Option<String>,
    /// What the session is told beside its CLI's own instructions.
    pub instructions: Option<String>,
}

/// What the session asks of its agent. Each command has one [`Outcome`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Starts an agent in place of the one running, if it starts.
    Start(Start),
    /// Starts a turn, at `effort` if it differs from the agent's.
    Send {
        text: String,
        effort: Option<Effort>,
    },
    Interrupt(NativeTurn),
    Reply {
        native: NativeRequest,
        reply: Reply,
        by: ClientName,
    },
    /// The agent's turns, oldest first, by the provider's ids.
    Turns,
    /// Discards this turn and every later one from the agent's history.
    Revert(String),
    /// Stops the agent.
    Shutdown,
}

/// How an agent's history is anchored, and so how an undo restores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchoring {
    /// At the last message a turn ended on: an undo forks the conversation
    /// through the restored checkpoint's message
    /// ([`HistoryAnchor::Messages`]).
    Messages,
    /// At each turn: an undo reverts the turns after the restored
    /// checkpoint's ([`HistoryAnchor::Turns`]).
    Turns,
}

/// An agent that started: its conversation's id, and how its history is
/// anchored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub conversation: String,
    pub anchoring: Anchoring,
}

/// What a command came to. Each answers the command of its name, so the
/// session matches it with the command it waits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Started(Result<Started, String>),
    Sent(Result<(), String>),
    Interrupted(Result<(), String>),
    Replied(Result<(), String>),
    Turns(Result<Vec<String>, String>),
    Reverted(Result<(), String>),
    RevertUnknown,
    ShutDown(Result<(), String>),
}

/// What an agent reports: a step of its work, or the outcome of the
/// command the session waits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Step(Step),
    Outcome(Outcome),
}

impl Command {
    /// The outcome of this command when it could not be performed, for
    /// `reason`.
    #[must_use]
    pub fn failed(&self, reason: &str) -> Outcome {
        let error = String::from(reason);
        match self {
            Self::Start(_) => Outcome::Started(Err(error)),
            Self::Send { .. } => Outcome::Sent(Err(error)),
            Self::Interrupt(_) => Outcome::Interrupted(Err(error)),
            Self::Reply { .. } => Outcome::Replied(Err(error)),
            Self::Turns => Outcome::Turns(Err(error)),
            Self::Revert(_) => Outcome::Reverted(Err(error)),
            Self::Shutdown => Outcome::ShutDown(Err(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, Outcome};
    use crate::NativeTurn;

    #[test]
    fn a_command_that_could_not_run_fails_with_its_own_outcome() {
        let reason = "the agent is not running";
        let failed = |command: Command| command.failed(reason);
        let error = || String::from(reason);
        assert_eq!(
            [
                failed(Command::Turns),
                failed(Command::Interrupt(NativeTurn(String::from("t")))),
                failed(Command::Shutdown),
            ],
            [
                Outcome::Turns(Err(error())),
                Outcome::Interrupted(Err(error())),
                Outcome::ShutDown(Err(error())),
            ]
        );
    }
}
