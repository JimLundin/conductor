//! Codex driven over `codex app-server` JSON-RPC.
//!
//! One app-server process per host serves every thread. `codex` reads and
//! writes the few messages we use and decides what the server, the hub and
//! each agent do; this crate owns the process, the hub that shares it among
//! threads, and runs each agent behind a `runtime::Runtime`.

mod agent;
mod config;
mod home;
mod hub;
mod server;

pub use agent::starter;
pub use home::private_home;
pub use hub::{Hub, Thread};
pub use server::{
    CodexServer, CodexThread, Error, ServerOptions, cli, login, login_status,
};
