#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod credentials;
mod driver;
mod launch;
mod mapping;
pub mod pin;
mod prelude;
mod protocol;
pub mod runtime;
mod sign_in;

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
    ASK_USER_QUESTION, Applied, Block, ClaudeEvent, ControlCall, Inbound, Open,
    Outbound, PermissionDecision, PermissionMode, PermissionRequestId, Role,
    SessionId, UnsupportedVersion, applied, check_version, encode, models,
    parse_line,
};
pub use sign_in::{
    SetupTokenOutput, setup_token_code, setup_token_link, setup_token_run,
    sign_in,
};

pub const PROVIDER: agents::Provider = agents::Provider::new("claude");
