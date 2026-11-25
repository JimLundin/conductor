#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod children;
mod credentials;
mod driver;
pub mod hub;
mod launch;
mod mapping;
pub mod pin;
mod prelude;
mod protocol;
pub mod runtime;
mod sign_in;

pub use credentials::{
    CodexCredential, account, credential, default_model, info,
};
pub use driver::{CallRequest, Effect, Failure, Input, Problem, Server, Then};
pub use launch::{
    Edit, KEY_VAR, app_server, cli, config_edits, login, login_status, profile,
    version,
};
pub use mapping::{codex_answer, codex_steps};
pub use protocol::{
    AnswerError, AnswerShape, ApprovalId, ApprovalKind, ApprovalPolicy, Call,
    ChildActivity, ChildEvent, ChildState, ChildStatus, CodexEvent, Decision,
    INITIALIZE, Inbound, Outbound, PermissionAsk, RequestId, ResponseError,
    Sandbox, SpawnCall, ThreadEvent, ThreadId, ThreadOptions, ThreadSettings,
    TurnId, answer, encode, models, parse_line, thread_started, turn_id,
    user_agent,
};
pub use sign_in::sign_in;

pub const PROVIDER: agents::Provider = agents::Provider::new("codex");
