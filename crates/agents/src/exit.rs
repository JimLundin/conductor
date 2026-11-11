//! How a CLI process ended.

use std::process::ExitStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exit {
    Status(ExitStatus),
    /// Waiting on the killed process failed, with the error it gave.
    Unknown(String),
}

impl Exit {
    pub fn describe(&self) -> String {
        match self {
            Exit::Status(status) => status.to_string(),
            Exit::Unknown(error) => format!("unknown exit ({error})"),
        }
    }
}
