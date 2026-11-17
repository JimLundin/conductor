//! Codex driven over `codex app-server` JSON-RPC.
//!
//! One app-server process per host serves every thread. `codex` reads and
//! writes the few messages we use; this crate owns the process.

mod config;
mod server;

pub use server::{
    CodexServer, CodexThread, Error, ServerOptions, cli, login, login_status,
};
