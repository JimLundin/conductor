//! Codex's `codex app-server` JSON-RPC: the few messages we read and write,
//! and their events mapped into the agent API (`agents`). `codex-cli` runs
//! the server that speaks it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod mapping;
mod prelude;
mod protocol;

pub use mapping::{codex_answer, codex_steps};
pub use protocol::{
    AnswerError, AnswerShape, ApprovalId, ApprovalKind, ApprovalPolicy, Call,
    CodexEvent, Decision, INITIALIZE, Inbound, Outbound, PermissionAsk,
    RequestId, Sandbox, ThreadEvent, ThreadId, ThreadOptions, ThreadSettings,
    TurnId, answer, encode, history_page, models, parse_line, thread_started,
    turn_id, user_agent,
};
