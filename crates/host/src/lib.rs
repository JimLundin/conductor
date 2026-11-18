//! The host's decisions, kept apart from its I/O: each session's state
//! machine, what its events say about its checkpoints and undo, how its
//! tasks follow the agent's checklist, the protocol's rules on a
//! connection, the event log's, those of each session's Git worktree, the
//! registry of sessions and providers, how each provider's CLI starts,
//! what a restart recovers, and the secrets on disk. It is pure and `no_std`; `daemon` runs it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod checkpoints;
pub mod log;
mod note;
mod prelude;
pub mod registry;
pub mod restore;
pub mod secret;
pub mod serve;
pub mod session;
pub mod start;
pub mod tasks;
pub mod workspace;

pub use note::Note;
