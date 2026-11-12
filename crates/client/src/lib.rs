//! The client core (architecture §9). Compiles natively and to
//! `wasm32-unknown-unknown`; target differences live in [`platform`].

pub mod host_view;
mod link;
pub mod new_session;
pub mod platform;
mod upgrade;
mod wire;

pub use link::{
    CandidatePair, Cursor, HostLink, LinkOptions, LinkState, PathKind, SendError, Status, Target,
    Update,
};
pub use platform::Network;
