//! An issue's file in the home, `projects/<project>/issues/<id>.md`
//! (docs/home.md §3), and the changes a host makes to it.
//!
//! A file is `+++` TOML front matter, then the body, then `## Checklist`
//! and `## Notes`. Front matter a person added is kept as written. A change
//! is an [`Edit`] the host keeps until the remote has it: applied to the
//! file as it is, it gives the file's next text, so the host can apply it
//! again on top of whatever the remote holds and get the same result.

use alloc::collections::BTreeMap;
use alloc::string::ToString;

use proto::{
    ChecklistItem, Issue, IssueChange, IssueFields, IssueId, IssueStatus,
    Priority, ProjectId, Rejection,
};
use serde::{Deserialize, Serialize};
use toml::{Table, Value};

use crate::prelude::*;

const CHECKLIST: &str = "## Checklist";
const NOTES: &str = "## Notes";

/// The issue's id as its file and branch name it: a lower-case ULID.
#[must_use]
pub fn name(id: IssueId) -> String {
    ulid::Ulid::from(id.0).to_string().to_lowercase()
}

/// The issue's file, from the home's root.
#[must_use]
pub fn file(project: &ProjectId, id: IssueId) -> String {
    format!("{}/{}.md", folder(project), name(id))
}

/// Where a project keeps its issues, from the home's root.
#[must_use]
pub fn folder(project: &ProjectId) -> String {
    format!("projects/{}/issues", project.0)
}

/// An issue as its file has it, with the front matter Conductor doesn't
/// read.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub issue: Issue,
    extra: Table,
}

/// The issue in the file named `stem`.md.
///
/// # Errors
///
/// Why the file isn't an issue.
pub fn parse(stem: &str, text: &str) -> Result<Parsed, String> {
    let (front, rest) = text
        .strip_prefix("+++\n")
        .and_then(|after| after.split_once("\n+++\n"))
        .ok_or_else(|| {
            String::from("it does not start with +++ front matter")
        })?;
    let mut table = toml::from_str::<Table>(front)
        .map_err(|err| format!("front matter: {err}"))?;
    let id = ulid::Ulid::from_string(stem)
        .ok()
        .map(|ulid| IssueId(ulid.0))
        .filter(|id| name(*id) == stem)
        .filter(|_| string(table.remove("id")).as_deref() == Some(stem))
        .ok_or_else(|| String::from("its id is not its file's name"))?;
    let title = string(table.remove("title"))
        .ok_or_else(|| String::from("it has no title"))?;
    let status = word(
        table.remove("status"),
        "status",
        IssueStatus::named,
        &IssueStatus::ALL.map(IssueStatus::word),
    )?
    .unwrap_or(IssueStatus::Open);
    let priority = word(
        table.remove("priority"),
        "priority",
        Priority::named,
        &Priority::ALL.map(Priority::word),
    )?
    .unwrap_or(Priority::Normal);
    let labels = match table.remove("labels") {
        Some(Value::Array(values)) => values
            .into_iter()
            .filter_map(|value| string(Some(value)))
            .collect(),
        _ => Vec::new(),
    };
    let branch =
        string(table.remove("branch")).unwrap_or_else(|| branch_of(id));
    let created = match table.remove("created") {
        Some(Value::Datetime(at)) => at.to_string(),
        other => string(other).unwrap_or_default(),
    };
    let (body, checklist, notes) = sections(rest);
    Ok(Parsed {
        issue: Issue {
            id,
            title,
            body,
            status,
            priority,
            labels,
            branch,
            created,
            checklist,
            notes,
        },
        extra: table,
    })
}

fn string(value: Option<Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) => Some(text),
        _ => None,
    }
}

/// A field whose value is one of a few words, `None` when it is absent.
fn word<T>(
    value: Option<Value>,
    field: &str,
    named: impl Fn(&str) -> Option<T>,
    words: &[&str],
) -> Result<Option<T>, String> {
    string(value).map_or(Ok(None), |text| {
        named(&text).map(Some).ok_or_else(|| {
            format!("{field} {text:?} is not one of {}", words.join(", "))
        })
    })
}

/// The body, the checklist and the notes after the front matter. A note
/// continues on lines indented by two spaces.
fn sections(rest: &str) -> (String, Vec<ChecklistItem>, Vec<String>) {
    let mut body = Vec::new();
    let mut checklist = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut section = "";
    for line in rest.lines() {
        match line.trim_end() {
            heading @ (CHECKLIST | NOTES) => section = heading,
            _ if section == CHECKLIST => checklist.extend(item(line)),
            _ if section == NOTES => match (
                line.strip_prefix("- "),
                line.strip_prefix("  "),
                notes.last_mut(),
            ) {
                (Some(note), _, _) => notes.push(String::from(note)),
                (None, Some(more), Some(last)) => {
                    last.push('\n');
                    last.push_str(more);
                }
                _ => {}
            },
            _ => body.push(line),
        }
    }
    (String::from(body.join("\n").trim()), checklist, notes)
}

fn item(line: &str) -> Option<ChecklistItem> {
    let (done, text) = line
        .strip_prefix("- [x] ")
        .or_else(|| line.strip_prefix("- [X] "))
        .map(|text| (true, text))
        .or_else(|| line.strip_prefix("- [ ] ").map(|text| (false, text)))?;
    Some(ChecklistItem {
        done,
        text: String::from(text),
    })
}

/// The file's text.
#[must_use]
pub fn render(parsed: &Parsed) -> String {
    let issue = &parsed.issue;
    let quoted = |text: &str| Value::String(String::from(text)).to_string();
    let labels =
        Value::Array(issue.labels.iter().cloned().map(Value::String).collect());
    let created = issue
        .created
        .parse::<toml::value::Datetime>()
        .map_or_else(|_| quoted(&issue.created), |at| at.to_string());
    let extra = toml::to_string(&parsed.extra).unwrap_or_default();
    let front = format!(
        "+++\nid = {}\ntitle = {}\nstatus = {}\npriority = {}\nlabels = {labels}\nbranch = {}\ncreated = {created}\n{extra}+++\n",
        quoted(&name(issue.id)),
        quoted(&issue.title),
        quoted(issue.status.word()),
        quoted(issue.priority.word()),
        quoted(&issue.branch),
    );
    let body = (!issue.body.is_empty()).then(|| format!("{}\n", issue.body));
    let checklist = (!issue.checklist.is_empty()).then(|| {
        issue
            .checklist
            .iter()
            .fold(format!("{CHECKLIST}\n"), |text, item| {
                let mark = if item.done { 'x' } else { ' ' };
                format!("{text}- [{mark}] {}\n", item.text)
            })
    });
    let notes = (!issue.notes.is_empty()).then(|| {
        issue.notes.iter().fold(format!("{NOTES}\n"), |text, note| {
            format!("{text}- {}\n", note.replace('\n', "\n  "))
        })
    });
    [body, checklist, notes]
        .into_iter()
        .flatten()
        .fold(front, |text, section| format!("{text}\n{section}"))
}

/// The issues of `project` in its folder's files, each `.md` file by its
/// name without the extension, by id; and why any file isn't one.
#[must_use]
pub fn listed(
    project: &ProjectId,
    files: Vec<(String, Result<String, String>)>,
) -> (Vec<Issue>, Vec<String>) {
    let (mut issues, problems) = files.into_iter().fold(
        (Vec::new(), Vec::new()),
        |(mut issues, mut problems), (stem, text)| {
            match text.and_then(|read| parse(&stem, &read)) {
                Ok(parsed) => issues.push(parsed.issue),
                Err(problem) => problems
                    .push(format!("{}/{stem}.md: {problem}", folder(project))),
            }
            (issues, problems)
        },
    );
    issues.sort_by_key(|issue| issue.id);
    (issues, problems)
}

/// The project whose issues include `id`.
#[must_use]
pub fn locate(
    issues: &BTreeMap<ProjectId, Vec<Issue>>,
    id: IssueId,
) -> Option<ProjectId> {
    issues
        .iter()
        .find(|(_, listed)| listed.iter().any(|issue| issue.id == id))
        .map(|(project, _)| project.clone())
}

/// The fields an issue opens with in `project`, one of the home's
/// `projects`.
///
/// # Errors
///
/// The host has no home, the home lists no such project, or the fields
/// won't do.
pub fn opening(
    remote: Option<&str>,
    projects: &[ProjectId],
    project: &ProjectId,
    wanted: &IssueFields,
) -> Result<IssueFields, Rejection> {
    crate::configured(remote)?;
    if !projects.contains(project) {
        return Err(Rejection::UnknownProject);
    }
    fields(wanted).map_err(|reason| Rejection::IssueNotSaved { reason })
}

/// The project of the issue a change is to, and the change.
///
/// # Errors
///
/// The host has no home, its home lists no such issue, or the change
/// won't do.
pub fn changing(
    remote: Option<&str>,
    issues: &BTreeMap<ProjectId, Vec<Issue>>,
    id: IssueId,
    change: IssueChange,
) -> Result<(ProjectId, IssueChange), Rejection> {
    crate::configured(remote)?;
    let project = locate(issues, id).ok_or(Rejection::UnknownIssue)?;
    let checked = match change {
        IssueChange::Edit(wanted) => fields(&wanted).map(IssueChange::Edit),
        IssueChange::Note(text) if text.trim().is_empty() => {
            Err(String::from("a note needs text"))
        }
        other => Ok(other),
    };
    checked
        .map(|change| (project, change))
        .map_err(|reason| Rejection::IssueNotSaved { reason })
}

/// The message of the commit an edit makes in the home.
#[must_use]
pub fn message(edit: &Edit) -> String {
    let id = name(edit.issue);
    match &edit.change {
        Change::Create(fields) => {
            format!("Open issue {id}: {}", fields.title.trim())
        }
        Change::Issue(IssueChange::Edit(_)) => format!("Edit issue {id}"),
        Change::Issue(IssueChange::Status(status)) => {
            format!("Move issue {id} to {}", status.word())
        }
        Change::Issue(IssueChange::Checklist(_)) => {
            format!("Check off issue {id}'s list")
        }
        Change::Issue(IssueChange::Note(_)) => format!("Note on issue {id}"),
    }
}

/// Who signs a note: the host, and the client that sent it.
#[must_use]
pub fn signer(host: &str, client: &str) -> String {
    format!("{host} · {client}")
}

/// A change a host made to an issue, kept until the remote has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    pub project: ProjectId,
    pub issue: IssueId,
    /// When the host took it, in seconds since the Unix epoch.
    pub at: u64,
    /// Who signs a note it adds.
    pub by: String,
    pub change: Change,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change {
    Create(IssueFields),
    Issue(IssueChange),
}

/// What an edit does to the issue's file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    Write(String),
    /// It is in the file already.
    Unchanged,
    /// The issue it changes is gone, so it is dropped.
    Gone,
}

/// The edit on top of the issue's file as it is now, `None` when there is
/// none. Applying it again gives the same file.
#[must_use]
pub fn apply(current: Option<&str>, edit: &Edit) -> Applied {
    let stem = name(edit.issue);
    let read = current.map(|text| parse(&stem, text));
    match (&edit.change, read) {
        (Change::Create(_), Some(_)) => Applied::Unchanged,
        (Change::Create(wanted), None) => {
            fields(wanted).map_or(Applied::Unchanged, |made| {
                Applied::Write(render(&Parsed {
                    issue: opened(edit, made),
                    extra: Table::new(),
                }))
            })
        }
        (Change::Issue(_), None | Some(Err(_))) => Applied::Gone,
        (Change::Issue(change), Some(Ok(parsed))) => {
            let next = Parsed {
                issue: changed(parsed.issue.clone(), change, edit),
                extra: parsed.extra.clone(),
            };
            if next == parsed {
                Applied::Unchanged
            } else {
                Applied::Write(render(&next))
            }
        }
    }
}

fn opened(edit: &Edit, made: IssueFields) -> Issue {
    Issue {
        id: edit.issue,
        title: made.title,
        body: made.body,
        status: IssueStatus::Open,
        priority: made.priority,
        labels: made.labels,
        branch: branch_of(edit.issue),
        created: rfc3339(edit.at),
        checklist: Vec::new(),
        notes: Vec::new(),
    }
}

fn changed(issue: Issue, change: &IssueChange, edit: &Edit) -> Issue {
    match change {
        IssueChange::Edit(wanted) => match fields(wanted) {
            Ok(made) => Issue {
                title: made.title,
                body: made.body,
                priority: made.priority,
                labels: made.labels,
                ..issue
            },
            Err(_) => issue,
        },
        IssueChange::Status(status) => Issue {
            status: *status,
            ..issue
        },
        IssueChange::Checklist(items) => Issue {
            checklist: items.clone(),
            ..issue
        },
        IssueChange::Note(text) => {
            let line =
                format!("{} {}: {}", date(edit.at), edit.by, text.trim());
            let mut notes = issue.notes;
            if !notes.contains(&line) {
                notes.push(line);
            }
            Issue { notes, ..issue }
        }
    }
}

/// The fields as an issue keeps them: trimmed, each label once.
///
/// # Errors
///
/// When the title is empty.
pub fn fields(wanted: &IssueFields) -> Result<IssueFields, String> {
    let title = String::from(wanted.title.trim());
    if title.is_empty() {
        return Err(String::from("an issue needs a title"));
    }
    let labels = wanted.labels.iter().map(|label| label.trim()).fold(
        Vec::<String>::new(),
        |mut kept, label| {
            if !label.is_empty() && !kept.iter().any(|known| known == label) {
                kept.push(String::from(label));
            }
            kept
        },
    );
    Ok(IssueFields {
        title,
        body: String::from(wanted.body.trim()),
        priority: wanted.priority,
        labels,
    })
}

fn branch_of(id: IssueId) -> String {
    format!("issue/{}", name(id))
}

fn date(at: u64) -> String {
    let (year, month, day) = proto::civil(at / 86_400);
    format!("{year}-{month:02}-{day:02}")
}

fn rfc3339(at: u64) -> String {
    let rest = at % 86_400;
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        date(at),
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests;
