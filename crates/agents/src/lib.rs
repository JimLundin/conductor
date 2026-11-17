//! The provider-agnostic agent API a session speaks: the steps a provider's
//! events map to, the replies it answers requests with, how its process
//! starts and how it ended. Each provider's wire maps into it (`claude`,
//! `codex`).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod exit;
pub mod pin;
mod plan;
mod prelude;
mod run;
mod step;

pub use exit::Exit;
pub use plan::PlanStep;
pub use run::{HomeKind, Invocation, Var};
pub use step::{
    NativeRequest, NativeTurn, Reply, Step, reasoning, reasoning_delta,
};
