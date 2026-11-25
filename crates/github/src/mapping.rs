use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use home::flow::{
    FieldRole, Flow, Severity,
    check::Finding,
    graph::{self, branch_of},
};
use home::issue::{
    Coverage, Datum, Issue, Locator, Observation, Unreadable, body::Body,
};
use home::{IssueId, ProjectId, UnixMillis};
use sha2::{Digest, Sha256};

use crate::configuration::{Configuration, RelationSource};

mod footer;
mod labels;

pub const AUTHORITY: &str = "https://api.github.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: IssueId,
    pub locator: Locator,
    pub title: String,
    pub body: String,
    pub created: UnixMillis,
    pub open: bool,
    pub state_reason: Option<String>,
    pub labels: Vec<String>,
}

impl Record {
    fn fingerprint(&self) -> String {
        fn field(hash: &mut Sha256, text: &str) {
            hash.update((text.len() as u64).to_be_bytes());
            hash.update(text.as_bytes());
        }
        let mut hash = Sha256::new();
        field(&mut hash, &self.id.to_string());
        field(&mut hash, &self.title);
        field(&mut hash, &self.body);
        hash.update(self.created.0.to_be_bytes());
        hash.update([
            u8::from(self.open),
            u8::from(self.state_reason.is_some()),
        ]);
        if let Some(reason) = &self.state_reason {
            field(&mut hash, reason);
        }
        let mut labels: Vec<_> = self.labels.iter().collect();
        labels.sort_unstable();
        for label in labels {
            field(&mut hash, label);
        }
        data_encoding::HEXLOWER.encode(&hash.finalize())
    }
}

#[must_use]
pub fn observe(
    flow: &Flow,
    config: &Configuration,
    project: ProjectId,
    record: Record,
) -> Observation {
    let mut findings = Findings {
        subject: format!("{project}/{}", record.id),
        entries: Vec::new(),
    };
    let labels = labels::Labels::read(config, &record.labels, &mut findings);
    let status = labels.status(&record, flow, config, &mut findings);
    let kind = labels
        .managed
        .get(home::issue::KIND)
        .unwrap_or(&flow.default_kind)
        .clone();
    let stage = labels.managed.get(home::issue::STAGE).cloned();
    let (human, metadata) =
        footer::read(&record.body, flow, config, &mut findings);
    let fields = labels.fields(
        flow,
        &kind,
        config,
        record.created,
        metadata.fields,
        &mut findings,
    );
    let revision = record.fingerprint();
    let branch = branch_of(
        &record.id,
        flow.field_with(&kind, FieldRole::Branch)
            .and_then(|field| fields.get(&field.name))
            .and_then(Datum::text),
    );
    let issue = Issue {
        id: record.id,
        project,
        title: record.title,
        created: Some(record.created),
        kind,
        status,
        stage,
        fields,
        links: metadata.links,
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
    for relation in &config.relations {
        let coverage = match relation.source {
            RelationSource::Metadata => observation.completeness.fields.clone(),
            _ => Coverage::Incomplete {
                reason: String::from(
                    "native GitHub relations have not been read",
                ),
            },
        };
        observation
            .completeness
            .relations
            .insert(relation.name.clone(), coverage);
    }
    observation
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
