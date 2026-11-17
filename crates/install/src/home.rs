//! Where a CLI keeps its config, credentials and transcripts.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Home {
    /// The user's own (`~/.claude`, `~/.codex`), for the live checks.
    User,
    /// A directory of the host's own, passed as `CLAUDE_CONFIG_DIR` or
    /// `CODEX_HOME`, so the user's homes stay untouched.
    Private(PathBuf),
}
