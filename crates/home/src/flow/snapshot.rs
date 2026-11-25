use alloc::collections::BTreeMap;
use alloc::string::ToString;

use crate::flow::Flow;
use crate::issue::{Issue, Observation};
use crate::{IssueId, ProjectId};

use super::graph::{Graph, Problem, read};
use crate::prelude::*;
use crate::project::{Listing, Project};

/// The decoded flow, projects and issue observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inputs {
    /// The flow, or why it could not be loaded.
    pub flow: Result<Flow, String>,
    /// The project catalog and its findings.
    pub projects: Listing,
    pub issues: Vec<Result<Observation, Problem>>,
    /// Failures reported by the observation source.
    pub unread: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    flow: Result<Flow, String>,
    listing: Listing,
    records: Vec<Result<Observation, Problem>>,
    unread: Vec<String>,
    graph: Graph,
}

impl Snapshot {
    /// The home `inputs` hold. Its issues are read only when its flow
    /// loads.
    #[must_use]
    pub fn of(inputs: Inputs) -> Self {
        let flow = inputs.flow;
        let graph = flow.as_ref().map_or_else(
            |_| Graph::default(),
            |read_flow| read(read_flow, &issues(&inputs.issues)),
        );
        Self {
            flow,
            listing: inputs.projects,
            records: inputs.issues,
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
            records: Vec::new(),
            unread: Vec::new(),
            graph: Graph::default(),
        }
    }

    #[must_use]
    pub fn with(&self, observation: Observation) -> Self {
        let issue = observation.issue();
        let mut records: Vec<Result<Observation, Problem>> = self
            .records
            .iter()
            .filter(|record| match record {
                Ok(other) => {
                    other.issue().project != issue.project
                        || other.issue().id != issue.id
                }
                Err(problem) => {
                    problem.project != issue.project
                        || problem.record != issue.id.to_string()
                }
            })
            .cloned()
            .collect();
        records.push(Ok(observation));
        let graph = self.flow.as_ref().map_or_else(
            |_| Graph::default(),
            |flow| read(flow, &issues(&records)),
        );
        Self {
            records,
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
        let id = ProjectId::parse(name);
        self.projects()
            .iter()
            .find(|project| project.name == name || Some(project.id) == id)
    }

    #[must_use]
    pub fn records(&self) -> &[Result<Observation, Problem>] {
        &self.records
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
    pub fn observation(&self, id: IssueId) -> Option<&Observation> {
        self.issue(id)?;
        self.records
            .iter()
            .filter_map(|record| record.as_ref().ok())
            .find(|observation| observation.issue().id == id)
    }

    /// Its issues, by project and id.
    #[must_use]
    pub fn issues(&self) -> BTreeMap<ProjectId, Vec<Issue>> {
        let mut issues: BTreeMap<ProjectId, Vec<Issue>> = BTreeMap::new();
        for issue in &self.graph.issues {
            issues.entry(issue.project).or_default().push(issue.clone());
        }
        issues
    }

    /// The project folders that aren't projects, and why.
    #[must_use]
    pub fn listing_problems(&self) -> &[String] {
        &self.listing.problems
    }

    /// Failures reported by the observation source.
    #[must_use]
    pub fn unread(&self) -> &[String] {
        &self.unread
    }

    /// Flow, project and issue observation problems, each naming its subject.
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
                format!("{}/{}: {problem}", problem.project, problem.record)
            }))
            .collect()
    }
}

#[cfg(test)]
mod tests;

fn issues(
    records: &[Result<Observation, Problem>],
) -> Vec<Result<Issue, Problem>> {
    records
        .iter()
        .map(|record| {
            record
                .as_ref()
                .map(|observation| observation.issue().clone())
                .map_err(Clone::clone)
        })
        .collect()
}
