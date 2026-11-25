//! The home's decisions (docs/home.md): what a project's `project.toml`
//! says, which local repositories are a project's checkouts, where the
//! host's clone of the home stands, and what a host says of it. The home is
//! fleet-wide data in its own files, read by hosts now and by more of the
//! fleet as issues and work land, so it is a core of its own rather than
//! part of `host`'s session machinery. It is pure and `no_std`; `daemon`
//! runs it.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod calendar;
pub mod checkout;
pub mod flow;
mod home;
mod id;
pub mod instructions;
pub mod issue;
pub mod layout;
mod prelude;
pub mod project;
pub mod repos;
pub mod skills;
pub mod sync;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;

pub use home::{
    Checkout, CheckoutLocation, Cloning, Home, HomeCommand, HomeRefusal,
    HomeResult, HomeSync, Origin, Project,
};
pub use id::{DRAWS, Draws, IssueId, ProjectId};
use serde::{Deserialize, Serialize};

use crate::flow::Flow;
use crate::issue::Issue;
use crate::prelude::*;
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
                    checkouts
                        .iter()
                        .map(|checkout| (checkout.path.0.clone(), *project))
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
    /// The home's flow, when it loads.
    pub flow: Option<&'host Flow>,
    pub issues: &'host BTreeMap<ProjectId, Vec<Issue>>,
    /// Issue changes the remote doesn't have yet.
    pub unpushed: u64,
}

/// The home as the host reports it.
#[must_use]
pub fn view(
    remote: &str,
    sync: &Sync,
    projects: &[project::Project],
    found: Found<'_>,
) -> Home {
    let Found {
        checkouts,
        cloning,
        flow,
        issues,
        unpushed,
    } = found;
    Home {
        remote: checkout::display(remote),
        synced_secs: sync.synced_secs,
        sync: sync.status.clone(),
        unpushed,
        flow: flow.cloned().map(Box::new),
        projects: projects
            .iter()
            .map(|project| Project {
                id: project.id,
                name: project.name.clone(),
                title: project.title.clone(),
                remote: project.remote.as_deref().map(checkout::display),
                profiles: project.profiles.clone(),
                provides: project.provides.clone(),
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

/// Where a session on project `id` starts: its first checkout.
///
/// # Errors
///
/// The host has no home, its home lists no such project, or the project
/// has no checkout on the host.
pub fn first_checkout(
    home: Option<&Home>,
    id: ProjectId,
) -> Result<CheckoutLocation, HomeRefusal> {
    let known = home.ok_or(HomeRefusal::NoHome)?;
    let project = known
        .projects
        .iter()
        .find(|project| project.id == id)
        .ok_or(HomeRefusal::UnknownProject)?;
    project
        .checkouts
        .first()
        .map(|checkout| checkout.path.clone())
        .ok_or(HomeRefusal::NotCheckedOut)
}

/// The remote to clone a project from.
///
/// # Errors
///
/// The home lists no such project, or it is local only.
pub fn remote_of(
    projects: &[project::Project],
    id: &ProjectId,
) -> Result<String, HomeRefusal> {
    projects
        .iter()
        .find(|project| &project.id == id)
        .ok_or(HomeRefusal::UnknownProject)?
        .remote
        .clone()
        .ok_or(HomeRefusal::NoRemote)
}

/// A home command runs only on a host given a home.
///
/// # Errors
///
/// [`HomeRefusal::NoHome`] without a remote.
pub const fn configured(remote: Option<&str>) -> Result<(), HomeRefusal> {
    match remote {
        Some(_) => Ok(()),
        None => Err(HomeRefusal::NoHome),
    }
}

/// A home command's answer once it has run: how it went is in the home.
#[must_use]
pub fn answer(done: Result<(), HomeRefusal>) -> HomeResult {
    done.map_or_else(HomeResult::Refused, |()| HomeResult::Accepted)
}

/// Where a clone of the project stands once `git clone` has ended.
#[must_use]
pub fn cloned(outcome: Result<(), String>) -> Cloning {
    outcome.map_or_else(|reason| Cloning::Failed { reason }, |()| Cloning::Idle)
}

#[cfg(test)]
mod tests;
