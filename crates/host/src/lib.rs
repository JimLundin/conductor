//! The host's decisions, kept apart from its I/O: each session's state
//! machine, what its events say about its commit snapshots, how its
//! tasks follow the agent's checklist, how a session command is routed
//! ([`serve`]), the event log's, those of each session's Git worktree, the
//! registry of sessions and providers, how each provider's CLI starts and
//! signs in from the app, what a restart recovers, and the secrets on
//! disk. It is pure and `no_std`; `daemon` runs it.
//!
//! It owns the types of those domains (ADR 0003): the session record
//! ([`log`], [`session`], [`workspace`], [`finish`]), instances and the
//! roster ([`registry`]), sign-ins ([`sign_in`]), push ([`push`]) and the
//! fleet ([`fleet`]). Host stores and traffic between hosts are postcard,
//! where a variant is its index and a field its position, so there a
//! reordered type decodes as the wrong value with no error.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod assignment;
pub mod finish;
pub mod fleet;
mod json;
pub mod log;
mod note;
mod prelude;
pub mod push;
pub mod registry;
pub mod restore;
pub mod secret;
pub mod serve;
pub mod session;
pub mod sign_in;
mod snapshots;
pub mod start;
pub mod workspace;

pub use note::Note;
