//! Codex driven over `codex app-server` JSON-RPC.
//!
//! One app-server process per host serves every thread. `codex` reads and
//! writes the few messages we use and decides what the server, the hub and
//! each agent do; this crate owns the process, the hub that shares it among
//! threads, runs each agent behind a `runtime::Runtime`, and hands the host
//! Codex's [`plugin`].

mod agent;
mod config;
mod hub;
mod plugin;
mod server;

pub use agent::starter;
pub use hub::{Hub, Thread};
pub use plugin::plugin;
pub use server::{CodexServer, CodexThread, Error, ServerOptions};
