//! Claude Code driven over its stream-json control protocol (`--input-format
//! stream-json --permission-prompt-tool stdio`). Parsing and framing come from
//! the `claude-codes` crate; this module owns process lifecycle and maps the
//! protocol into Conductor's own types.

mod protocol;
mod session;

pub use protocol::{
    AfterDeny, Block, ClaudeEvent, Outcome, PermissionDecision, PermissionMode,
    PermissionRequestId, Role, SessionId,
};
pub use session::{ClaudeOptions, ClaudeSession, Error, SettingSource, VersionPolicy};
