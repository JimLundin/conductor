//! The host's decisions, kept apart from its I/O: what a session's events
//! say about its checkpoints and undo, and how its tasks follow the agent's
//! checklist. It is pure and `no_std`; `daemon` runs it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod checkpoints;
mod prelude;
pub mod tasks;
