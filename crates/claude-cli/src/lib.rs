//! Claude Code driven over its stream-json control protocol (`--input-format
//! stream-json --permission-prompt-tool stdio`).
//!
//! `claude` reads and writes the frames we use; this crate owns the process
//! lifecycle.

mod session;

pub use session::{
    ClaudeOptions, ClaudeSession, Error, Resume, Sandbox, SettingSource,
    VersionPolicy, auth_status, cli, login, sandbox,
};
