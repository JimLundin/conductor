//! Where the host's clone of the home stands.
//!
//! The host clones the home when it has no clone and pulls it otherwise,
//! on an interval and before it reads, unless it pulled a moment ago. A
//! failure is a state the host reports, never an error.

use crate::HomeSync;

use crate::prelude::*;

/// What the host runs next to bring its clone up to date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Git {
    /// Clone, then replay the host's unpushed issue changes, if any.
    Clone,
    Pull,
    /// Fetch, reset to the remote, replay the host's unpushed issue
    /// changes on top and push them: no merge of its own commits.
    Replay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sync {
    /// When the clone last matched the remote, in seconds since the Unix
    /// epoch.
    pub synced_secs: Option<u64>,
    /// When the host last tried.
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

    /// Whether a read at `now` pulls first: the host hasn't tried within
    /// `fresh` seconds.
    #[must_use]
    pub fn due(&self, now_secs: u64, fresh_secs: u64) -> bool {
        self.attempted_secs
            .is_none_or(|at| now_secs.saturating_sub(at) >= fresh_secs)
    }

    /// Where the clone stands once an attempt at `now` has ended, with a
    /// clone or without one.
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

/// A clone with a `.git` is pulled, or replayed onto when the host has
/// changes the remote lacks; otherwise the host clones.
#[must_use]
pub const fn next(cloned: bool, unpushed: bool) -> Git {
    match (cloned, unpushed) {
        (false, _) => Git::Clone,
        (true, false) => Git::Pull,
        (true, true) => Git::Replay,
    }
}

/// The queued edits must stay with their original home.
#[must_use]
pub fn queue_needs_check(previous: Option<&str>, address: &str) -> bool {
    previous.is_some_and(|old| !same_remote(old, address))
}

/// Home identity excludes transient Git user information.
#[must_use]
pub fn same_remote(left: &str, right: &str) -> bool {
    crate::checkout::display(left) == crate::checkout::display(right)
}

/// The branch to publish, derived from origin's default branch and
/// confirmed against the checked-out branch.
///
/// # Errors
/// The tracking ref is invalid or the checkout is on a different branch.
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
