use alloc::string::ToString;

use serde::{Deserialize, Serialize};

pub use edit_id::EditId;
pub use model::{
    ChecklistChange, ChecklistItem, Datum, Entry, Issue, IssueChange, Item,
    LogEvent, NewIssue, Part, Section,
};

use crate::flow::Flow;
use crate::flow::command::{self, said};
use crate::flow::file::{Document, checklist_line, log_line};
use crate::flow::graph::{self, File, branch_of};
use crate::flow::snapshot::Snapshot;
use crate::prelude::*;
use crate::{
    CheckoutLocation, Home, HomeRefusal, IssueId, ProjectId, UnixSecs,
};

fn read(
    flow: &Flow,
    project: ProjectId,
    id: IssueId,
    text: &str,
) -> Result<(Issue, Document), String> {
    graph::parsed(
        flow,
        &File {
            project,
            stem: id.to_string(),
            text: String::from(text),
        },
    )
    .map_err(|problem| problem.to_string())
}

#[must_use]
fn message(edit: &command::Edit) -> String {
    match edit {
        command::Edit::New { id, issue } => {
            format!("Open issue {id}: {}", issue.title.trim())
        }
        command::Edit::Change { id, changes } => match changes.as_slice() {
            [change] => change_message(*id, change),
            _ => format!("Change issue {id}"),
        },
    }
}

fn change_message(id: IssueId, change: &IssueChange) -> String {
    match change {
        IssueChange::Move { to, .. } => format!("Move issue {id} to {to}"),
        IssueChange::Set { field, value } => {
            format!("Set the {field} of issue {id} to {}", said(value))
        }
        IssueChange::Title(title) => {
            format!("Retitle issue {id}: {}", title.trim())
        }
        IssueChange::Body(_) => format!("Describe issue {id} anew"),
        IssueChange::Note(_) => format!("Note on issue {id}"),
        IssueChange::Checklist(ChecklistChange::Add(_)) => {
            format!("Add an item to issue {id}")
        }
        IssueChange::Checklist(ChecklistChange::Check {
            done: true, ..
        }) => {
            format!("Check off an item of issue {id}")
        }
        IssueChange::Checklist(ChecklistChange::Check {
            done: false, ..
        }) => {
            format!("Uncheck an item of issue {id}")
        }
        IssueChange::Checklist(ChecklistChange::Remove(_)) => {
            format!("Remove an item from issue {id}")
        }
    }
}

#[must_use]
fn signer(host: &str, client: &str) -> String {
    let word =
        |name: &str| name.split_whitespace().collect::<Vec<_>>().join("-");
    format!("{}·{}", word(host), word(client))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    id: EditId,
    data: EditData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditData {
    project: ProjectId,
    at: UnixSecs,
    by: String,
    edit: command::Edit,
    was: Vec<Option<Vec<String>>>,
}

impl EditData {
    const fn issue(&self) -> IssueId {
        self.edit.id()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Record,
    Replay { changed: UnixSecs },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Applied {
    Write { text: String, warnings: Vec<String> },
    AlreadyRecorded,
    Unchanged,
    Gone,
    Unreadable(String),
    Refused(String),
}

#[must_use]
fn was(
    flow: Option<&Flow>,
    current: Option<&str>,
    edit: &EditData,
) -> Vec<Option<Vec<String>>> {
    let issue = flow
        .zip(current)
        .and_then(|(read_flow, text)| {
            read(read_flow, edit.project, edit.issue(), text).ok()
        })
        .map(|(issue, _)| issue);
    values(issue.as_ref(), &edit.edit)
}

fn values(
    issue: Option<&Issue>,
    edit: &command::Edit,
) -> Vec<Option<Vec<String>>> {
    match edit {
        command::Edit::New { .. } => Vec::new(),
        command::Edit::Change { changes, .. } => changes
            .iter()
            .map(|change| issue.and_then(|had| field(had, change)))
            .collect(),
    }
}

fn field(issue: &Issue, change: &IssueChange) -> Option<Vec<String>> {
    let one = |word: &str| normalized_words(&Datum::Text(String::from(word)));
    match change {
        IssueChange::Move {
            part: Part::Status, ..
        } => Some(one(&issue.status)),
        IssueChange::Move {
            part: Part::Stage, ..
        } => Some(one(issue.stage.as_deref().unwrap_or_default())),
        IssueChange::Set { field, .. } => Some(value_of(issue, field)),
        IssueChange::Title(_) => Some(one(&issue.title)),
        IssueChange::Body(_) => Some(one(&issue.body)),
        IssueChange::Note(_) | IssueChange::Checklist(_) => None,
    }
}

fn value_of(issue: &Issue, field: &str) -> Vec<String> {
    issue.fields.get(field).map_or_else(
        || {
            issue
                .links
                .get(field)
                .into_iter()
                .flatten()
                .map(ToString::to_string)
                .collect()
        },
        normalized_words,
    )
}

fn normalized_words(value: &Datum) -> Vec<String> {
    let kept = |text: &str| {
        let trimmed = text.trim();
        (!trimmed.is_empty()).then(|| String::from(trimmed))
    };
    match value {
        Datum::Text(text) => kept(text).into_iter().collect(),
        Datum::Texts(texts) => {
            texts.iter().filter_map(|text| kept(text)).collect()
        }
        Datum::Bool(flag) => Vec::from([flag.to_string()]),
    }
}

fn overtaken(edit: &EditData, issue: &Issue) -> bool {
    values(Some(issue), &edit.edit)
        .into_iter()
        .enumerate()
        .any(|(at, now)| edit.was.get(at).cloned().flatten() != now)
}

fn unchanged(issue: &Issue, change: &IssueChange) -> bool {
    match change {
        IssueChange::Move {
            part: Part::Status,
            to,
            ..
        } => issue.status == *to,
        IssueChange::Move {
            part: Part::Stage,
            to,
            ..
        } => issue.stage.as_ref() == Some(to),
        IssueChange::Set { field, value } => {
            value_of(issue, field) == normalized_words(value)
        }
        IssueChange::Title(title) => issue.title.trim() == title.trim(),
        IssueChange::Body(body) => issue.body == body.trim(),
        IssueChange::Checklist(ChecklistChange::Check { item, done }) => {
            !stale(issue, item)
                && issue
                    .checklist
                    .get(item.index)
                    .is_some_and(|found| found.done == *done)
        }
        IssueChange::Note(_)
        | IssueChange::Checklist(
            ChecklistChange::Add(_) | ChecklistChange::Remove(_),
        ) => false,
    }
}

fn moot(issue: &Issue, change: &IssueChange) -> bool {
    unchanged(issue, change)
        || matches!(
            change,
            IssueChange::Checklist(
                ChecklistChange::Check { item, .. }
                    | ChecklistChange::Remove(item)
            ) if stale(issue, item)
        )
}

fn stale(issue: &Issue, item: &Item) -> bool {
    item.seen
        .as_ref()
        .is_some_and(|seen| *seen != issue.checklist)
        || item.index >= issue.checklist.len()
}

#[must_use]
fn apply(
    home: &Snapshot,
    current: Option<&str>,
    edit: &Edit,
    mode: Mode,
) -> Applied {
    let data = &edit.data;
    let marker = edit_marker(edit.id);
    let readable = |text: &str| {
        home.flow()
            .map_err(String::from)
            .and_then(|flow| read(flow, data.project, data.issue(), text))
    };
    let (changes, text) = match (&data.edit, current) {
        (command::Edit::New { .. }, Some(text)) => {
            return readable(text).map_or_else(Applied::Unreadable, |_| {
                Applied::AlreadyRecorded
            });
        }
        (command::Edit::New { .. }, None) => {
            return written(home, data, None, &marker);
        }
        (command::Edit::Change { .. }, None) => return Applied::Gone,
        (command::Edit::Change { changes, .. }, Some(text)) => (changes, text),
    };
    let (issue, document) = match readable(text) {
        Ok((_, document)) if document.has_comment(&marker) => {
            return Applied::AlreadyRecorded;
        }
        Ok(read) => read,
        Err(reason) => return Applied::Unreadable(reason),
    };
    match mode {
        Mode::Record
            if changes.iter().all(|change| unchanged(&issue, change)) =>
        {
            Applied::Unchanged
        }
        Mode::Replay { changed }
            if changes.iter().all(|change| moot(&issue, change))
                || (data.at < changed && overtaken(data, &issue)) =>
        {
            marked(document, &marker)
        }
        Mode::Record | Mode::Replay { .. } => {
            written(home, data, Some(text), &marker)
        }
    }
}

fn edit_marker(id: EditId) -> String {
    format!("conductor-edit: {id}")
}

fn marked(mut document: Document, marker: &str) -> Applied {
    document.comment(marker);
    Applied::Write {
        text: document.text(),
        warnings: Vec::new(),
    }
}

fn written(
    home: &Snapshot,
    data: &EditData,
    current: Option<&str>,
    marker: &str,
) -> Applied {
    let among = current.map_or_else(
        || home.clone(),
        |text| {
            home.with(File {
                project: data.project,
                stem: data.issue().to_string(),
                text: String::from(text),
            })
        },
    );
    let context = command::Context {
        now: data.at.millis(),
        by: data.by.clone(),
        mark: Some(String::from(marker)),
    };
    match command::write(&among, &data.edit, &context) {
        Ok(made) => Applied::Write {
            text: made.write.text,
            warnings: made.warnings,
        },
        Err(reason) => Applied::Refused(reason),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    pub issue: IssueId,
    pub checkout: CheckoutLocation,
    pub branch: String,
    pub brief: String,
}

pub fn start(home: Option<&Home>, id: IssueId) -> Result<Start, HomeRefusal> {
    let known = home.ok_or(HomeRefusal::NoHome)?;
    let (project, issue) = known
        .projects
        .iter()
        .find_map(|project| {
            project
                .issues
                .iter()
                .find(|issue| issue.id == id)
                .map(|issue| (project, issue))
        })
        .ok_or(HomeRefusal::UnknownIssue)?;
    let checkout = project
        .checkouts
        .first()
        .ok_or(HomeRefusal::NotCheckedOut)?;
    Ok(Start {
        issue: id,
        checkout: checkout.path.clone(),
        branch: format!("refs/heads/{}", issue.branch),
        brief: brief(&project.title, known.flow.as_deref(), issue),
    })
}

#[must_use]
pub fn branch(home: Option<&Home>, id: IssueId) -> String {
    let named = home.and_then(|known| {
        known
            .projects
            .iter()
            .flat_map(|project| &project.issues)
            .find(|issue| issue.id == id)
            .map(|issue| issue.branch.clone())
    });
    format!(
        "refs/heads/{}",
        named.unwrap_or_else(|| branch_of(id, None))
    )
}

fn brief(project: &str, flow: Option<&Flow>, issue: &Issue) -> String {
    let checklist = issue
        .checklist
        .iter()
        .map(checklist_line)
        .collect::<Vec<String>>();
    let notes = issue.log.iter().map(log_line).collect::<Vec<String>>();
    let section = |heading: &str, lines: &[String]| {
        if lines.is_empty() {
            String::new()
        } else {
            format!("\n{heading}:\n{}\n", lines.join("\n"))
        }
    };
    let body = &issue.body;
    format!(
        "You are working on the issue \"{title}\" in {project}. You are on a branch cut from {branch}; commit your work on it as you go.\n\n{body}\n{checklist}{notes}{stage}\nWork through what is left. When you are done, or stuck, say so, and sum up what you did and what is left.",
        title = issue.title,
        branch = issue.branch,
        body = if body.is_empty() {
            "The issue has no description."
        } else {
            body
        },
        checklist = section("Its checklist", &checklist),
        notes = section("Its notes so far", &notes),
        stage = staged(flow, issue),
    )
}

fn staged(flow: Option<&Flow>, issue: &Issue) -> String {
    let Some(name) = &issue.stage else {
        return String::new();
    };
    let skills = flow
        .and_then(|declared| declared.stage(name))
        .map_or(&[][..], |stage| stage.skills.as_slice());
    let serving = match skills {
        [] => String::new(),
        [leading] => format!(": follow the {leading} skill"),
        [leading, rest @ ..] => format!(
            ": follow the {leading} skill, and {} where it serves",
            rest.join(", ")
        ),
    };
    format!("\nIt is at the stage {name}{serving}.\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remote {
    Absent,
    At(String),
    Unreachable(String),
    PresentUnfetched(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub local: Option<String>,
    pub remote: Remote,
    pub base: String,
    pub behind: bool,
    pub ahead: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Move {
    Keep,
    Set { commit: String, old: Option<String> },
    Refuse(String),
}

impl Branch {
    #[must_use]
    pub fn next(self) -> Move {
        match (self.local, self.remote) {
            (None, Remote::Absent) => Move::Set {
                commit: self.base,
                old: None,
            },
            (None, Remote::At(remote)) => Move::Set {
                commit: remote,
                old: None,
            },
            (_, Remote::PresentUnfetched(reason)) => Move::Refuse(format!(
                "the issue's remote branch could not be fetched: {reason}"
            )),
            (None, Remote::Unreachable(reason)) => Move::Refuse(format!(
                "the project's remote can't be read, and the issue's branch may be there: {reason}"
            )),
            (Some(local), Remote::At(remote))
                if local != remote && self.behind =>
            {
                Move::Set {
                    commit: remote,
                    old: Some(local),
                }
            }
            (Some(local), Remote::At(remote))
                if local != remote && !self.ahead =>
            {
                Move::Refuse(String::from(
                    "the issue's branch here and on the remote have both moved on; merge them first",
                ))
            }
            (Some(_), _) => Move::Keep,
        }
    }
}

#[cfg(test)]
mod tests;

mod edit_id;
mod model;
mod operation;
pub use operation::*;
