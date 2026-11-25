use core::time::Duration;

use crate::{HomeSync, UnixSecs};

use crate::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Git {
    Clone,
    Pull,
    Replay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sync {
    pub synced: Option<UnixSecs>,
    pub attempted: Option<UnixSecs>,
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

    #[must_use]
    pub fn due(&self, now: UnixSecs, fresh: Duration) -> bool {
        self.attempted
            .is_none_or(|at| now.elapsed_since(at) >= fresh)
    }

    #[must_use]
    pub fn after(
        self,
        outcome: Result<(), String>,
        cloned: bool,
        now: UnixSecs,
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

#[must_use]
pub const fn next(cloned: bool, unpushed: bool) -> Git {
    match (cloned, unpushed) {
        (false, _) => Git::Clone,
        (true, false) => Git::Pull,
        (true, true) => Git::Replay,
    }
}

#[must_use]
pub fn queue_needs_check(previous: Option<&str>, address: &str) -> bool {
    previous.is_some_and(|old| !same_remote(old, address))
}

#[must_use]
pub fn same_remote(left: &str, right: &str) -> bool {
    crate::checkout::display(left) == crate::checkout::display(right)
}

pub fn publishing_branch(tracking: &str, head: &str) -> Result<String, String> {
    let name = tracking
        .strip_prefix("refs/remotes/origin/")
        .ok_or_else(|| String::from("invalid home origin default branch"))?;
    let branch = format!("refs/heads/{name}");
    if head == branch {
        Ok(branch)
    } else {
        Err(String::from("the home checkout is on a different branch"))
    }
}
