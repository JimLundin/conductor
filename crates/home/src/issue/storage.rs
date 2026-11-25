use super::document::Document;
use super::{ID, KIND, STAGE, STATUS, TITLE};
use crate::flow::graph::{Problem, branch_of};
use crate::flow::snapshot::{Inputs as DomainInputs, Snapshot};
use crate::flow::{FieldRole, Flow, Relation};
use crate::issue::{Datum, Issue, Observation};
use crate::prelude::*;
use crate::{IssueId, ProjectId};
use alloc::collections::BTreeMap;
use alloc::string::ToString;
use serde::Deserialize;
use toml::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub project: ProjectId,
    pub stem: String,
    pub text: String,
}

/// Reads one issue document through the domain model and body interpreter.
///
/// # Errors
/// The document cannot be decoded or its issue values fail the flow.
pub fn decode(
    flow: &Flow,
    file: &File,
) -> Result<(Observation, Document), Problem> {
    let fail = |field: Option<&str>, message: String| Problem {
        project: file.project,
        record: file.stem.clone(),
        field: field.map(ToOwned::to_owned),
        message,
    };
    let document =
        Document::of(&file.text).map_err(|message| fail(None, message))?;
    let front = document
        .front_matter()
        .map_err(|message| fail(None, message))?;
    let id = front
        .id
        .filter(|id| *id == file.stem)
        .and_then(|id| IssueId::parse(&id))
        .ok_or_else(|| {
            fail(
                Some(ID),
                String::from("is not its file's name, an issue id"),
            )
        })?;
    let title = front
        .title
        .ok_or_else(|| fail(Some(TITLE), String::from("is missing")))?;
    let kind = front.kind.unwrap_or_else(|| flow.default_kind.clone());
    let status = front
        .status
        .ok_or_else(|| fail(Some(STATUS), String::from("is missing")))?;
    let stage = front.stage;
    let mut fields = BTreeMap::new();
    let mut links = BTreeMap::new();
    for (key, value) in front.rest {
        if flow.field(&kind, &key).is_some() {
            let datum =
                datum(value).map_err(|message| fail(Some(&key), message))?;
            fields.insert(key, datum);
        } else if let Some(relation) = flow.relations.iter().find(|relation| {
            relation.name == key
                && (relation.from.is_empty() || relation.from.contains(&kind))
        }) {
            let ids = targets(relation, value)
                .map_err(|message| fail(Some(&key), message))?;
            links.insert(key, ids);
        } else {
            return Err(fail(
                Some(&key),
                format!("is not a field or relation of {kind:?}"),
            ));
        }
    }
    let branch = branch_of(
        id,
        flow.field_with(&kind, FieldRole::Branch)
            .and_then(|field| fields.get(&field.name))
            .and_then(Datum::text),
    );
    let issue = Issue {
        id,
        project: file.project,
        title,
        kind,
        status,
        stage,
        fields,
        links,
        body: String::new(),
        sections: Vec::new(),
        checklist: Vec::new(),
        log: Vec::new(),
        branch,
    };
    crate::flow::graph::validate(flow, &issue)?;
    Ok((Observation::new(issue, document.body.clone()), document))
}

fn datum(value: Value) -> Result<Datum, String> {
    let read =
        Datum::deserialize(value).map_err(|err| String::from(err.message()))?;
    Ok(read)
}

fn targets(relation: &Relation, value: Value) -> Result<Vec<IssueId>, String> {
    let id = |value: Value| match value {
        Value::String(name) => {
            IssueId::parse(&name).ok_or_else(|| format!("{name} is no issue"))
        }
        _ => Err(String::from("is not a list of ids")),
    };
    match (relation.many, value) {
        (_, Value::String(name)) => {
            id(Value::String(name)).map(|one| Vec::from([one]))
        }
        (true, Value::Array(values)) => values.into_iter().map(id).collect(),
        (false, Value::Array(_)) => {
            Err(String::from("points to one issue, not a list"))
        }
        _ => Err(String::from("is not an id")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inputs {
    pub flow: Result<String, String>,
    pub projects: Vec<(String, Result<String, String>)>,
    pub issues: Vec<File>,
    pub unread: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadHome {
    pub snapshot: Snapshot,
    documents: Vec<(ProjectId, IssueId, Document)>,
}

impl Inputs {
    #[must_use]
    pub fn read(self) -> ReadHome {
        let flow = self
            .flow
            .and_then(|text| crate::flow::load(&text))
            .map_err(|problem| format!("{}: {problem}", crate::flow::PATH));
        let mut documents = Vec::new();
        let issues = flow.as_ref().map_or_else(
            |_| Vec::new(),
            |declared| {
                self.issues
                    .iter()
                    .map(|file| {
                        decode(declared, file).map(|(issue, document)| {
                            documents.push((
                                issue.issue().project,
                                issue.issue().id,
                                document,
                            ));
                            issue
                        })
                    })
                    .collect()
            },
        );
        ReadHome {
            snapshot: Snapshot::of(DomainInputs {
                flow,
                projects: crate::project::listed(self.projects),
                issues,
                unread: self.unread,
            }),
            documents,
        }
    }
}

impl ReadHome {
    /// Encodes a planned change using its previously read document.
    ///
    /// # Errors
    /// The flow or the existing issue document is unavailable or invalid.
    pub fn encode(
        &self,
        change: &crate::flow::command::Change,
    ) -> Result<File, String> {
        let document = self
            .documents
            .iter()
            .find(|(project, id, _)| {
                *project == change.observation.issue().project
                    && *id == change.observation.issue().id
            })
            .map(|(_, _, document)| document.clone());
        encode(self.snapshot.flow()?, change, document, None)
    }
}

#[cfg(test)]
pub(crate) fn snapshot(inputs: Inputs) -> Snapshot {
    inputs.read().snapshot
}

#[cfg(test)]
pub(crate) fn one(flow: &Flow, file: &File) -> Result<Issue, Problem> {
    decode(flow, file).map(|(observation, _)| observation.issue().clone())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub observed: super::Observed,
    pub document: Option<Document>,
}

#[must_use]
pub fn load(flow: &Flow, target: &super::Read, text: &str) -> Loaded {
    let decoded = decode(
        flow,
        &File {
            project: target.project,
            stem: target.issue.to_string(),
            text: String::from(text),
        },
    );
    match decoded {
        Ok((issue, document)) => Loaded {
            observed: super::Observed {
                reading: Ok(issue),
                applied: document
                    .comments()
                    .iter()
                    .filter_map(|comment| {
                        comment.strip_prefix("conductor-edit: ")
                    })
                    .filter_map(super::EditId::parse)
                    .collect(),
            },
            document: Some(document),
        },
        Err(problem) => Loaded {
            observed: super::Observed {
                reading: Err(problem.to_string()),
                applied: Vec::new(),
            },
            document: None,
        },
    }
}

impl File {
    #[must_use]
    pub fn path(&self) -> String {
        crate::layout::issue_file(&self.project, &self.stem)
    }

    #[must_use]
    pub fn folder(&self) -> String {
        crate::layout::issues(&self.project)
    }
}

#[must_use]
pub fn path(target: &super::Read) -> String {
    crate::layout::issue_file(&target.project, &target.issue.to_string())
}

/// Encodes a semantic change while preserving the document front matter.
///
/// # Errors
/// A changed issue has no document, or its front matter cannot be updated.
pub fn encode(
    flow: &Flow,
    change: &crate::flow::command::Change,
    existing: Option<Document>,
    marker: Option<super::EditId>,
) -> Result<File, String> {
    let issue = change.observation.issue();
    let mut document = if change.created {
        opened(flow, change)
    } else {
        let mut document = existing
            .ok_or_else(|| String::from("the issue document is missing"))?;
        for mutation in &change.mutations {
            update(&mut document, mutation)?;
        }
        document
    };
    document.body = change.observation.body().clone();
    if let Some(id) = marker {
        document.comment(&edit_marker(id));
    }
    Ok(File {
        project: issue.project,
        stem: issue.id.to_string(),
        text: document.text(),
    })
}

fn opened(flow: &Flow, change: &crate::flow::command::Change) -> Document {
    use crate::flow::command::Mutation;
    let issue = change.observation.issue();
    let mut front = Vec::from([
        (String::from(ID), quoted(&issue.id.to_string())),
        (String::from(TITLE), quoted(&issue.title)),
        (String::from(KIND), quoted(&issue.kind)),
        (String::from(STATUS), quoted(&issue.status)),
    ]);
    let created = flow.field_with(&issue.kind, FieldRole::Created);
    front.extend(change.mutations.iter().filter_map(
        |mutation| match mutation {
            Mutation::Set { field, value } => {
                let encoded = if created
                    .is_some_and(|declared| declared.name == *field)
                {
                    crate::flow::command::said(value)
                } else {
                    toml_of(value)
                };
                Some((field.clone(), encoded))
            }
            _ => None,
        },
    ));
    Document::new(&front, change.observation.body().clone())
}

fn update(
    document: &mut Document,
    mutation: &crate::flow::command::Mutation,
) -> Result<(), String> {
    use super::Part;
    use crate::flow::command::Mutation;
    let unread = || String::from("its document doesn't read");
    match mutation {
        Mutation::Set { field, value } => {
            document.set(field, &toml_of(value)).ok_or_else(unread)?;
        }
        Mutation::Title(title) => {
            document.set(TITLE, &quoted(title)).ok_or_else(unread)?;
        }
        Mutation::Move { part, to } => document
            .set(
                match part {
                    Part::Stage => STAGE,
                    Part::Status => STATUS,
                },
                &quoted(to),
            )
            .ok_or_else(unread)?,
        Mutation::Body(_) | Mutation::Note(_) | Mutation::Checklist(_) => {}
    }
    Ok(())
}

fn quoted(text: &str) -> String {
    Value::String(String::from(text)).to_string()
}

fn toml_of(value: &Datum) -> String {
    match value {
        Datum::Text(text) => quoted(text),
        Datum::Texts(texts) => Value::Array(
            texts
                .iter()
                .map(|text| Value::String(text.clone()))
                .collect(),
        )
        .to_string(),
        Datum::Bool(flag) => Value::Boolean(*flag).to_string(),
    }
}

fn edit_marker(id: super::EditId) -> String {
    format!("conductor-edit: {id}")
}

pub struct Listing {
    pub path: String,
}

impl Listing {
    #[must_use]
    pub fn creation_pattern(id: super::EditId) -> String {
        format!("^{}$", super::document::comment_line(&edit_marker(id)))
    }

    #[must_use]
    pub fn created_from_path(path: &str) -> Option<IssueId> {
        crate::layout::issue_stem(path).and_then(IssueId::parse)
    }
}
