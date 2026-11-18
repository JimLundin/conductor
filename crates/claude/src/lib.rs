//! Claude Code's stream-json control protocol (`--input-format stream-json
//! --permission-prompt-tool stdio`): the few frames we read and write, and
//! their events mapped into the agent API (`agents`), how its process starts,
//! what the process actor decides and what the agent decides with a
//! session's runtime commands. `claude-cli` runs the process that speaks
//! it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod credentials;
mod driver;
mod launch;
mod mapping;
mod prelude;
mod protocol;
pub mod runtime;

pub use credentials::{ClaudeCredential, credential, info};
pub use driver::{After, Command, Effect, Failure, Input, Session, Then};
pub use launch::{
    Launch, Resume, SANDBOX_TOOLS, Sandbox, SettingSource, VersionPolicy,
    admit, auth_status, cli, login, profile, reported_refusal, sandbox,
    session, unprotected,
};
pub use mapping::{claude_answer, claude_steps};
pub use protocol::{
    ASK_USER_QUESTION, AfterDeny, Applied, Block, ClaudeEvent, ControlCall,
    Inbound, MessageId, Open, Outbound, PermissionDecision, PermissionMode,
    PermissionRequestId, Role, SessionId, UnsupportedVersion, applied,
    check_version, encode, models, parse_line,
};
