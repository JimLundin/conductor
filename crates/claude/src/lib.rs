//! Claude Code's stream-json control protocol (`--input-format stream-json
//! --permission-prompt-tool stdio`): the few frames we read and write, and
//! their events mapped into the agent API (`agents`), how its process starts
//! and what the session actor decides. `claude-cli` runs the process that
//! speaks it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod driver;
mod launch;
mod mapping;
mod prelude;
mod protocol;

pub use driver::{
    After, Command, Effect, Failure, Input, Session, Then, log_stderr,
    log_unrecorded,
};
pub use launch::{
    Launch, Resume, SANDBOX_TOOLS, Sandbox, SettingSource, VersionPolicy,
    admit, auth_status, cli, login, reported_refusal, sandbox, session,
};
pub use mapping::{claude_answer, claude_steps};
pub use protocol::{
    ASK_USER_QUESTION, AfterDeny, Applied, Block, ClaudeEvent, ControlCall,
    Inbound, MessageId, Open, Outbound, PermissionDecision, PermissionMode,
    PermissionRequestId, Role, SessionId, UnsupportedVersion, applied,
    check_version, encode, models, parse_line,
};
