use super::{Issue, body::Body};
use crate::flow::check::Finding;
use crate::prelude::*;
use crate::{IssueId, ProjectId};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::ToString;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Locator {
    pub repository: String,
    pub reference: String,
    pub url: Option<String>,
    pub addresses: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Coverage {
    Complete,
    Incomplete { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Completeness {
    pub fields: Coverage,
    pub notes: Coverage,
    pub relations: BTreeMap<String, Coverage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Unreadable {
    pub id: IssueId,
    pub project: ProjectId,
    pub title: Option<String>,
    pub body: Option<String>,
    pub created: Option<u64>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum IssueState {
    Valid { issue: Box<Issue>, body: Body },
    Invalid(Unreadable),
    Unavailable(Unreadable),
    Deleted(Unreadable),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Observation {
    pub state: IssueState,
    pub locator: Option<Locator>,
    pub observed_at: Option<u64>,
    pub revision: Option<String>,
    pub completeness: Completeness,
    pub findings: Vec<Finding>,
    pub flow_findings: Vec<Finding>,
}

impl Observation {
    #[must_use]
    pub fn mapped(mut issue: Issue, body: Body) -> Self {
        issue.body = body.body_text();
        issue.sections = body.sections();
        issue.checklist = body.checklist();
        let relations = issue
            .links
            .keys()
            .map(|name| (name.clone(), Coverage::Complete))
            .collect();
        Self {
            state: IssueState::Valid {
                issue: Box::new(issue),
                body,
            },
            locator: None,
            observed_at: None,
            revision: None,
            completeness: Completeness {
                fields: Coverage::Complete,
                notes: Coverage::Complete,
                relations,
            },
            findings: Vec::new(),
            flow_findings: Vec::new(),
        }
    }

    #[must_use]
    pub fn id(&self) -> &IssueId {
        match &self.state {
            IssueState::Valid { issue, .. } => &issue.id,
            IssueState::Invalid(record)
            | IssueState::Unavailable(record)
            | IssueState::Deleted(record) => &record.id,
        }
    }

    #[must_use]
    pub fn project(&self) -> ProjectId {
        match &self.state {
            IssueState::Valid { issue, .. } => issue.project,
            IssueState::Invalid(record)
            | IssueState::Unavailable(record)
            | IssueState::Deleted(record) => record.project,
        }
    }

    #[must_use]
    pub fn issue(&self) -> Option<&Issue> {
        match &self.state {
            IssueState::Valid { issue, .. } => Some(issue),
            _ => None,
        }
    }

    #[must_use]
    pub const fn body(&self) -> Option<&Body> {
        match &self.state {
            IssueState::Valid { body, .. } => Some(body),
            _ => None,
        }
    }

    pub fn editable(&self) -> Result<(&Issue, &Body), String> {
        match &self.state {
            IssueState::Valid { issue, body } => Ok((issue, body)),
            IssueState::Invalid(record)
            | IssueState::Unavailable(record)
            | IssueState::Deleted(record) => Err(record.reason.clone()),
        }
    }

    #[must_use]
    pub fn created(&self) -> Option<u64> {
        match &self.state {
            IssueState::Valid { issue, .. } => issue.created,
            IssueState::Invalid(record)
            | IssueState::Unavailable(record)
            | IssueState::Deleted(record) => record.created,
        }
    }

    #[must_use]
    pub fn invalid(record: Unreadable) -> Self {
        let reason = record.reason.clone();
        Self {
            state: IssueState::Invalid(record),
            locator: None,
            observed_at: None,
            revision: None,
            completeness: Completeness {
                fields: Coverage::Incomplete { reason },
                notes: Coverage::Incomplete {
                    reason: String::from("notes were not read"),
                },
                relations: BTreeMap::new(),
            },
            findings: Vec::new(),
            flow_findings: Vec::new(),
        }
    }

    #[must_use]
    pub fn problem(&self) -> Option<crate::flow::graph::Problem> {
        self.editable()
            .err()
            .map(|message| crate::flow::graph::Problem {
                project: self.project(),
                record: self.id().to_string(),
                field: None,
                message,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocatorConflict {
    ChangedIdentity,
    UnconfiguredRepository,
}

impl Observation {
    pub fn relocated(
        &self,
        returned: &IssueId,
        locator: Locator,
        configured: &[String],
    ) -> Result<Self, LocatorConflict> {
        if self.id() != returned {
            return Err(LocatorConflict::ChangedIdentity);
        }
        if !configured.contains(&locator.repository) {
            return Err(LocatorConflict::UnconfiguredRepository);
        }
        Ok(Self {
            locator: Some(locator),
            ..self.clone()
        })
    }
}
