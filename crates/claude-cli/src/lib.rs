//! Claude Code driven over its stream-json control protocol (`--input-format
//! stream-json --permission-prompt-tool stdio`).
//!
//! `claude` reads and writes the frames we use and decides what the session
//! and the agent do with them; this crate owns the process and performs
//! those decisions, runs each agent behind a `runtime::Runtime`, and hands
//! the host Claude's [`plugin`].

mod agent;
mod plugin;
mod session;

pub use claude::Sandbox;
pub use plugin::plugin;
pub use session::{ClaudeOptions, ClaudeSession, Error, sandbox};
