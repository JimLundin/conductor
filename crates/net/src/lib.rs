//! The client link on iroh: its frame I/O on streams, the generic reads
//! the fleet framing also uses, and how a relay is named.

pub mod platform;
mod relay;
pub mod wire;

pub use relay::without_root_dot;
