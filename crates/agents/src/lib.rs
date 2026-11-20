//! The provider-agnostic agent API a session speaks: the runtime protocol
//! of commands and events, the steps a provider's events map to, the
//! replies it answers requests with, how its process starts and how it
//! ended, and the profile of its CLI the host starts and signs it in by. Each
//! provider's wire maps into it (`claude`, `codex`).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod driver;
mod exit;
mod note;
pub mod pin;
mod plan;
mod prelude;
mod profile;
mod run;
pub mod runtime;
mod sign_in;
mod step;

pub use exit::Exit;
pub use note::Note;
pub use plan::PlanStep;
pub use profile::{Config, Profile, Reported};
pub use run::{HomeKind, Invocation, Var};
pub use sign_in::{Failure, Pick, Shown, SignIn, Word, plain};
pub use step::{
    NativeRequest, NativeTurn, Reply, Step, reasoning, reasoning_delta,
};
