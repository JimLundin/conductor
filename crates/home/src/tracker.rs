use alloc::collections::BTreeMap;
use core::fmt;

use crate::flow::snapshot::Snapshot;
use crate::prelude::*;
use crate::project::Project;
use crate::{IssueId, IssueKey, IssueName, ProjectId, Slug, TrackerId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tracker<'a> {
    Markdown,
    Configured(&'a Project),
}

#[derive(Clone, Copy)]
pub struct Issuer {
    adapter: &'static str,
    read: fn(&str) -> Option<IssueId>,
}

impl Issuer {
    pub const MARKDOWN: Self = Self::of::<Slug>("markdown");

    #[must_use]
    pub const fn of<T: TrackerId>(adapter: &'static str) -> Self {
        Self {
            adapter,
            read: read_as::<T>,
        }
    }

    #[must_use]
    pub fn read(&self, text: &str) -> Option<IssueId> {
        (self.read)(text)
    }
}

fn read_as<T: TrackerId>(text: &str) -> Option<IssueId> {
    T::read(text).map(|id| IssueId::of(&id))
}

impl PartialEq for Issuer {
    fn eq(&self, other: &Self) -> bool {
        self.adapter == other.adapter
    }
}

impl Eq for Issuer {}

impl fmt::Debug for Issuer {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(self.adapter)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Issuers(&'static [Issuer]);

impl Issuers {
    #[must_use]
    pub const fn new(configured: &'static [Issuer]) -> Self {
        Self(configured)
    }

    #[must_use]
    pub fn ids(&self, projects: &[Project]) -> Ids {
        Ids(projects
            .iter()
            .filter_map(|project| {
                let adapter = &project.backends.tracker.as_ref()?.adapter;
                let issuer = self
                    .0
                    .iter()
                    .find(|issuer| issuer.adapter == adapter)
                    .copied();
                Some((project.id, issuer))
            })
            .collect())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unread {
    Untracked,
    Unknown,
}

impl Unread {
    #[must_use]
    pub fn said(self, project: impl core::fmt::Display, text: &str) -> String {
        match self {
            Self::Untracked => {
                format!("no tracker of {project} reads issue IDs")
            }
            Self::Unknown => format!("{text:?} is no issue ID of {project}"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ids(BTreeMap<ProjectId, Option<Issuer>>);

impl Ids {
    #[must_use]
    pub fn with(mut self, project: ProjectId, issuer: Issuer) -> Self {
        self.0.insert(project, Some(issuer));
        self
    }

    pub fn read(
        &self,
        project: ProjectId,
        text: &str,
    ) -> Result<IssueId, Unread> {
        let issuer = self
            .0
            .get(&project)
            .copied()
            .unwrap_or(Some(Issuer::MARKDOWN))
            .ok_or(Unread::Untracked)?;
        issuer.read(text).ok_or(Unread::Unknown)
    }

    pub fn key(&self, name: &IssueName) -> Result<IssueKey, String> {
        Ok(IssueKey {
            project: name.project,
            id: self
                .read(name.project, &name.id)
                .map_err(|unread| unread.said(name.project, &name.id))?,
        })
    }
}

#[must_use]
pub const fn of(project: &Project) -> Tracker<'_> {
    if project.backends.tracker.is_some() {
        Tracker::Configured(project)
    } else {
        Tracker::Markdown
    }
}

#[must_use]
pub fn select(home: &Snapshot, project: ProjectId) -> Option<Tracker<'_>> {
    home.projects()
        .iter()
        .find(|known| known.id == project)
        .map(of)
}

pub fn writable(home: &Snapshot, project: ProjectId) -> Result<(), String> {
    match select(home, project) {
        Some(Tracker::Markdown) => Ok(()),
        Some(Tracker::Configured(_)) => Err(String::from(
            "edit this issue in the configured tracker; local issue writes are disabled for this project",
        )),
        None => Err(String::from(
            "local issue writes require a valid project configuration",
        )),
    }
}
