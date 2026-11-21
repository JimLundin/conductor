//! What an agent's core decides is worth logging, as data: the shell logs
//! each kind with its fields.

use proto::Provider;

use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    /// The CLI's stdout failed to read, which ends the agent.
    Unreadable {
        provider: Provider,
        error: String,
    },
    NotJson {
        provider: Provider,
        error: String,
    },
    /// A frame that maps to no step.
    Unmapped {
        provider: Provider,
        frame: String,
    },
    UnmappedOnThread {
        provider: Provider,
        thread: String,
        frame: String,
    },
    ThreadSettings {
        provider: Provider,
        thread: String,
        settings: String,
    },
    /// A response to a request nobody waits on.
    UnknownResponse {
        provider: Provider,
        request: String,
    },
    /// The CLI asked for something Conductor does not serve, and was
    /// refused.
    Unsupported {
        provider: Provider,
        request: String,
    },
    /// A line nobody waits on failed to write.
    Unwritten {
        provider: Provider,
        writing: String,
        error: String,
    },
    /// The CLI asked a question whose answer it would keep secret, and was
    /// declined.
    SecretDeclined {
        provider: Provider,
    },
    Answering {
        provider: Provider,
        approval: String,
        thread: String,
    },
    /// The CLI's version is outside the tested range, and it runs anyway.
    Untested {
        provider: Provider,
        found: String,
        supported: String,
    },
    /// The CLI reported no version, and it runs anyway.
    Unversioned {
        provider: Provider,
    },
    /// The CLI's commands run unsandboxed with a gateway credential, so
    /// they can read its token.
    CredentialExposed {
        provider: Provider,
    },
}

impl Note {
    #[must_use]
    pub fn unmapped(
        provider: Provider,
        frame: String,
        on_thread: Option<String>,
    ) -> Self {
        match on_thread {
            Some(thread) => Self::UnmappedOnThread {
                provider,
                thread,
                frame,
            },
            None => Self::Unmapped { provider, frame },
        }
    }
}
