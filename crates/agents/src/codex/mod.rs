//! Codex driven over `codex app-server` JSON-RPC. One app-server process per
//! host serves every thread. `protocol` reads and writes the few messages we use;
//! this module owns the process and maps the protocol into Conductor's types.

mod protocol;
mod server;

pub use protocol::{
    ApprovalId, ApprovalKind, ApprovalPolicy, ChangeKind, CodexEvent, Decision, FileChange, Item,
    ItemPhase, ModelInfo, PermissionAsk, Sandbox, ThreadEvent, ThreadId, ThreadOptions, TurnId,
    TurnOutcome,
};
pub use server::{CodexServer, CodexThread, Error, ServerOptions};
