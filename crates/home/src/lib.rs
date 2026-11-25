#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod calendar;
pub mod canvas;
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

use crate::canvas::CanvasEntry;
use crate::flow::Flow;
use crate::issue::Issue;
use crate::prelude::*;
use crate::sync::Sync;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub remote: Option<String>,
    pub checkouts: BTreeMap<String, ProjectId>,
}

impl Stored {
    #[must_use]
    pub fn remote(&self, given: Option<String>) -> Option<String> {
        given.or_else(|| self.remote.clone())
    }

    #[must_use]
    pub fn known(&self) -> Vec<String> {
        self.checkouts.keys().cloned().collect()
    }

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

#[derive(Debug, Clone, Copy)]
pub struct Found<'host> {
    pub checkouts: &'host BTreeMap<ProjectId, Vec<Checkout>>,
    pub cloning: &'host BTreeMap<ProjectId, Cloning>,
    pub flow: Option<&'host Flow>,
    pub issues: &'host BTreeMap<ProjectId, Vec<Issue>>,
    pub canvases: &'host BTreeMap<IssueId, Vec<CanvasEntry>>,
    pub unpushed: u64,
}

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
        canvases,
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
                issues: issues
                    .get(&project.id)
                    .into_iter()
                    .flatten()
                    .map(|issue| Issue {
                        canvases: canvases
                            .get(&issue.id)
                            .cloned()
                            .unwrap_or_default(),
                        ..issue.clone()
                    })
                    .collect(),
            })
            .collect(),
    }
}

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

pub const fn configured(remote: Option<&str>) -> Result<(), HomeRefusal> {
    match remote {
        Some(_) => Ok(()),
        None => Err(HomeRefusal::NoHome),
    }
}

#[must_use]
pub fn answer(done: Result<(), HomeRefusal>) -> HomeResult {
    done.map_or_else(HomeResult::Refused, |()| HomeResult::Accepted)
}

#[must_use]
pub fn cloned(outcome: Result<(), String>) -> Cloning {
    outcome.map_or_else(|reason| Cloning::Failed { reason }, |()| Cloning::Idle)
}

#[cfg(test)]
mod tests;
