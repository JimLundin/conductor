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
        let mut records = inputs.issues;
        let graph = flow.as_ref().map_or_else(
            |_| Graph::default(),
            |read_flow| observed(read_flow, &mut records),
        );
        Self {
            flow,
            listing: inputs.projects,
            records,
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
        let id = observation.id();
        let project = observation.project();
        let mut records: Vec<Result<Observation, Problem>> = self
            .records
            .iter()
            .filter(|record| match record {
                Ok(other) => other.project() != project || other.id() != id,
                Err(problem) => {
                    problem.project != project
                        || problem.record != id.to_string()
                }
            })
            .cloned()
            .collect();
        records.push(Ok(observation));
        let graph = self.flow.as_ref().map_or_else(
            |_| Graph::default(),
            |flow| observed(flow, &mut records),
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
    pub fn issue(&self, id: &IssueId) -> Option<&Issue> {
        self.graph.issues.iter().find(|issue| &issue.id == id)
    }

    #[must_use]
    pub fn observation(&self, id: &IssueId) -> Option<&Observation> {
        self.records
            .iter()
            .filter_map(|record| record.as_ref().ok())
            .find(|observation| observation.id() == id)
    }

    /// Its observations, grouped by project and ordered by creation time.
    #[must_use]
    pub fn issues(&self) -> BTreeMap<ProjectId, Vec<Observation>> {
        let mut issues: BTreeMap<ProjectId, Vec<Observation>> = BTreeMap::new();
        for observation in self
            .records
            .iter()
            .filter_map(|record| record.as_ref().ok())
        {
            issues
                .entry(observation.project())
                .or_default()
                .push(observation.clone());
        }
        for records in issues.values_mut() {
            records.sort_by(|one, other| {
                one.created()
                    .is_none()
                    .cmp(&other.created().is_none())
                    .then_with(|| one.created().cmp(&other.created()))
                    .then_with(|| one.id().cmp(other.id()))
            });
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
        .filter(|record| {
            !record.as_ref().is_ok_and(|observation| {
                matches!(
                    &observation.state,
                    crate::issue::IssueState::Deleted(_)
                )
            })
        })
        .map(|record| {
            record
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|observation| {
                    observation
                        .editable()
                        .map(|(issue, _)| issue.clone())
                        .map_err(|message| Problem {
                            project: observation.project(),
                            record: observation.id().to_string(),
                            field: None,
                            message,
                        })
                })
        })
        .collect()
}

fn observed(
    flow: &Flow,
    records: &mut [Result<Observation, Problem>],
) -> Graph {
    for observation in
        records.iter_mut().filter_map(|record| record.as_mut().ok())
    {
        if let Some(issue) = observation.issue()
            && let Err(problem) = super::graph::validate(flow, issue)
        {
            let record = crate::issue::Unreadable {
                id: issue.id.clone(),
                project: issue.project,
                title: Some(issue.title.clone()),
                body: observation.body().map(crate::issue::body::Body::text),
                created: issue.created,
                reason: problem.to_string(),
            };
            observation.state = crate::issue::IssueState::Invalid(record);
        }
    }
    let graph = read(flow, &issues(records));
    let all: Vec<&Issue> = graph.issues.iter().collect();
    for observation in
        records.iter_mut().filter_map(|record| record.as_mut().ok())
    {
        let subject = observation.id().to_string();
        observation.flow_findings = graph
            .problems
            .iter()
            .filter(|problem| {
                problem.record == subject
                    && problem.project == observation.project()
            })
            .map(|problem| super::check::Finding {
                severity: crate::flow::Severity::Error,
                subject: Some(subject.clone()),
                message: problem.to_string(),
            })
            .collect();
        if let Some(issue) = observation.issue() {
            let standing = super::eval::Move {
                from: Some(issue),
                to: Some(issue),
            };
            let findings =
                super::check::of_issue(flow, &all, issue, standing, &subject);
            observation.flow_findings.extend(findings);
        }
    }
    graph
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveIssue {
    ProjectRequired,
    Unknown,
    Ambiguous,
}

impl Snapshot {
    /// # Errors
    /// The reference lacks project context, is ambiguous, or is unknown.
    pub fn resolve(
        &self,
        project: Option<ProjectId>,
        reference: &str,
    ) -> Result<&Observation, ResolveIssue> {
        if let Some(id) = IssueId::parse(reference) {
            return self
                .observation(&id)
                .filter(|observation| {
                    project
                        .is_none_or(|context| context == observation.project())
                })
                .ok_or(ResolveIssue::Unknown);
        }
        let context = project.ok_or(ResolveIssue::ProjectRequired)?;
        let mut found =
            self.records
                .iter()
                .filter_map(|record| record.as_ref().ok())
                .filter(|record| {
                    record.project() == context
                        && record.locator.as_ref().is_some_and(|locator| {
                            locator.reference == reference
                        })
                });
        let issue = found.next().ok_or(ResolveIssue::Unknown)?;
        if found.next().is_some() {
            return Err(ResolveIssue::Ambiguous);
        }
        Ok(issue)
    }
}
