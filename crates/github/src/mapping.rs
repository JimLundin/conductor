use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::fmt;
use home::flow::{
    FieldRole, Flow, Severity,
    check::Finding,
    graph::{self, branch_of},
};
use home::issue::{
    Coverage, Datum, Issue, IssueState, Locator, NativeIssue, Observation,
    Unreadable, body::Body,
};
use home::tracker::Ids;
use home::{IssueId, IssueKey, ProjectId, UnixMillis};

use crate::IssueNumber;
use crate::access::{Repositories, Repository};
use crate::configuration::Configuration;

mod footer;
mod labels;
pub mod relate;
mod relations;
mod writes;

pub use footer::carries;
pub use writes::{opening, writes};

pub const AUTHORITY: &str = "https://api.github.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: IssueId,
    pub locator: Locator,
    pub created: UnixMillis,
    pub native: NativeIssue,
    pub parent: Option<Native>,
    pub blocked_by: Option<Vec<Native>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Native {
    Here(u64),
    Elsewhere(Repository, u64),
}

impl Native {
    #[must_use]
    pub fn read(text: &str) -> Option<Self> {
        Some(match text.split_once('#') {
            Some((repository, number)) => Self::Elsewhere(
                Repository::parse(repository).ok()?,
                number.parse().ok()?,
            ),
            None => Self::Here(text.parse().ok()?),
        })
    }

    #[must_use]
    pub fn key(
        &self,
        here: ProjectId,
        repositories: &Repositories,
    ) -> Option<IssueKey> {
        let (project, number) = match self {
            Self::Here(number) => (here, *number),
            Self::Elsewhere(repository, number) => {
                (repositories.project(repository)?, *number)
            }
        };
        Some(IssueKey {
            project,
            id: IssueId::of(&IssueNumber::new(number)?),
        })
    }
}

impl fmt::Display for Native {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Here(number) => write!(out, "{number}"),
            Self::Elsewhere(repository, number) => {
                write!(out, "{repository}#{number}")
            }
        }
    }
}

#[must_use]
pub fn observe(
    flow: &Flow,
    config: &Configuration,
    ids: &Ids,
    repositories: &Repositories,
    project: ProjectId,
    record: Record,
) -> Observation {
    let mut findings = Findings {
        subject: format!("{project}/{}", record.id),
        entries: Vec::new(),
    };
    let labels =
        labels::Labels::read(config, &record.native.labels, &mut findings);
    let status = labels.status(&record.native, flow, config, &mut findings);
    let kind = labels
        .managed
        .get(home::issue::KIND)
        .unwrap_or(&flow.default_kind)
        .clone();
    let stage = labels.managed.get(home::issue::STAGE).cloned();
    let (human, metadata) = footer::read(
        &record.native.body,
        flow,
        config,
        ids,
        project,
        &mut findings,
    );
    let fields = labels.fields(
        flow,
        &kind,
        config,
        record.created,
        metadata.fields,
        &mut findings,
    );
    let relations = relations::Relations::read(
        flow,
        repositories,
        project,
        &record,
        metadata.links,
        &mut findings,
    );
    let revision = record.native.revision();
    let branch = branch_of(
        &record.id,
        flow.field_with(&kind, FieldRole::Branch)
            .and_then(|field| fields.get(&field.name))
            .and_then(Datum::text),
    );
    let issue = Issue {
        id: record.id,
        project,
        title: record.native.title.clone(),
        created: Some(record.created),
        kind,
        status,
        stage,
        fields,
        links: relations.links,
        branch,
        body: String::new(),
        sections: Vec::new(),
        checklist: Vec::new(),
        notes: Vec::new(),
        canvases: Vec::new(),
        log: Vec::new(),
    };
    if let Err(problem) = graph::validate(flow, &issue) {
        findings.error(problem.to_string());
    }
    let mut observation = findings.observation(issue, human);
    observation.locator = Some(record.locator);
    observation.revision = Some(revision);
    observation.completeness.notes = Coverage::Incomplete {
        reason: String::from("GitHub comments have not been read"),
    };
    for (name, read) in relations.coverage {
        let coverage = match &observation.completeness.fields {
            Coverage::Complete => read,
            unread @ Coverage::Incomplete { .. } => unread.clone(),
        };
        observation.completeness.relations.insert(name, coverage);
    }
    observation
}

pub fn logged(observation: &mut Observation, posted: Vec<crate::log::Posted>) {
    let log = crate::log::read(posted);
    if let IssueState::Valid { issue, .. } = &mut observation.state {
        issue.log = log.entries;
        issue.notes = log.notes;
        observation.completeness.notes = Coverage::Complete;
    }
}

struct Findings {
    subject: String,
    entries: Vec<Finding>,
}

impl Findings {
    fn error(&mut self, message: String) {
        self.push(Severity::Error, message);
    }

    fn warn(&mut self, message: String) {
        self.push(Severity::Warn, message);
    }

    fn push(&mut self, severity: Severity, message: String) {
        self.entries.push(Finding {
            severity,
            message,
            subject: Some(self.subject.clone()),
        });
    }

    fn observation(self, issue: Issue, human: &str) -> Observation {
        let mut observation = if let Some(first) = self
            .entries
            .iter()
            .find(|finding| finding.severity == Severity::Error)
        {
            Observation::invalid(Unreadable {
                id: issue.id,
                project: issue.project,
                title: Some(issue.title),
                body: Some(String::from(human)),
                created: issue.created,
                reason: first.message.clone(),
            })
        } else {
            Observation::mapped(issue, Body::of(human))
        };
        observation.findings = self.entries;
        observation
    }
}

#[cfg(test)]
mod tests;
