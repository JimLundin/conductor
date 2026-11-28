use alloc::string::ToString;

use serde::{Deserialize, Serialize};

pub use edit_id::EditId;
pub use model::{
    ChecklistChange, ChecklistItem, Datum, Entry, Issue, IssueChange, Item,
    LogEvent, NewIssue, Note, Part, Section,
};

use crate::flow::Flow;
use crate::flow::command::{self, said};
use crate::flow::graph::branch_of;
use crate::flow::snapshot::Snapshot;
use crate::issue::body::{checklist_line, log_line};
use crate::prelude::*;
use crate::{
    CheckoutLocation, Home, HomeRefusal, IssueId, IssueKey, ProjectId, UnixSecs,
};

pub mod cache;
mod comment;
pub use comment::*;
mod relation;
pub use relation::*;
mod native;
pub use native::{IssueCreate, IssueWrite, NativeIssue, Opened};
mod observation;
mod tracking;
pub use tracking::*;
pub mod ownership;
pub use observation::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    pub reading: Result<Observation, String>,
    pub applied: Vec<EditId>,
}

#[must_use]
pub(crate) fn message(edit: &command::Edit) -> String {
    match edit {
        command::Edit::New { id, issue } => {
            format!("Open issue {id}: {}", issue.title.trim())
        }
        command::Edit::Change { id, changes, .. } => match changes.as_slice() {
            [change] => change_message(id, change),
            _ => format!("Change issue {id}"),
        },
    }
}

fn change_message(id: &IssueId, change: &IssueChange) -> String {
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
    fn key(&self) -> IssueKey {
        self.edit.key()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Record,
    Replay { changed: UnixSecs },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Applied {
    change: command::Change,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NotApplied {
    AlreadyRecorded,
    Unchanged,
    Gone,
    Unreadable(String),
    Refused(String),
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
                .map(|target| target.said_from(issue.project))
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

fn apply(
    home: &Snapshot,
    current: Option<&Observed>,
    edit: &Edit,
    mode: Mode,
) -> Result<Applied, NotApplied> {
    let data = &edit.data;
    let (changes, observed) = match (&data.edit, current) {
        (command::Edit::New { .. }, Some(observed)) => {
            return Err(observed.reading.as_ref().map_or_else(
                |reason| NotApplied::Unreadable(reason.clone()),
                |_| NotApplied::AlreadyRecorded,
            ));
        }
        (command::Edit::New { .. }, None) => return written(home, data, None),
        (command::Edit::Change { .. }, None) => return Err(NotApplied::Gone),
        (command::Edit::Change { changes, .. }, Some(observed)) => {
            (changes, observed)
        }
    };
    let reading = match &observed.reading {
        Ok(_) if observed.applied.contains(&edit.id) => {
            return Err(NotApplied::AlreadyRecorded);
        }
        Ok(issue) => issue,
        Err(reason) => return Err(NotApplied::Unreadable(reason.clone())),
    };
    let (issue, _) = reading.editable().map_err(NotApplied::Unreadable)?;
    match mode {
        Mode::Record
            if changes.iter().all(|change| unchanged(issue, change)) =>
        {
            Err(NotApplied::Unchanged)
        }
        Mode::Replay { changed }
            if changes.iter().all(|change| moot(issue, change))
                || (data.at < changed && overtaken(data, issue)) =>
        {
            Ok(Applied {
                change: command::Change {
                    observation: reading.clone(),
                    created: false,
                    mutations: Vec::new(),
                },
                warnings: Vec::new(),
            })
        }
        Mode::Record | Mode::Replay { .. } => {
            written(home, data, Some(reading))
        }
    }
}

fn written(
    home: &Snapshot,
    data: &EditData,
    current: Option<&Observation>,
) -> Result<Applied, NotApplied> {
    let among =
        current.map_or_else(|| home.clone(), |issue| home.with(issue.clone()));
    let context = command::Context {
        now: data.at.millis(),
        by: data.by.clone(),
    };
    let key = data.key();
    let observed = among.observation(key.project, &key.id);
    match command::plan(&among, &data.edit, observed, &context) {
        Ok(made) => Ok(Applied {
            change: made.change,
            warnings: made.warnings,
        }),
        Err(reason) => Err(NotApplied::Refused(reason)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    pub issue: IssueKey,
    pub checkout: CheckoutLocation,
    pub branch: String,
    pub brief: String,
}

pub fn start(
    home: Option<&Home>,
    id: IssueKey,
) -> Result<(ProjectId, Start), HomeRefusal> {
    let known = home.ok_or(HomeRefusal::NoHome)?;
    let project = known
        .projects
        .iter()
        .find(|project| project.id == id.project)
        .ok_or(HomeRefusal::UnknownIssue)?;
    let observation = project
        .issues
        .iter()
        .find(|observation| observation.id() == &id.id)
        .ok_or(HomeRefusal::UnknownIssue)?;
    let (issue, _) = observation
        .editable()
        .map_err(|reason| HomeRefusal::IssueRefused { reason })?;
    let checkout = project
        .checkouts
        .first()
        .ok_or(HomeRefusal::NotCheckedOut)?;
    Ok((
        project.id,
        Start {
            issue: id,
            checkout: checkout.path.clone(),
            branch: format!("refs/heads/{}", issue.branch),
            brief: brief(&project.title, known.flow.as_deref(), issue),
        },
    ))
}

#[must_use]
pub fn branch(home: Option<&Home>, id: &IssueKey) -> String {
    let named = home.and_then(|known| {
        known
            .projects
            .iter()
            .filter(|project| project.id == id.project)
            .flat_map(|project| &project.issues)
            .filter_map(Observation::issue)
            .find(|issue| issue.id == id.id)
            .map(|issue| issue.branch.clone())
    });
    format!(
        "refs/heads/{}",
        named.unwrap_or_else(|| branch_of(&id.id, None))
    )
}

fn brief(project: &str, flow: Option<&Flow>, issue: &Issue) -> String {
    let checklist = issue
        .checklist
        .iter()
        .map(checklist_line)
        .collect::<Vec<String>>();
    let notes = issue
        .log
        .iter()
        .map(log_line)
        .chain(
            issue
                .notes
                .iter()
                .map(|note| format!("- {}: {}", note.author, note.text)),
        )
        .collect::<Vec<String>>();
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

pub mod body;
pub mod document;
mod edit_id;
pub mod storage;

pub const ID: &str = "id";
pub const TITLE: &str = "title";
pub const KIND: &str = "kind";
pub const STATUS: &str = "status";
pub const STAGE: &str = "stage";
mod model;
mod operation;
pub use operation::*;
