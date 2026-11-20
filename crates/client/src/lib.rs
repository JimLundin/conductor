//! The pure `no_std` client core (architecture §7) folds host updates into
//! the view the app draws and decides how the link in `net` keeps up.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod device;
pub mod host_fold;
pub mod host_view;
pub mod link;
mod prelude;
