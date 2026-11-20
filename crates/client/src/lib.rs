//! The client core (architecture §9): a host's events folded into the view
//! the app draws, and the drafts the app sends back. It is pure and `no_std`;
//! the link that feeds it lives in `net`.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod answers;
pub mod device;
pub mod host_fold;
pub mod host_view;
pub mod link;
pub mod new_session;
mod prelude;
