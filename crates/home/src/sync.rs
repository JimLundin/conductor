//! Where the host's clone of the home stands.
//!
//! The host clones the home when it has no clone and pulls it otherwise,
//! on an interval and before it reads, unless it pulled a moment ago. A
//! failure is a state the host reports, never an error.

use proto::HomeSync;

use crate::prelude::*;

/// What the host runs next to bring its clone up to date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Git {
    Clone,
    Pull,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sync {
    /// When the clone last matched the remote, in seconds since the Unix
    /// epoch.
    pub synced: Option<u64>,
    /// When the host last tried.
    pub attempted: Option<u64>,
    pub status: HomeSync,
}

impl Default for Sync {
    fn default() -> Self {
        Self::new()
    }
}

impl Sync {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            synced: None,
            attempted: None,
            status: HomeSync::Syncing,
        }
    }

    /// Whether a read at `now` pulls first: the host hasn't tried within
    /// `fresh` seconds.
    #[must_use]
    pub fn due(&self, now: u64, fresh: u64) -> bool {
        self.attempted
            .is_none_or(|at| now.saturating_sub(at) >= fresh)
    }

    /// Where the clone stands once an attempt at `now` has ended, with a
    /// clone or without one.
    #[must_use]
    pub fn after(
        self,
        outcome: Result<(), String>,
        cloned: bool,
        now: u64,
    ) -> Self {
        match (outcome, cloned) {
            (Ok(()), _) => Self {
                synced: Some(now),
                attempted: Some(now),
                status: HomeSync::Synced,
            },
            (Err(reason), true) => Self {
                attempted: Some(now),
                status: HomeSync::Behind { reason },
                ..self
            },
            (Err(reason), false) => Self {
                attempted: Some(now),
                status: HomeSync::Failed { reason },
                ..self
            },
        }
    }
}

/// A clone with a `.git` is pulled; otherwise the host clones.
#[must_use]
pub const fn next(cloned: bool) -> Git {
    if cloned { Git::Pull } else { Git::Clone }
}
