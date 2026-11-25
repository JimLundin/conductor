use alloc::string::String;
use alloc::vec::Vec;

use crate::backend::Capability;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Statuses,
    Checks,
    Workflows,
}

impl Source {
    #[must_use]
    pub const fn capability(self) -> Capability {
        match self {
            Self::Statuses => Capability::ReadStatuses,
            Self::Checks => Capability::ReadChecks,
            Self::Workflows => Capability::ReadWorkflows,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub revision: String,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub revision: String,
    pub source: Source,
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub state: State,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Passed,
    Failed,
    Unavailable,
}
