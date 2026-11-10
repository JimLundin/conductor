//! The client core (architecture §9). Compiles natively and to
//! `wasm32-unknown-unknown`; target differences live in [`platform`].

mod link;
pub mod platform;
pub mod transcript;
mod wire;

pub use link::{Cursor, HostLink, LinkState, PathKind, SendError, Status, Target, Update};
