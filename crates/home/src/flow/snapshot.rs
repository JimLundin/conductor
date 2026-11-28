use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::ToString;

use crate::flow::Flow;
use crate::issue::{
    Freshness, Issue, IssueListing, IssueReading, Observation, ReadStatus,
    Unreadable,
};
use crate::{IssueId, IssueKey, ProjectId};

use super::command::of_canvas;
use super::graph::{Graph, Problem};
use crate::canvas::CanvasKey;
use crate::prelude::*;
use crate::project::{Listing, Project};
use crate::tracker::{Ids, Issuers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inputs {
    pub flow: Result<Flow, String>,
    pub projects: Listing,
    pub issues: Vec<Result<Observation, Problem>>,
    pub unread: Vec<String>,
    pub issuers: Issuers,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    flow: Result<Flow, String>,
    listing: Listing,
    records: Vec<Result<Observation, Problem>>,
    unread: Vec<String>,
    graph: Graph,
    ids: Ids,
    partial: BTreeSet<ProjectId>,
}

impl Snapshot {
    #[must_use]
    pub fn of(inputs: Inputs) -> Self {
        let flow = inputs.flow;
        let mut records = inputs.issues;
        let graph = flow.as_ref().map_or_else(
            |_| Graph::default(),
            |read_flow| observed(read_flow, &mut records, &|_| true),
        );
        let ids = inputs.issuers.ids(&inputs.projects.projects);
        Self {
            flow,
            listing: inputs.projects,
            records,
            unread: inputs.unread,
            graph,
            ids,
            partial: BTreeSet::new(),
        }
    }

    #[must_use]
    pub fn empty(reason: String) -> Self {
        Self {
            flow: Err(reason),
            listing: Listing::default(),
            records: Vec::new(),
            unread: Vec::new(),
            graph: Graph::default(),
            ids: Ids::default(),
            partial: BTreeSet::new(),
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
        self.holding(records, self.partial.clone())
    }

    #[must_use]
    pub fn cataloged(&self, project: ProjectId, listing: IssueListing) -> Self {
        let mut records: Vec<Result<Observation, Problem>> = self
            .records
            .iter()
            .filter(|record| match record {
                Ok(other) => other.project() != project,
                Err(problem) => problem.project != project,
            })
            .cloned()
            .collect();
        records.extend(listing.records.into_iter().map(Ok));
        let mut partial = self.partial.clone();
        if listing.status.complete {
            partial.remove(&project);
        } else {
            partial.insert(project);
        }
        self.holding(records, partial)
    }

    fn holding(
        &self,
        mut records: Vec<Result<Observation, Problem>>,
        partial: BTreeSet<ProjectId>,
    ) -> Self {
        let graph = self.flow.as_ref().map_or_else(
            |_| Graph::default(),
            |flow| {
                observed(flow, &mut records, &|project| {
                    !partial.contains(&project)
                })
            },
        );
        Self {
            records,
            graph,
            partial,
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
    pub const fn ids(&self) -> &Ids {
        &self.ids
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
    pub fn issue(&self, project: ProjectId, id: &IssueId) -> Option<&Issue> {
        self.graph
            .issues
            .iter()
            .find(|issue| issue.project == project && &issue.id == id)
    }

    #[must_use]
    pub fn observation(
        &self,
        project: ProjectId,
        id: &IssueId,
    ) -> Option<&Observation> {
        self.records
            .iter()
            .filter_map(|record| record.as_ref().ok())
            .find(|observation| {
                observation.project() == project && observation.id() == id
            })
    }

    #[must_use]
    pub fn reading(&self, key: &IssueKey) -> IssueReading {
        let record = self.records.iter().find_map(|record| match record {
            Ok(observation) => (observation.project() == key.project
                && observation.id() == &key.id)
                .then(|| observation.clone()),
            Err(problem) => (problem.project == key.project
                && problem.record == key.id.to_string())
            .then(|| {
                Observation::invalid(Unreadable {
                    id: key.id.clone(),
                    project: key.project,
                    title: None,
                    body: None,
                    created: None,
                    reason: problem.to_string(),
                })
            }),
        });
        let status = ReadStatus {
            freshness: if record.is_some() {
                Freshness::Fresh
            } else {
                Freshness::Unknown
            },
            complete: record.is_some(),
            observed_at: record
                .as_ref()
                .and_then(|observation| observation.observed_at),
            failure: None,
        };
        IssueReading { record, status }
    }

    #[must_use]
    pub fn listed(&self, project: ProjectId) -> IssueListing {
        let mut records: Vec<Observation> = self
            .records
            .iter()
            .filter_map(|record| match record {
                Ok(observation) => (observation.project() == project)
                    .then(|| observation.clone()),
                Err(problem) => (problem.project == project)
                    .then(|| self.ids.read(project, &problem.record).ok())
                    .flatten()
                    .map(|id| {
                        Observation::invalid(Unreadable {
                            id,
                            project,
                            title: None,
                            body: None,
                            created: None,
                            reason: problem.to_string(),
                        })
                    }),
            })
            .collect();
        records.sort_by(creation_order);
        IssueListing {
            records,
            status: ReadStatus {
                freshness: Freshness::Fresh,
                complete: true,
                observed_at: None,
                failure: None,
            },
        }
    }

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
            records.sort_by(creation_order);
        }
        issues
    }

    #[must_use]
    pub fn canvased(&self, kept: &[CanvasKey]) -> BTreeSet<IssueKey> {
        self.graph()
            .issues
            .iter()
            .map(|record| IssueKey {
                project: record.project,
                id: record.id.clone(),
            })
            .chain(kept.iter().map(of_canvas))
            .collect()
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
                format!("{} {}: {problem}", problem.project, problem.record)
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

pub(crate) fn project_issues(
    flow: &Flow,
    records: Vec<Observation>,
) -> Vec<Observation> {
    let applicable = super::coverage::applicable(flow, &records);
    let mut inputs: Vec<_> = records.into_iter().map(Ok).collect();
    observed(&applicable, &mut inputs, &|_| false);
    let mut evaluated: Vec<_> = inputs.into_iter().flatten().collect();
    evaluated.sort_by(creation_order);
    evaluated
}

fn creation_order(
    one: &Observation,
    other: &Observation,
) -> core::cmp::Ordering {
    one.created()
        .is_none()
        .cmp(&other.created().is_none())
        .then_with(|| one.created().cmp(&other.created()))
        .then_with(|| one.id().cmp(other.id()))
}

fn observed(
    flow: &Flow,
    records: &mut [Result<Observation, Problem>],
    held: &dyn Fn(ProjectId) -> bool,
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
    let graph = super::graph::observed(flow, &issues(records), held);
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
