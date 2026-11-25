use super::document::{Document, targets};
use super::{ID, KIND, STAGE, STATUS, TITLE};
use crate::flow::graph::{Problem, branch_of};
use crate::flow::snapshot::{Inputs as DomainInputs, Snapshot};
use crate::flow::{FieldRole, Flow};
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
        &id,
        flow.field_with(&kind, FieldRole::Branch)
            .and_then(|field| fields.get(&field.name))
            .and_then(Datum::text),
    );
    let created = flow
        .field_with(&kind, FieldRole::Created)
        .and_then(|field| {
            fields
                .get(&field.name)
                .map(|value| (field.name.as_str(), value))
        })
        .map(|(name, value)| {
            value.text().and_then(created).ok_or_else(|| {
                fail(Some(name), String::from("is not a valid creation time"))
            })
        })
        .transpose()?;
    let issue = Issue {
        id,
        project: file.project,
        created,
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
        notes: Vec::new(),
        branch,
    };
    crate::flow::graph::validate(flow, &issue)?;
    Ok((observation(issue, document.body.clone()), document))
}

fn datum(value: Value) -> Result<Datum, String> {
    let read =
        Datum::deserialize(value).map_err(|err| String::from(err.message()))?;
    Ok(read)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inputs {
    pub flow: Result<String, String>,
    pub projects: Vec<(String, Result<String, String>)>,
    pub issues: Vec<File>,
    pub unavailable: Vec<super::Unreadable>,
    pub observed_at: Option<u64>,
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
        let mut issues: Vec<Result<Observation, Problem>> =
            flow.as_ref().map_or_else(
                |_| Vec::new(),
                |declared| {
                    self.issues
                        .iter()
                        .map(|file| {
                            decode(declared, file)
                                .map(|(issue, document)| {
                                    documents.push((
                                        issue.project(),
                                        issue.id().clone(),
                                        document,
                                    ));
                                    issue
                                })
                                .or_else(|problem| invalid(file, problem))
                        })
                        .collect()
                },
            );
        issues.extend(self.unavailable.into_iter().map(|record| {
            let mut observation = Observation::invalid(record.clone());
            observation.state = super::IssueState::Unavailable(record);
            Ok(observation)
        }));
        for observation in
            issues.iter_mut().filter_map(|record| record.as_mut().ok())
        {
            observation.observed_at = self.observed_at;
        }
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
    pub fn encode(
        &self,
        change: &crate::flow::command::Change,
    ) -> Result<File, String> {
        if self.snapshot.projects().iter().any(|project| {
            project.id == change.observation.project()
                && project.backends.tracker.is_some()
        }) {
            return Err(String::from(
                "edit this issue in the configured tracker; local issue writes are disabled for this project",
            ));
        }
        let document = self
            .documents
            .iter()
            .find(|(project, id, _)| {
                *project == change.observation.project()
                    && *id == *change.observation.id()
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
    decode(flow, file).and_then(|(observation, _)| {
        observation.issue().cloned().ok_or_else(|| {
            observation.problem().unwrap_or_else(|| Problem {
                project: file.project,
                record: file.stem.clone(),
                field: None,
                message: String::from("invalid"),
            })
        })
    })
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

pub fn encode(
    flow: &Flow,
    change: &crate::flow::command::Change,
    existing: Option<Document>,
    marker: Option<super::EditId>,
) -> Result<File, String> {
    let (issue, body) = change.observation.editable()?;
    let mut document = if change.created {
        opened(flow, change)?
    } else {
        let mut document = existing
            .ok_or_else(|| String::from("the issue document is missing"))?;
        for mutation in &change.mutations {
            update(&mut document, mutation)?;
        }
        document
    };
    document.body = body.clone();
    if let Some(id) = marker {
        document.comment(&edit_marker(id));
    }
    Ok(File {
        project: issue.project,
        stem: issue.id.to_string(),
        text: document.text(),
    })
}

fn opened(
    flow: &Flow,
    change: &crate::flow::command::Change,
) -> Result<Document, String> {
    use crate::flow::command::Mutation;
    let (issue, body) = change.observation.editable()?;
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
    Ok(Document::new(&front, body.clone()))
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

const LOCAL_ID_EPOCH_SECS: u64 = 1_767_225_600;

#[must_use]
pub fn fresh(
    home: &Snapshot,
    now_secs: u64,
    draws: &crate::Draws,
) -> Option<IssueId> {
    let seconds = now_secs
        .checked_sub(LOCAL_ID_EPOCH_SECS)
        .filter(|seconds| *seconds < (1 << 30))?;
    draws
        .iter()
        .filter_map(|draw| {
            let value = (seconds << 10) | (draw & 1023);
            let digits = b"0123456789abcdefghjkmnpqrstvwxyz";
            let key: String = (0..8)
                .rev()
                .map(|place| {
                    char::from(
                        digits[usize::try_from((value >> (place * 5)) & 31)
                            .unwrap_or_default()],
                    )
                })
                .collect();
            IssueId::new("local", &key)
        })
        .find(|id| home.observation(id).is_none())
}

fn invalid(file: &File, problem: Problem) -> Result<Observation, Problem> {
    let reason = problem.to_string();
    let id = IssueId::parse(&file.stem).ok_or(problem)?;
    let document = Document::of(&file.text).ok();
    let front = document
        .as_ref()
        .and_then(|parsed| parsed.front_matter().ok());
    let title = front.and_then(|parsed| parsed.title);
    Ok(Observation::invalid(super::Unreadable {
        id,
        project: file.project,
        title,
        body: document.map(|parsed| parsed.body.text()),
        created: None,
        reason,
    }))
}

#[must_use]
pub fn observation(mut issue: Issue, body: super::body::Body) -> Observation {
    issue.log = body.log();
    Observation::mapped(issue, body)
}

fn created(text: &str) -> Option<u64> {
    let at = time::OffsetDateTime::parse(
        text,
        &time::format_description::well_known::Rfc3339,
    )
    .ok()?;
    u64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok()
}

#[cfg(test)]
mod epoch {
    #[test]
    fn local_issue_ids_count_from_the_start_of_2026() {
        let seconds = super::LOCAL_ID_EPOCH_SECS;
        assert_eq!(
            (seconds % 86_400, crate::calendar::civil(seconds / 86_400)),
            (0, (2026, 1, 1))
        );
    }
}
