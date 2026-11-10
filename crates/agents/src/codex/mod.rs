//! Codex driven over `codex app-server` JSON-RPC. One app-server process per
//! host serves every thread. Types come from the vendored `codex-codes` crate;
//! this module owns the process and maps the protocol into Conductor's types.

mod protocol;
mod server;

pub use protocol::{
    ApprovalId, ApprovalKind, ApprovalPolicy, CodexEvent, Decision, FileChange, Item, ItemPhase,
    ModelInfo, Sandbox, ThreadEvent, ThreadId, ThreadOptions, TurnId, TurnOutcome,
};
pub use server::{CodexServer, CodexThread, Error, Result, ServerOptions};
