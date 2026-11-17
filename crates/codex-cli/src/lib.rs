//! Codex driven over `codex app-server` JSON-RPC.
//!
//! One app-server process per host serves every thread. `codex` reads and
//! writes the few messages we use and decides what the server and the hub
//! do; this crate owns the process, and the hub that shares it among
//! threads.

mod config;
mod hub;
mod server;

pub use hub::{Hub, Thread};
pub use server::{
    CodexServer, CodexThread, Error, ServerOptions, cli, login, login_status,
};
