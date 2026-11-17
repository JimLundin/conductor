//! How a CLI process ended.

use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exit {
    /// The exit status, as the platform describes it (`exit status: 1`).
    Status(String),
    /// Waiting on the killed process failed, with the error it gave.
    Unknown(String),
}

impl Exit {
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Status(status) => status.clone(),
            Self::Unknown(error) => format!("unknown exit ({error})"),
        }
    }
}
