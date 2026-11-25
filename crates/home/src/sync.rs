use crate::HomeSync;

use crate::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Git {
    Clone,
    Pull,
    Replay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sync {
    pub synced_secs: Option<u64>,
    pub attempted_secs: Option<u64>,
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
            synced_secs: None,
            attempted_secs: None,
            status: HomeSync::Syncing,
        }
    }

    #[must_use]
    pub fn due(&self, now_secs: u64, fresh_secs: u64) -> bool {
        self.attempted_secs
            .is_none_or(|at| now_secs.saturating_sub(at) >= fresh_secs)
    }

    #[must_use]
    pub fn after(
        self,
        outcome: Result<(), String>,
        cloned: bool,
        now_secs: u64,
    ) -> Self {
        match (outcome, cloned) {
            (Ok(()), _) => Self {
                synced_secs: Some(now_secs),
                attempted_secs: Some(now_secs),
                status: HomeSync::Synced,
            },
            (Err(reason), true) => Self {
                attempted_secs: Some(now_secs),
                status: HomeSync::Behind { reason },
                ..self
            },
            (Err(reason), false) => Self {
                attempted_secs: Some(now_secs),
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
    vcs::remote::display(left) == vcs::remote::display(right)
}

pub fn publishing_branch(
    default: Option<&str>,
    head: &str,
) -> Result<String, String> {
    let branch = default
        .ok_or_else(|| String::from("invalid home origin default branch"))?;
    if head == branch {
        Ok(String::from(branch))
    } else {
        Err(String::from("the home checkout is on a different branch"))
    }
}
