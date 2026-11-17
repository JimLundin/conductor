//! The provider-agnostic agent API a session speaks: the runtime protocol
//! of commands and events, the steps a provider's events map to, the
//! replies it answers requests with, how its process starts and how it
//! ended. Each provider's wire maps into it (`claude`, `codex`).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod exit;
mod note;
pub mod pin;
mod plan;
mod prelude;
mod run;
pub mod runtime;
mod step;

pub use exit::Exit;
pub use note::{Level, Note};
pub use plan::PlanStep;
pub use run::{HomeKind, Invocation, Var};
pub use step::{
    NativeRequest, NativeTurn, Reply, Step, reasoning, reasoning_delta,
};
