//! Claude Code's stream-json control protocol (`--input-format stream-json
//! --permission-prompt-tool stdio`): the few frames we read and write, and
//! their events mapped into the agent API (`agents`). `claude-cli` runs the
//! process that speaks it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod mapping;
mod prelude;
mod protocol;

pub use mapping::{claude_answer, claude_steps};
pub use protocol::{
    ASK_USER_QUESTION, AfterDeny, Applied, Block, ClaudeEvent, ControlCall,
    Inbound, MessageId, Open, Outbound, PermissionDecision, PermissionMode,
    PermissionRequestId, Role, SessionId, UnsupportedVersion, applied,
    check_version, encode, models, parse_line,
};
