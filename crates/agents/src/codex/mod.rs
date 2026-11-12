//! Codex driven over `codex app-server` JSON-RPC. One app-server process per
//! host serves every thread. `protocol` reads and writes the few messages we use;
//! this module owns the process and maps the protocol into Conductor's types.

mod config;
mod protocol;
mod server;

pub use protocol::{
    ApprovalId, ApprovalKind, ApprovalPolicy, ChangeKind, CodexEvent, Decision, FileChange, Item,
    ItemPhase, PermissionAsk, Sandbox, ThreadEvent, ThreadId, ThreadOptions, ThreadSettings,
    TurnId, TurnOutcome,
};
pub use server::{CodexServer, CodexThread, Error, ServerOptions, cli, login, login_status};
