//! The protocol a session speaks with its agent, whichever provider runs
//! it.
//!
//! The session sends [`Command`]s and waits for each one's [`Outcome`];
//! meanwhile the agent reports [`Step`]s. Each provider translates the
//! commands into its own wire and its wire into these events (`claude`,
//! `codex`).

mod lifecycle;

pub use lifecycle::{Lifecycle, NOT_RUNNING, Report};

use crate::prelude::*;
use crate::{
    ClientName, Effort, ModelId, NativeRequest, NativeTurn, Reply, Step,
};

/// The session an agent runs for, by the id its host gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session(pub u128);

/// The directory an agent works in, as the host's OS spells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directory(pub String);

/// Providers decide how to fall back when the saved conversation is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    pub session: Session,
    pub directory: Directory,
    pub model: Option<ModelId>,
    pub effort: Option<Effort>,
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
    /// Stops the agent.
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub conversation: String,
}

/// What a command came to. Each answers the command of its name, so the
/// session matches it with the command it waits on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Started(Result<Started, String>),
    Sent(Result<(), String>),
    Interrupted(Result<(), String>),
    Replied(Result<(), String>),
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
                failed(Command::Interrupt(NativeTurn(String::from("t")))),
                failed(Command::Shutdown),
            ],
            [
                Outcome::Interrupted(Err(error())),
                Outcome::ShutDown(Err(error())),
            ]
        );
    }
}
