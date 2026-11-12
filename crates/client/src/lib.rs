//! The client core (architecture §9). Compiles natively and to
//! `wasm32-unknown-unknown`; target differences live in [`platform`].

pub mod device;
pub mod host_view;
mod link;
pub mod new_session;
pub mod platform;

pub use link::{Cursor, HostLink, LinkOptions, LinkState, SendError, Status, Target, Update};
pub use platform::Network;
