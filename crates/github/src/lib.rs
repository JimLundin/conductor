#![no_std]

extern crate alloc;

pub mod configuration;

pub const ADAPTER: &str = "github";

pub mod access;

pub mod canvas;

pub mod log;

pub mod mapping;

mod number;

pub use number::{ISSUER, IssueNumber};
