//! The host's decisions, kept apart from its I/O: each session's state
//! machine, what its events say about its checkpoints and undo, how its
//! tasks follow the agent's checklist, the registry of sessions and
//! providers, what a restart recovers, and the secrets on disk. It is pure
//! and `no_std`; `daemon` runs it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod checkpoints;
mod prelude;
pub mod registry;
pub mod restore;
pub mod secret;
pub mod session;
pub mod tasks;
