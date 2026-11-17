//! The provider-agnostic agent API a session speaks: the steps a provider's
//! events map to, the replies it answers requests with, and how its process
//! ended. Each provider's wire maps into it (`claude`, `codex`).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod exit;
mod plan;
mod prelude;
mod step;

pub use exit::Exit;
pub use plan::PlanStep;
pub use step::{
    NativeRequest, NativeTurn, Reply, Step, reasoning, reasoning_delta,
};
