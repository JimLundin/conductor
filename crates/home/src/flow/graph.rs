use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::ToString;
use core::fmt;

use crate::flow::Flow;
use crate::issue::Issue;
use crate::{IssueId, ProjectId};

use crate::issue::{ID, KIND, STAGE, STATUS};
use crate::prelude::*;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    pub issues: Vec<Issue>,
    pub problems: Vec<Problem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub project: ProjectId,
    pub record: String,
    pub field: Option<String>,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.field {
            Some(field) => write!(out, "{field}: {}", self.message),
            None => write!(out, "{}", self.message),
        }
    }
}

#[must_use]
pub fn read(flow: &Flow, records: &[Result<Issue, Problem>]) -> Graph {
    observed(flow, records, true)
}

pub(super) fn observed(
    flow: &Flow,
    records: &[Result<Issue, Problem>],
    complete_catalog: bool,
) -> Graph {
    let mut issues = Vec::new();
    let mut problems = Vec::new();
    let mut unread = BTreeSet::new();
    for record in records {
        match record {
            Ok(issue) => match validate(flow, issue) {
                Ok(()) => issues.push(issue.clone()),
                Err(problem) => {
                    unread.insert(problem.record.clone());
                    problems.push(problem);
                }
            },
            Err(problem) => {
                unread.insert(problem.record.clone());
                problems.push(problem.clone());
            }
        }
    }
    issues.sort_by_key(|issue| issue.id.clone());
    problems.extend(shared(&issues));
    problems.extend(references(flow, &issues, &unread, complete_catalog));
    problems.extend(cycles(flow, &issues));
    issues.sort_by(|one, other| {
        one.created
            .is_none()
            .cmp(&other.created.is_none())
            .then_with(|| one.created.cmp(&other.created))
            .then_with(|| one.id.cmp(&other.id))
    });
    Graph { issues, problems }
}

pub fn validate(flow: &Flow, issue: &Issue) -> Result<(), Problem> {
    let fail = |field: &str, message: String| Problem {
        project: issue.project,
        record: issue.id.to_string(),
        field: Some(String::from(field)),
        message,
    };
    if flow.kind(&issue.kind).is_none() {
        return Err(fail(KIND, format!("no kind is called {:?}", issue.kind)));
    }
    if flow.status(&issue.status).is_none() {
        return Err(fail(
            STATUS,
            format!("no status is called {:?}", issue.status),
        ));
    }
    if let Some(stage) = issue
        .stage
        .as_ref()
        .filter(|stage| flow.stage(stage).is_none())
    {
        return Err(fail(STAGE, format!("no stage is called {stage:?}")));
    }
    for (key, value) in &issue.fields {
        let field = flow.field(&issue.kind, key).ok_or_else(|| {
            fail(
                key,
                format!("is not a field or relation of {:?}", issue.kind),
            )
        })?;
        super::admitted(&field.value, value)
            .map_err(|message| fail(key, message))?;
    }
    for (key, targets) in &issue.links {
        let relation = flow
            .relations
            .iter()
            .find(|relation| {
                relation.name == *key
                    && (relation.from.is_empty()
                        || relation.from.contains(&issue.kind))
            })
            .ok_or_else(|| {
                fail(
                    key,
                    format!("is not a field or relation of {:?}", issue.kind),
                )
            })?;
        if !relation.many && targets.len() != 1 {
            return Err(fail(
                key,
                String::from("points to one issue, not a list"),
            ));
        }
    }
    Ok(())
}

#[must_use]
pub fn branch_of(id: &IssueId, declared: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    declared.map_or_else(
        || {
            let digest = Sha256::digest(id.to_string().as_bytes());
            format!("issue/{}", data_encoding::HEXLOWER.encode(&digest))
        },
        String::from,
    )
}

fn shared(issues_sorted_by_id: &[Issue]) -> Vec<Problem> {
    issues_sorted_by_id
        .chunk_by(|one, other| one.id == other.id)
        .flat_map(|sharing| {
            sharing.iter().flat_map(move |issue| {
                sharing
                    .iter()
                    .filter(move |other| !core::ptr::eq(*other, issue))
                    .map(move |other| Problem {
                        project: issue.project,
                        record: issue.id.to_string(),
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

fn references(
    flow: &Flow,
    issues: &[Issue],
    unread: &BTreeSet<String>,
    complete_catalog: bool,
) -> Vec<Problem> {
    let by_id: BTreeMap<IssueId, &Issue> = issues
        .iter()
        .map(|issue| (issue.id.clone(), issue))
        .collect();
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
                    None if !complete_catalog
                        || unread.contains(id.to_string().as_str()) =>
                    {
                        None
                    }
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
                    record: issue.id.to_string(),
                    field: Some(name.clone()),
                    message: text,
                }));
            }
        }
    }
    problems
}

fn cycles(flow: &Flow, issues: &[Issue]) -> Vec<Problem> {
    let projects: BTreeMap<IssueId, ProjectId> = issues
        .iter()
        .map(|issue| (issue.id.clone(), issue.project))
        .collect();
    let mut problems = Vec::new();
    for relation in flow.relations.iter().filter(|relation| !relation.cycles) {
        let edges: BTreeMap<IssueId, &[IssueId]> = issues
            .iter()
            .filter_map(|issue| {
                issue
                    .links
                    .get(&relation.name)
                    .map(|ids| (issue.id.clone(), ids.as_slice()))
            })
            .collect();
        let mut done: BTreeSet<IssueId> = BTreeSet::new();
        for issue in issues {
            let mut path = Vec::new();
            if let Some(cycle) =
                walk(&edges, issue.id.clone(), &mut path, &mut done)
            {
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
                    record: id.to_string(),
                    field: Some(relation.name.clone()),
                    message: message.clone(),
                }));
            }
        }
    }
    problems
}

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
    if !done.insert(id.clone()) {
        return None;
    }
    path.push(id.clone());
    let found = edges
        .get(&id)
        .into_iter()
        .flat_map(|ids| ids.iter())
        .find_map(|next| walk(edges, next.clone(), path, done));
    path.pop();
    found
}

#[cfg(test)]
mod tests;
