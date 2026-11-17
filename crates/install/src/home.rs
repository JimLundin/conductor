//! Where a CLI keeps its config, credentials and transcripts.

use std::path::PathBuf;

use agents::HomeKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Home {
    /// The user's own (`~/.claude`, `~/.codex`).
    User,
    /// A directory of the host's own, passed as `CLAUDE_CONFIG_DIR` or
    /// `CODEX_HOME`, so the user's homes stay untouched.
    Private(PathBuf),
}

impl Home {
    #[must_use]
    pub const fn kind(&self) -> HomeKind {
        match self {
            Self::User => HomeKind::User,
            Self::Private(_) => HomeKind::Private,
        }
    }
}
