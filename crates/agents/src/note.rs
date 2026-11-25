use crate::Provider;
use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    Unreadable {
        provider: Provider,
        error: String,
    },
    NotJson {
        provider: Provider,
        error: String,
    },
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
    UnknownResponse {
        provider: Provider,
        request: String,
    },
    Unsupported {
        provider: Provider,
        request: String,
    },
    Unwritten {
        provider: Provider,
        writing: String,
        error: String,
    },
    SecretDeclined {
        provider: Provider,
    },
    Answering {
        provider: Provider,
        approval: String,
        thread: String,
    },
    Untested {
        provider: Provider,
        found: String,
        supported: String,
    },
    Unversioned {
        provider: Provider,
    },
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
