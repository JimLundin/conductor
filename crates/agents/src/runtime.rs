mod lifecycle;

pub use lifecycle::{Lifecycle, NOT_RUNNING, Report};

use crate::prelude::*;
use crate::{
    ClientName, Effort, ModelId, NativeRequest, NativeTurn, Reply, Step,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session(pub u128);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directory(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    pub session: Session,
    pub directory: Directory,
    pub model: Option<ModelId>,
    pub effort: Option<Effort>,
    pub conversation: Option<String>,
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Start(Start),
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
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub conversation: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Started(Result<Started, String>),
    Sent(Result<(), String>),
    Interrupted(Result<(), String>),
    Replied(Result<(), String>),
    ShutDown(Result<(), String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Step(Step),
    Outcome(Outcome),
}

impl Command {
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
