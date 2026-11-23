//! Every project's issues read as the flow declares them, in one graph.
//!
//! An issue's front matter holds its id, title, kind, status and stage,
//! the fields its kind has, and the relations that start from its kind.
//! Anything else is a structural problem, as is an id more than one file
//! holds, a reference to no issue, a target of a kind the relation doesn't
//! allow, and a cycle where the relation forbids one. Ids are global, so a
//! relation may cross projects.
//! The file's parts are as [`super::file`] reads them.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::ToString;
use core::fmt;

use crate::flow::{Field, FieldRole, Flow, Relation};
use crate::issue::{Datum, Issue};
use crate::{IssueId, ProjectId};
use serde::Deserialize;
use toml::Value;

use super::file::{Document, ID, KIND, STAGE, STATUS, TITLE};
use crate::prelude::*;

/// An issue file: the project it's in, its name without `.md`, its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub project: ProjectId,
    pub stem: String,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    /// By id.
    pub issues: Vec<Issue>,
    pub problems: Vec<Problem>,
}

/// Something structurally wrong with an issue, where a field is the cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The project and the stem of the issue's file.
    pub project: ProjectId,
    pub stem: String,
    pub field: Option<String>,
    pub message: String,
}

impl fmt::Display for Problem {
    /// What is wrong, after the field that causes it.
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.field {
            Some(field) => write!(out, "{field}: {}", self.message),
            None => write!(out, "{}", self.message),
        }
    }
}

/// The issues `files` hold, as `flow` declares them, and what's
/// structurally wrong with any of them.
#[must_use]
pub fn read(flow: &Flow, files: &[File]) -> Graph {
    let mut issues = Vec::new();
    let mut problems = Vec::new();
    let mut unread = BTreeSet::new();
    for file in files {
        match one(flow, file) {
            Ok(read_issue) => issues.push(read_issue),
            Err(problem) => {
                unread.insert(file.stem.as_str());
                problems.push(problem);
            }
        }
    }
    issues.sort_by_key(|read_issue| read_issue.id);
    problems.extend(shared(&issues));
    problems.extend(references(flow, &issues, &unread));
    problems.extend(cycles(flow, &issues));
    Graph { issues, problems }
}

/// The issue one file holds, as `flow` declares it, or its structural
/// problem; references to other issues are left to [`read`].
///
/// # Errors
///
/// What is structurally wrong with it.
pub fn one(flow: &Flow, file: &File) -> Result<Issue, Problem> {
    parsed(flow, file).map(|(issue, _)| issue)
}

/// The issue one file holds, as [`one`] reads it, with the document it
/// was read from.
///
/// # Errors
///
/// What is structurally wrong with it.
pub fn parsed(flow: &Flow, file: &File) -> Result<(Issue, Document), Problem> {
    let fail = |field: Option<&str>, message: String| Problem {
        project: file.project,
        stem: file.stem.clone(),
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
    if flow.kind(&kind).is_none() {
        return Err(fail(Some(KIND), format!("no kind is called {kind:?}")));
    }
    let status = front
        .status
        .ok_or_else(|| fail(Some(STATUS), String::from("is missing")))?;
    if flow.status(&status).is_none() {
        return Err(fail(
            Some(STATUS),
            format!("no status is called {status:?}"),
        ));
    }
    let stage = front.stage;
    if let Some(name) = stage.as_ref().filter(|name| flow.stage(name).is_none())
    {
        return Err(fail(Some(STAGE), format!("no stage is called {name:?}")));
    }
    let mut fields = BTreeMap::new();
    let mut links = BTreeMap::new();
    for (key, value) in front.rest {
        if let Some(field) = flow.field(&kind, &key) {
            let datum = datum(field, value)
                .map_err(|message| fail(Some(&key), message))?;
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
        body: document.body_text(),
        sections: document.sections(),
        checklist: document.checklist(),
        log: document.log(),
        branch,
    };
    Ok((issue, document))
}

/// The branch an issue's sessions land on: the one its file declares, else
/// `issue/<id>`.
#[must_use]
pub fn branch_of(id: IssueId, declared: Option<&str>) -> String {
    declared.map_or_else(|| format!("issue/{id}"), String::from)
}

/// The value an issue gives a field, read by serde from its TOML and
/// then checked against the field's type.
fn datum(field: &Field, value: Value) -> Result<Datum, String> {
    let read =
        Datum::deserialize(value).map_err(|err| String::from(err.message()))?;
    super::admitted(&field.value, &read).map(|()| read)
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

/// Each issue whose id another file holds too, naming that file; `issues`
/// are sorted by id, so those sharing one are side by side.
fn shared(issues: &[Issue]) -> Vec<Problem> {
    issues
        .chunk_by(|one, other| one.id == other.id)
        .flat_map(|sharing| {
            sharing.iter().flat_map(move |issue| {
                sharing
                    .iter()
                    .filter(move |other| !core::ptr::eq(*other, issue))
                    .map(move |other| Problem {
                        project: issue.project,
                        stem: issue.id.to_string(),
                        field: Some(String::from(ID)),
                        message: format!(
                            "is also the id of {}/{}",
                            other.project, other.id
                        ),
                    })
            })
        })
        .collect()
}

/// Each reference to no issue, or to one of a kind its relation doesn't
/// allow. An issue whose file has a problem of its own still exists, so a
/// reference to it is left to that problem.
fn references(
    flow: &Flow,
    issues: &[Issue],
    unread: &BTreeSet<&str>,
) -> Vec<Problem> {
    let by_id: BTreeMap<IssueId, &Issue> =
        issues.iter().map(|issue| (issue.id, issue)).collect();
    let mut problems = Vec::new();
    for issue in issues {
        for (name, ids) in &issue.links {
            let allowed = flow
                .relations
                .iter()
                .find(|relation| relation.name == *name)
                .map_or(&[][..], |relation| relation.to.as_slice());
            for id in ids {
                let message = match by_id.get(id) {
                    None if unread.contains(id.to_string().as_str()) => None,
                    None => Some(format!("{id} is no issue")),
                    Some(target)
                        if !allowed.is_empty()
                            && !allowed.contains(&target.kind) =>
                    {
                        Some(format!(
                            "{id} is a {}, which {name} can't point to",
                            target.kind
                        ))
                    }
                    Some(_) => None,
                };
                problems.extend(message.map(|text| Problem {
                    project: issue.project,
                    stem: issue.id.to_string(),
                    field: Some(name.clone()),
                    message: text,
                }));
            }
        }
    }
    problems
}

/// Each issue on a cycle through a relation that forbids cycles, each
/// naming the whole cycle, so an edit to any issue on it sees the problem
/// as its own.
fn cycles(flow: &Flow, issues: &[Issue]) -> Vec<Problem> {
    let projects: BTreeMap<IssueId, ProjectId> = issues
        .iter()
        .map(|issue| (issue.id, issue.project))
        .collect();
    let mut problems = Vec::new();
    for relation in flow.relations.iter().filter(|relation| !relation.cycles) {
        let edges: BTreeMap<IssueId, &[IssueId]> = issues
            .iter()
            .filter_map(|issue| {
                issue
                    .links
                    .get(&relation.name)
                    .map(|ids| (issue.id, ids.as_slice()))
            })
            .collect();
        let mut done: BTreeSet<IssueId> = BTreeSet::new();
        for issue in issues {
            let mut path = Vec::new();
            if let Some(cycle) = walk(&edges, issue.id, &mut path, &mut done) {
                let message = format!(
                    "is part of a cycle: {}",
                    cycle
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<String>>()
                        .join(" → ")
                );
                let members = cycle.iter().skip(1).filter_map(|id| {
                    projects.get(id).map(|project| (*project, id))
                });
                problems.extend(members.map(|(project, id)| Problem {
                    project,
                    stem: id.to_string(),
                    field: Some(relation.name.clone()),
                    message: message.clone(),
                }));
            }
        }
    }
    problems
}

/// A cycle reached from `id`, as the ids around it, unless every issue on
/// the way was already walked.
fn walk(
    edges: &BTreeMap<IssueId, &[IssueId]>,
    id: IssueId,
    path: &mut Vec<IssueId>,
    done: &mut BTreeSet<IssueId>,
) -> Option<Vec<IssueId>> {
    if let Some(start) = path.iter().position(|seen| *seen == id) {
        let mut cycle = path.get(start..)?.to_vec();
        cycle.push(id);
        return Some(cycle);
    }
    if !done.insert(id) {
        return None;
    }
    path.push(id);
    let found = edges
        .get(&id)
        .into_iter()
        .flat_map(|ids| ids.iter())
        .find_map(|next| walk(edges, *next, path, done));
    path.pop();
    found
}

#[cfg(test)]
mod tests;
