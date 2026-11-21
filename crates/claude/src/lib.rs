//! Claude Code's stream-json control protocol (`--input-format stream-json
//! --permission-prompt-tool stdio`): the few frames we read and write, and
//! their events mapped into the agent API (`agents`), how its process starts,
//! what the process actor decides, what the agent decides with a
//! session's runtime commands, and how its sign-in reads. `claude-cli`
//! runs the process that speaks it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod credentials;
mod driver;
mod launch;
mod mapping;
mod prelude;
mod protocol;
pub mod runtime;
mod sign_in;
pub mod transcript;

pub use credentials::{
    ClaudeCredential, SETUP_TOKEN, account, credential, info,
    setup_token_credential,
};
pub use driver::{CallRequest, Effect, Failure, Input, Problem, Session, Then};
pub use launch::{
    Launch, Resume, SANDBOX_TOOLS, Sandbox, SettingSource, VersionPolicy,
    admit, auth_status, cli, login, profile, reported_refusal, sandbox,
    session, unprotected,
};
pub use mapping::{claude_answer, claude_steps};
pub use protocol::{
    ASK_USER_QUESTION, Applied, Block, ClaudeEvent, ControlCall, Inbound,
    MessageId, Open, Outbound, PermissionDecision, PermissionMode,
    PermissionRequestId, Role, SessionId, UnsupportedVersion, applied,
    check_version, encode, models, parse_line,
};
pub use sign_in::{
    SetupTokenOutput, setup_token_code, setup_token_link, setup_token_run,
    sign_in,
};
