//! Codex's `codex app-server` JSON-RPC: the few messages we read and write,
//! and their events mapped into the agent API (`agents`), how the server
//! starts, what the server actor decides, what the hub that shares the
//! server among threads decides, what a thread's agent decides with a
//! session's runtime commands, and how its sign-in reads. `codex-cli`
//! runs the server that speaks it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod children;
mod credentials;
mod driver;
pub mod hub;
mod launch;
mod mapping;
mod prelude;
mod protocol;
pub mod runtime;
mod sign_in;

pub use credentials::{
    CodexCredential, account, credential, default_model, info,
};
pub use driver::{CallRequest, Effect, Failure, Input, Problem, Server, Then};
pub use launch::{
    Edit, History, KEY_VAR, Paging, app_server, cli, config_edits, login,
    login_status, profile, version,
};
pub use mapping::{codex_answer, codex_steps};
pub use protocol::{
    AnswerError, AnswerShape, ApprovalId, ApprovalKind, ApprovalPolicy, Call,
    ChildActivity, ChildEvent, ChildState, ChildStatus, CodexEvent, Decision,
    INITIALIZE, Inbound, Outbound, PermissionAsk, RequestId, ResponseError,
    Sandbox, SpawnCall, ThreadEvent, ThreadId, ThreadOptions, ThreadSettings,
    TurnId, answer, encode, history_page, models, parse_line, thread_started,
    turn_id, user_agent,
};
pub use sign_in::sign_in;
