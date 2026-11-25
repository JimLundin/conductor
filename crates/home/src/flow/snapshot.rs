use alloc::collections::BTreeMap;

use crate::flow::Flow;
use crate::issue::Issue;
use crate::{IssueId, ProjectId};

use super::graph::{File, Graph, read};
use super::{PATH, load};
use crate::prelude::*;
use crate::project::{Listing, Project, listed};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inputs {
    pub flow: Result<String, String>,
    pub projects: Vec<(String, Result<String, String>)>,
    pub issues: Vec<File>,
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

    pub fn flow(&self) -> Result<&Flow, &str> {
        self.flow.as_ref().map_err(String::as_str)
    }

    #[must_use]
    pub fn projects(&self) -> &[Project] {
        &self.listing.projects
    }

    #[must_use]
    pub fn project(&self, name: &str) -> Option<&Project> {
        let id = ProjectId::parse(name);
        self.projects()
            .iter()
            .find(|project| project.name == name || Some(project.id) == id)
    }

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

    #[must_use]
    pub fn issues(&self) -> BTreeMap<ProjectId, Vec<Issue>> {
        let mut issues: BTreeMap<ProjectId, Vec<Issue>> = BTreeMap::new();
        for issue in &self.graph.issues {
            issues.entry(issue.project).or_default().push(issue.clone());
        }
        issues
    }

    #[must_use]
    pub fn listing_problems(&self) -> &[String] {
        &self.listing.problems
    }

    #[must_use]
    pub fn unread(&self) -> &[String] {
        &self.unread
    }

    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        self.flow
            .as_ref()
            .err()
            .cloned()
            .into_iter()
            .chain(self.listing.problems.iter().cloned())
            .chain(self.unread.iter().cloned())
            .chain(self.graph.problems.iter().map(|problem| {
                format!("{}/{}: {problem}", problem.project, problem.stem)
            }))
            .collect()
    }
}

#[cfg(test)]
mod tests;
