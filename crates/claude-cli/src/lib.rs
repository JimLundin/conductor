//! Claude Code driven over its stream-json control protocol (`--input-format
//! stream-json --permission-prompt-tool stdio`).
//!
//! `claude` reads and writes the frames we use and decides what the session
//! does with them; this crate owns the process and performs those decisions.

mod session;

pub use session::{
    ClaudeOptions, ClaudeSession, Error, auth_status, cli, login, sandbox,
};
