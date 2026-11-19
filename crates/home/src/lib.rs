//! The home's decisions (docs/home.md): what a project's `project.toml`
//! says, which local repositories are a project's checkouts, where the
//! host's clone of the home stands, and what a host says of it. The home is
//! fleet-wide data in its own files, read by hosts now and by more of the
//! fleet as issues and work land, so it is a core of its own rather than
//! part of `host`'s session machinery. It is pure and `no_std`; `daemon`
//! runs it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod checkout;
pub mod issue;
mod prelude;
pub mod project;
pub mod sync;

use alloc::collections::BTreeMap;

use proto::{
    Checkout, Cloning, CommandResult, Home, HostPath, Issue, ProjectId,
    Rejection, StartAt,
};
use serde::{Deserialize, Serialize};

use crate::prelude::*;
use crate::project::Project;
use crate::sync::Sync;

/// What the host keeps of its home between runs: the remote it was given,
/// and its checkouts as last found, each by its path with its project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub remote: Option<String>,
    pub checkouts: BTreeMap<String, ProjectId>,
}

impl Stored {
    /// The remote the command line gives, else the one kept.
    #[must_use]
    pub fn remote(&self, given: Option<String>) -> Option<String> {
        given.or_else(|| self.remote.clone())
    }

    /// Every path a checkout was found at, which the next scan probes again.
    #[must_use]
    pub fn known(&self) -> Vec<String> {
        self.checkouts.keys().cloned().collect()
    }

    /// What to keep of a scan's checkouts, with the remote.
    #[must_use]
    pub fn kept(
        remote: Option<String>,
        found: &BTreeMap<ProjectId, Vec<Checkout>>,
    ) -> Self {
        Self {
            remote,
            checkouts: found
                .iter()
                .flat_map(|(project, checkouts)| {
                    checkouts.iter().map(|checkout| {
                        (checkout.path.0.clone(), project.clone())
                    })
                })
                .collect(),
        }
    }
}

/// What the host found of its projects beside what the home lists.
#[derive(Debug, Clone, Copy)]
pub struct Found<'host> {
    pub checkouts: &'host BTreeMap<ProjectId, Vec<Checkout>>,
    pub cloning: &'host BTreeMap<ProjectId, Cloning>,
    pub issues: &'host BTreeMap<ProjectId, Vec<Issue>>,
    /// Issue changes the remote doesn't have yet.
    pub unpushed: u64,
}

/// The home as the host reports it.
#[must_use]
pub fn view(
    remote: &str,
    sync: &Sync,
    projects: &[Project],
    found: Found<'_>,
) -> Home {
    let Found {
        checkouts,
        cloning,
        issues,
        unpushed,
    } = found;
    Home {
        remote: remote.to_owned(),
        synced: sync.synced,
        sync: sync.status.clone(),
        unpushed,
        projects: projects
            .iter()
            .map(|project| proto::Project {
                id: project.id.clone(),
                name: project.name.clone(),
                title: project.title.clone(),
                remote: project.remote.clone(),
                profiles: project.profiles.clone(),
                checkouts: checkouts
                    .get(&project.id)
                    .cloned()
                    .unwrap_or_default(),
                cloning: cloning
                    .get(&project.id)
                    .cloned()
                    .unwrap_or(Cloning::Idle),
                issues: issues.get(&project.id).cloned().unwrap_or_default(),
            })
            .collect(),
    }
}

/// The directory a session starts in: the one given, or a project's first
/// checkout. The session's project follows from its worktree's repository,
/// so it isn't kept apart.
///
/// # Errors
///
/// The host has no home, its home lists no such project, or the project
/// has no checkout on the host.
pub fn place(at: StartAt, home: Option<&Home>) -> Result<HostPath, Rejection> {
    match at {
        StartAt::Directory(directory) => Ok(directory),
        StartAt::Project(id) => {
            let known = home.ok_or(Rejection::NoHome)?;
            let project = known
                .projects
                .iter()
                .find(|project| project.id == id)
                .ok_or(Rejection::UnknownProject)?;
            let checkout =
                project.checkouts.first().ok_or(Rejection::NotCheckedOut)?;
            Ok(checkout.path.clone())
        }
    }
}

/// The remote to clone a project from.
///
/// # Errors
///
/// The home lists no such project, or it is local only.
pub fn remote_of(
    projects: &[Project],
    id: &ProjectId,
) -> Result<String, Rejection> {
    projects
        .iter()
        .find(|project| &project.id == id)
        .ok_or(Rejection::UnknownProject)?
        .remote
        .clone()
        .ok_or(Rejection::NoRemote)
}

/// A home command runs only on a host given a home.
///
/// # Errors
///
/// [`Rejection::NoHome`] without a remote.
pub const fn configured(remote: Option<&str>) -> Result<(), Rejection> {
    match remote {
        Some(_) => Ok(()),
        None => Err(Rejection::NoHome),
    }
}

/// A home command's answer once it has run: how it went is in the home.
#[must_use]
pub fn answer(done: Result<(), Rejection>) -> CommandResult {
    done.map_or_else(CommandResult::Rejected, |()| CommandResult::Accepted)
}

/// Where a clone of the project stands once `git clone` has ended.
#[must_use]
pub fn cloned(outcome: Result<(), String>) -> Cloning {
    outcome.map_or_else(|reason| Cloning::Failed { reason }, |()| Cloning::Idle)
}

#[cfg(test)]
mod tests;
