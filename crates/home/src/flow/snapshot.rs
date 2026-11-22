//! The home as one reading of its files.
//!
//! Its flow, its projects, every issue file and the graph the flow reads
//! from them. The check, every write and every replay are made against one
//! of these, so each sees the home the same way.

use alloc::collections::BTreeMap;

use proto::flow::Flow;
use proto::{Issue, IssueId, ProjectId};

use super::graph::{File, Graph, read};
use super::{PATH, load};
use crate::prelude::*;
use crate::project::{Listing, Project, listed};

/// What the shell found of the home's files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inputs {
    /// The text at [`PATH`], or why it couldn't be read.
    pub flow: Result<String, String>,
    /// Each folder of `projects/` with its `project.toml`, or why not.
    pub projects: Vec<(String, Result<String, String>)>,
    pub issues: Vec<File>,
    /// Why each issue file that couldn't be read wasn't, naming it.
    pub unread: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    flow: Result<Flow, String>,
    listing: Listing,
    files: Vec<File>,
    unread: Vec<String>,
    graph: Graph,
}

impl Snapshot {
    /// The home `inputs` hold. Its issues are read only when its flow
    /// loads.
    #[must_use]
    pub fn of(inputs: Inputs) -> Self {
        let flow = inputs
            .flow
            .and_then(|text| load(&text))
            .map_err(|problem| format!("{PATH}: {problem}"));
        let graph = flow.as_ref().map_or_else(
            |_| Graph::default(),
            |read_flow| read(read_flow, &inputs.issues),
        );
        Self {
            flow,
            listing: listed(inputs.projects),
            files: inputs.issues,
            unread: inputs.unread,
            graph,
        }
    }

    /// A home with nothing in it, for this reason.
    #[must_use]
    pub fn empty(reason: String) -> Self {
        Self {
            flow: Err(reason),
            listing: Listing::default(),
            files: Vec::new(),
            unread: Vec::new(),
            graph: Graph::default(),
        }
    }

    /// The home with `file` written, in place of any file it had of the
    /// same name.
    #[must_use]
    pub fn with(&self, file: File) -> Self {
        let mut files: Vec<File> = self
            .files
            .iter()
            .filter(|other| {
                other.project != file.project || other.stem != file.stem
            })
            .cloned()
            .collect();
        files.push(file);
        let graph = self
            .flow
            .as_ref()
            .map_or_else(|_| Graph::default(), |flow| read(flow, &files));
        Self {
            files,
            graph,
            ..self.clone()
        }
    }

    /// The flow, or why it doesn't load.
    ///
    /// # Errors
    ///
    /// The flow couldn't be read or doesn't load.
    pub fn flow(&self) -> Result<&Flow, &str> {
        self.flow.as_ref().map_err(String::as_str)
    }

    /// The projects it lists, by id.
    #[must_use]
    pub fn projects(&self) -> &[Project] {
        &self.listing.projects
    }

    /// The project called `name`, or whose id it is.
    #[must_use]
    pub fn project(&self, name: &str) -> Option<&Project> {
        self.projects()
            .iter()
            .find(|project| project.name == name || project.id.0 == name)
    }

    /// Every issue file that could be read.
    #[must_use]
    pub fn files(&self) -> &[File] {
        &self.files
    }

    #[must_use]
    pub const fn graph(&self) -> &Graph {
        &self.graph
    }

    #[must_use]
    pub fn issue(&self, id: IssueId) -> Option<&Issue> {
        self.graph.issues.iter().find(|issue| issue.id == id)
    }

    /// Its issues, by project and id.
    #[must_use]
    pub fn issues(&self) -> BTreeMap<ProjectId, Vec<Issue>> {
        let mut issues: BTreeMap<ProjectId, Vec<Issue>> = BTreeMap::new();
        for issue in &self.graph.issues {
            issues
                .entry(issue.project.clone())
                .or_default()
                .push(issue.clone());
        }
        issues
    }

    /// The project folders that aren't projects, and why.
    #[must_use]
    pub fn listing_problems(&self) -> &[String] {
        &self.listing.problems
    }

    /// Why each issue file that couldn't be read wasn't.
    #[must_use]
    pub fn unread(&self) -> &[String] {
        &self.unread
    }

    /// Everything that kept part of the home from being read: a flow that
    /// doesn't load, a folder that isn't a project, a file that couldn't
    /// be read or isn't an issue, each naming where.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        self.flow
            .as_ref()
            .err()
            .cloned()
            .into_iter()
            .chain(self.listing.problems.iter().cloned())
            .chain(self.unread.iter().cloned())
            .chain(
                self.graph
                    .problems
                    .iter()
                    .map(|problem| format!("{}: {problem}", problem.issue)),
            )
            .collect()
    }
}

#[cfg(test)]
mod tests;
