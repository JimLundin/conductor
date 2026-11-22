//! An issue's file in the home, `projects/<project>/issues/<id>.md`
//! (docs/home.md §3), and the changes a host makes to it.
//!
//! The file's layout is `crate::flow::file`'s, and the flow's graph
//! (`crate::flow::graph`) reads issues from it. A change is written by
//! `crate::flow::command`, as the `home` command line's is, and read back
//! against the flow before it is kept.
//!
//! The host keeps each change as an [`Edit`] until the remote has it, and
//! when the remote moved on it applies the edit again on top
//! ([`Edit::replay`]). Then a field someone else changed since stays theirs
//! unless the edit is newer. Replay recognizes edits it already applied,
//! even when a separately submitted note has the same text.

use alloc::string::ToString;

use proto::flow::Flow;
use proto::{
    ChecklistChange, CommandId, Datum, Home, HostPath, Issue, IssueChange,
    IssueId, Item, NewIssue, Part, ProjectId, Rejection,
};
use serde::{Deserialize, Serialize};

use crate::flow::command::{self, said};
use crate::flow::file::{Document, checklist_line, log_line};
use crate::flow::graph::{self, File, branch_of};
use crate::flow::snapshot::Snapshot;
use crate::prelude::*;

/// The issue's file, from the home's root.
#[must_use]
fn file(project: &ProjectId, id: IssueId) -> String {
    format!("{}/{id}.md", folder(project))
}

/// Where a project keeps its issues, from the home's root.
#[must_use]
pub fn folder(project: &ProjectId) -> String {
    format!("projects/{}/issues", project.0)
}

/// Issue `id` of `project` in `text`, as the flow reads it.
///
/// # Errors
///
/// Why the flow can't read the file.
fn read(
    flow: &Flow,
    project: &ProjectId,
    id: IssueId,
    text: &str,
) -> Result<Issue, String> {
    graph::one(
        flow,
        &File {
            project: project.clone(),
            stem: id.to_string(),
            text: String::from(text),
        },
    )
    .map_err(|problem| problem.to_string())
}

/// The message of the commit an edit makes in the home.
#[must_use]
fn message(edit: &EditData) -> String {
    let id = edit.issue;
    match &edit.change {
        Change::Create(new) => {
            format!("Open issue {id}: {}", new.title.trim())
        }
        Change::Issue(IssueChange::Move { to, .. }) => {
            format!("Move issue {id} to {to}")
        }
        Change::Issue(IssueChange::Set { field, value }) => {
            format!("Set the {field} of issue {id} to {}", said(value))
        }
        Change::Issue(IssueChange::Body(_)) => {
            format!("Describe issue {id} anew")
        }
        Change::Issue(IssueChange::Note(_)) => format!("Note on issue {id}"),
        Change::Issue(IssueChange::Checklist(ChecklistChange::Add(_))) => {
            format!("Add an item to issue {id}")
        }
        Change::Issue(IssueChange::Checklist(ChecklistChange::Check {
            done: true,
            ..
        })) => {
            format!("Check off an item of issue {id}")
        }
        Change::Issue(IssueChange::Checklist(ChecklistChange::Check {
            done: false,
            ..
        })) => {
            format!("Uncheck an item of issue {id}")
        }
        Change::Issue(IssueChange::Checklist(ChecklistChange::Remove(_))) => {
            format!("Remove an item from issue {id}")
        }
    }
}

/// Who signs what a client's change writes in the log, as one word: the
/// host, and the client that sent it.
#[must_use]
fn signer(host: &str, client: &str) -> String {
    let word =
        |name: &str| name.split_whitespace().collect::<Vec<_>>().join("-");
    format!("{}·{}", word(host), word(client))
}

/// A change a host made to an issue, kept until the remote has it, by the
/// command that asked for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    command: CommandId,
    data: EditData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditData {
    project: ProjectId,
    issue: IssueId,
    /// When the host took it, in seconds since the Unix epoch.
    at: u64,
    /// Who signs the lines it adds to the log.
    by: String,
    change: Change,
    /// The value the field it sets had when it was made, as [`was`] gives
    /// it, so a replay can tell whether someone else changed it since.
    was: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Change {
    Create(NewIssue),
    Issue(IssueChange),
}

/// How an edit meets the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The host makes the change now.
    Record,
    /// Again, on top of the remote, whose last change to the file was at
    /// `changed`, in seconds since the Unix epoch.
    Replay { changed: u64 },
}

/// What an edit does to the issue's file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Applied {
    /// The file, and what the flow warns of it.
    Write {
        text: String,
        warnings: Vec<String>,
    },
    AlreadyRecorded,
    /// The file has it already.
    Unchanged,
    /// The issue it changes is gone, so it is dropped.
    Gone,
    /// The file isn't an issue any more, for this reason; the edit waits.
    Unreadable(String),
    /// The flow refuses what it would write, for this reason.
    Refused(String),
}

/// The value the field the edit sets has in its issue's file as it is
/// now: what [`Edit::was`] keeps.
#[must_use]
fn was(
    flow: &Flow,
    current: Option<&str>,
    edit: &EditData,
) -> Option<Vec<String>> {
    let Change::Issue(sets) = &edit.change else {
        return None;
    };
    read(flow, &edit.project, edit.issue, current?)
        .ok()
        .and_then(|issue| field(&issue, sets))
}

/// What the change sets has in `issue`, as words.
fn field(issue: &Issue, change: &IssueChange) -> Option<Vec<String>> {
    let one = |word: &str| words(&Datum::Text(String::from(word)));
    match change {
        IssueChange::Move {
            part: Part::Status, ..
        } => Some(one(&issue.status)),
        IssueChange::Move {
            part: Part::Stage, ..
        } => Some(one(issue.stage.as_deref().unwrap_or_default())),
        IssueChange::Set { field, .. } => Some(value_of(issue, field)),
        IssueChange::Body(_) => Some(one(&issue.body)),
        IssueChange::Note(_) | IssueChange::Checklist(_) => None,
    }
}

/// A field's or relation's value, the title among them, as words.
fn value_of(issue: &Issue, field: &str) -> Vec<String> {
    if field == "title" {
        return words(&Datum::Text(issue.title.clone()));
    }
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
        words,
    )
}

/// A value's words, trimmed, without empty ones, so one value and a list
/// of it compare alike.
fn words(value: &Datum) -> Vec<String> {
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

/// The issue has what the change gives it already.
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
            value_of(issue, field) == words(value)
        }
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

/// The item is no longer where the person picked it.
fn stale(issue: &Issue, item: &Item) -> bool {
    item.seen
        .as_ref()
        .is_some_and(|seen| *seen != issue.checklist)
        || item.index >= issue.checklist.len()
}

/// The edit on top of the issue's file as it is now, `None` when there is
/// none, among the home's other issues.
#[must_use]
fn apply(
    home: &Snapshot,
    current: Option<&str>,
    edit: &Edit,
    mode: Mode,
) -> Applied {
    let data = &edit.data;
    let marker = edit_marker(edit.command);
    let text = match (&data.change, current) {
        (Change::Create(_), Some(text)) => {
            return match home
                .flow()
                .map_err(String::from)
                .and_then(|flow| read(flow, &data.project, data.issue, text))
            {
                Ok(_) => Applied::AlreadyRecorded,
                Err(reason) => Applied::Unreadable(reason),
            };
        }
        (Change::Create(_), None) => return written(home, data, None, &marker),
        (Change::Issue(_), None) => return Applied::Gone,
        (Change::Issue(_), Some(text)) => text,
    };
    let Change::Issue(change) = &data.change else {
        return Applied::Gone;
    };
    let issue = match home
        .flow()
        .map_err(String::from)
        .and_then(|flow| read(flow, &data.project, data.issue, text))
        .and_then(|issue| Document::of(text).map(|document| (issue, document)))
    {
        Ok((_, document)) if document.has_comment(&marker) => {
            return Applied::AlreadyRecorded;
        }
        Ok((issue, _)) => issue,
        Err(reason) => return Applied::Unreadable(reason),
    };
    let moot = unchanged(&issue, change)
        || matches!(
            change,
            IssueChange::Checklist(
                ChecklistChange::Check { item, .. }
                    | ChecklistChange::Remove(item)
            ) if stale(&issue, item)
        );
    match mode {
        Mode::Record if unchanged(&issue, change) => Applied::Unchanged,
        Mode::Replay { changed }
            if moot
                || (data.was != field(&issue, change) && data.at < changed) =>
        {
            marked(text, &marker)
        }
        Mode::Record | Mode::Replay { .. } => {
            written(home, data, Some(text), &marker)
        }
    }
}

/// The front matter's comment that marks an edit as had.
fn edit_marker(id: CommandId) -> String {
    format!("conductor-edit: {:032x}", id.0)
}

/// The file as it is, marked as having had the edit.
fn marked(text: &str, marker: &str) -> Applied {
    Document::of(text).map_or_else(Applied::Unreadable, |mut document| {
        document.comment(marker);
        Applied::Write {
            text: document.text(),
            warnings: Vec::new(),
        }
    })
}

/// What `home::flow::command` writes for the edit, among the home's other
/// issues and the issue's file as it is now, marked as having had it.
fn written(
    home: &Snapshot,
    data: &EditData,
    current: Option<&str>,
    marker: &str,
) -> Applied {
    let stem = data.issue.to_string();
    let among = current.map_or_else(
        || home.clone(),
        |text| {
            home.with(File {
                project: data.project.clone(),
                stem: stem.clone(),
                text: String::from(text),
            })
        },
    );
    let context = command::Context {
        now: data.at.saturating_mul(1000),
        id: data.issue,
        by: data.by.clone(),
    };
    let edit = match &data.change {
        Change::Create(new) => command::Edit::New(new.clone()),
        Change::Issue(change) => command::Edit::Change {
            id: data.issue,
            changes: Vec::from([change.clone()]),
        },
    };
    match command::write(&among, &edit, &context) {
        Ok(made) => match marked(&made.write.text, marker) {
            Applied::Write { text, .. } => Applied::Write {
                text,
                warnings: made.warnings,
            },
            other => other,
        },
        Err(reason) => Applied::Refused(reason),
    }
}

/// What a session on an issue starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    pub issue: IssueId,
    /// The project's checkout the session's worktree is made from.
    pub checkout: HostPath,
    /// The issue's branch, as a full ref, which the session's is cut from.
    pub branch: String,
    /// The agent's first message.
    pub brief: String,
}

/// How a session on issue `id` starts, in its project's first checkout.
///
/// # Errors
///
/// The host has no home, its home lists no such issue, or the issue's
/// project has no checkout on the host.
pub fn start(home: Option<&Home>, id: IssueId) -> Result<Start, Rejection> {
    let known = home.ok_or(Rejection::NoHome)?;
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
        .ok_or(Rejection::UnknownIssue)?;
    let checkout = project.checkouts.first().ok_or(Rejection::NotCheckedOut)?;
    Ok(Start {
        issue: id,
        checkout: checkout.path.clone(),
        branch: format!("refs/heads/{}", issue.branch),
        brief: brief(&project.title, issue),
    })
}

/// Issue `id`'s branch, as a full ref, as the home has it.
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

/// The agent's first message: the issue as its file has it, and what to
/// do with it.
fn brief(project: &str, issue: &Issue) -> String {
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
        "You are working on the issue \"{title}\" in {project}. You are on a branch cut from {branch}; commit your work on it as you go.\n\n{body}\n{checklist}{notes}\nWork through what is left. When you are done, or stuck, say so, and sum up what you did and what is left.",
        title = issue.title,
        branch = issue.branch,
        body = if body.is_empty() {
            "The issue has no description."
        } else {
            body
        },
        checklist = section("Its checklist", &checklist),
        notes = section("Its notes so far", &notes),
    )
}

/// The remote's copy of an issue's branch, as the checkout found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remote {
    /// The project has no remote, or its remote has no such branch.
    Absent,
    At(String),
    /// The remote couldn't be read, for this reason.
    Unreachable(String),
    /// The ref exists, but the checkout did not obtain it.
    PresentUnfetched(String),
}

/// Where an issue's branch stands in a checkout, before a session on it
/// starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub local: Option<String>,
    pub remote: Remote,
    /// Where a new issue's branch starts: the remote's default branch,
    /// else the checkout's `HEAD`.
    pub base: String,
    /// The local branch is an ancestor of the remote's.
    pub behind: bool,
    /// The remote's branch is an ancestor of the local one.
    pub ahead: bool,
}

/// What becomes of the checkout's issue branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Move {
    Keep,
    /// Point it at `commit`; `old` is where it is, `None` when it's new.
    Set {
        commit: String,
        old: Option<String>,
    },
    /// The session can't start from it, for this reason.
    Refuse(String),
}

impl Branch {
    /// The branch follows the remote's, which carries the issue from host
    /// to host. One with commits of its own the remote lacks stays as it
    /// is; one whose remote moved on too, or a new one while the remote
    /// can't be read, would leave the remote's work out, so the session
    /// doesn't start.
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

mod operation;
pub use operation::*;
