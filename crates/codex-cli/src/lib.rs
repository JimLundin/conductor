//! Codex driven over `codex app-server` JSON-RPC.
//!
//! One app-server per account serves its threads. `codex` reads and
//! writes the few messages we use and decides what the server, the hub and
//! each agent do; this crate owns the process, the hub that shares it among
//! threads, runs each agent behind a `runtime::Runtime`, and hands the host
//! Codex's [`plugin`].

mod agent;
mod config;
mod hub;
mod plugin;
mod server;

#[cfg(test)]
#[cfg(unix)]
#[path = "../tests/support/mod.rs"]
mod replay_support;

pub use plugin::plugin;
