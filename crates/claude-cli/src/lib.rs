//! Claude Code driven over its stream-json control protocol (`--input-format
//! stream-json --permission-prompt-tool stdio`).
//!
//! `claude` reads and writes the frames we use and decides what the session
//! and the agent do with them; this crate owns the process and performs
//! those decisions, and runs each agent behind a `runtime::Runtime`.

mod agent;
mod session;

pub use agent::{ClaudeLaunch, starter};
pub use session::{
    ClaudeOptions, ClaudeSession, Error, auth_status, cli, login, sandbox,
};
