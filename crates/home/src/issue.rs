//! An issue's file in the home, `projects/<project>/issues/<id>.md`
//! (docs/home.md §3), and the changes a host makes to it.
//!
//! A file is `+++` TOML front matter, then the body, then `## Checklist`
//! and `## Notes`. A change rewrites only the lines it is about, so what a
//! person wrote anywhere else in the file stays as written: comments, keys
//! and sections Conductor doesn't read, and the order of it all.
//!
//! The host keeps each change as an [`Edit`] until the remote has it, and
//! when the remote moved on it applies the edit again on top
//! ([`Mode::Replay`]). Then a field someone else changed since stays theirs
//! unless the edit is newer, and a note or an item the file already has
//! isn't added twice.

use alloc::collections::BTreeMap;
use alloc::string::ToString;

use proto::{
    ChecklistItem, Home, HostPath, Issue, IssueChange, IssueFields, IssueId,
    IssueStatus, Priority, ProjectId, Rejection,
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

/// A file as lines: its front matter's, between the `+++` lines, and the
/// rest's. Windows line ends read as Unix ones.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Lines {
    front: Vec<String>,
    rest: Vec<String>,
}

impl Lines {
    fn of(text: &str) -> Result<Self, String> {
        let unix = text.replace("\r\n", "\n");
        let (front, rest) = unix
            .strip_prefix("+++\n")
            .and_then(|after| after.split_once("\n+++\n"))
            .ok_or_else(|| {
                String::from("it does not start with +++ front matter")
            })?;
        Ok(Self {
            front: front.split('\n').map(String::from).collect(),
            rest: rest.lines().map(String::from).collect(),
        })
    }

    fn text(&self) -> String {
        let rest = self
            .rest
            .iter()
            .fold(String::new(), |text, line| format!("{text}{line}\n"));
        format!("+++\n{}\n+++\n{rest}", self.front.join("\n"))
    }

    fn table(&self) -> Result<Table, String> {
        toml::from_str::<Table>(&self.front.join("\n"))
            .map_err(|err| format!("front matter: {err}"))
    }

    /// Sets the front matter's top-level `key`, keeping the comment its
    /// line had; a key it lacks goes before the first table.
    fn set(&mut self, key: &str, value: &str) {
        let tables = self
            .front
            .iter()
            .position(|line| line.trim_start().starts_with('['))
            .unwrap_or(self.front.len());
        let found = self.front.iter().take(tables).position(|line| {
            line.split_once('=')
                .is_some_and(|(named, _)| named.trim() == key)
        });
        match found.and_then(|at| self.front.get_mut(at)) {
            Some(line) => {
                let kept = String::from(comment(line));
                *line = format!("{key} = {value}{kept}");
            }
            None => self.front.insert(tables, format!("{key} = {value}")),
        }
    }

    /// Where the body ends: at the checklist or the notes, whichever is
    /// first.
    fn body_end(&self) -> usize {
        self.rest
            .iter()
            .position(|line| matches!(line.trim_end(), CHECKLIST | NOTES))
            .unwrap_or(self.rest.len())
    }

    fn body(&mut self, body: &str) {
        let end = self.body_end();
        let mut lines = Vec::from([String::new()]);
        lines.extend(body.lines().map(String::from));
        if end < self.rest.len() {
            lines.push(String::new());
        }
        self.rest.splice(..end, lines);
    }

    /// The section under `heading`, from the heading's line to the next
    /// heading or the end, if the file has one.
    fn section(&self, heading: &str) -> Option<(usize, usize)> {
        let body_end = self.body_end();
        let start = self
            .rest
            .iter()
            .skip(body_end)
            .position(|line| line.trim_end() == heading)?
            + body_end;
        let end = self
            .rest
            .iter()
            .skip(start + 1)
            .position(|line| line.starts_with("## "))
            .map_or(self.rest.len(), |at| at + start + 1);
        Some((start, end))
    }

    /// The checklist line whose item has this text.
    fn item(&self, text: &str) -> Option<(usize, ChecklistItem)> {
        let (start, end) = self.section(CHECKLIST)?;
        (start..end).find_map(|at| {
            self.rest
                .get(at)
                .and_then(|line| item(line))
                .filter(|found| found.text == text)
                .map(|found| (at, found))
        })
    }

    /// Adds `lines` after the last line with text of the section under
    /// `heading`, or as a new section: before the one under `before`, or
    /// at the end.
    fn append(
        &mut self,
        heading: &str,
        before: Option<&str>,
        lines: Vec<String>,
    ) {
        if let Some((start, end)) = self.section(heading) {
            let last = (start..end)
                .rev()
                .find(|at| {
                    self.rest
                        .get(*at)
                        .is_some_and(|line| !line.trim().is_empty())
                })
                .unwrap_or(start);
            let after = last + 1;
            self.rest.splice(after..after, lines);
            return;
        }
        let mut section = Vec::from([String::from(heading)]);
        section.extend(lines);
        if let Some((at, _)) = before.and_then(|other| self.section(other)) {
            section.push(String::new());
            self.rest.splice(at..at, section);
        } else {
            if self.rest.last().is_some_and(|line| !line.trim().is_empty()) {
                self.rest.push(String::new());
            }
            self.rest.extend(section);
        }
    }
}

/// The comment ending a line of TOML, with the space before it, or
/// nothing: a `#` outside a string.
fn comment(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (at, character) in line.char_indices() {
        match (quote, character, escaped) {
            (Some('"'), '\\', false) => {
                escaped = true;
                continue;
            }
            (Some(open), close, false) if close == open => quote = None,
            (None, '"' | '\'', _) => quote = Some(character),
            (None, '#', _) => {
                let start =
                    line.get(..at).map_or(at, |before| before.trim_end().len());
                return line.get(start..).unwrap_or("");
            }
            _ => {}
        }
        escaped = false;
    }
    ""
}

/// The issue in the file named `stem`.md.
///
/// # Errors
///
/// Why the file isn't an issue.
pub fn parse(stem: &str, text: &str) -> Result<Issue, String> {
    let lines = Lines::of(text)?;
    let mut table = lines.table()?;
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
    let (body, checklist, notes) = sections(&lines.rest);
    Ok(Issue {
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

/// The body, the checklist and the notes after the front matter. The body
/// runs to the checklist or the notes, whichever is first; a section after
/// them that Conductor doesn't know is left out. A note continues on lines
/// indented by two spaces.
fn sections(rest: &[String]) -> (String, Vec<ChecklistItem>, Vec<String>) {
    let mut body = Vec::new();
    let mut checklist = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut section = "";
    for line in rest {
        match (line.trim_end(), section) {
            (heading @ (CHECKLIST | NOTES), _) => section = heading,
            (other, CHECKLIST | NOTES) if other.starts_with("## ") => {
                section = "other";
            }
            (_, "") => body.push(line.as_str()),
            (_, CHECKLIST) => checklist.extend(item(line)),
            (_, NOTES) => noted(&mut notes, line),
            _ => {}
        }
    }
    (String::from(body.join("\n").trim()), checklist, notes)
}

fn noted(notes: &mut Vec<String>, line: &str) {
    match (
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
    }
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

fn item_line(item: &ChecklistItem) -> String {
    let mark = if item.done { 'x' } else { ' ' };
    format!("- [{mark}] {}", item.text)
}

fn note_lines(note: &str) -> Vec<String> {
    note.lines()
        .enumerate()
        .map(|(at, line)| {
            if at == 0 {
                format!("- {line}")
            } else {
                format!("  {line}")
            }
        })
        .collect()
}

fn quoted(text: &str) -> String {
    Value::String(String::from(text)).to_string()
}

fn labels_value(labels: &[String]) -> String {
    Value::Array(labels.iter().cloned().map(Value::String).collect())
        .to_string()
}

/// A new issue's file.
#[must_use]
pub fn render(issue: &Issue) -> String {
    let created = issue
        .created
        .parse::<toml::value::Datetime>()
        .map_or_else(|_| quoted(&issue.created), |at| at.to_string());
    let mut lines = Lines {
        front: Vec::from([
            format!("id = {}", quoted(&name(issue.id))),
            format!("title = {}", quoted(&issue.title)),
            format!("status = {}", quoted(issue.status.word())),
            format!("priority = {}", quoted(issue.priority.word())),
            format!("labels = {}", labels_value(&issue.labels)),
            format!("branch = {}", quoted(&issue.branch)),
            format!("created = {created}"),
        ]),
        rest: Vec::new(),
    };
    if !issue.body.is_empty() {
        lines.body(&issue.body);
    }
    for added in &issue.checklist {
        lines.append(CHECKLIST, Some(NOTES), Vec::from([item_line(added)]));
    }
    for note in &issue.notes {
        lines.append(NOTES, None, note_lines(note));
    }
    lines.text()
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
                Ok(issue) => issues.push(issue),
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

/// The project of the issue a change is to, and the change as the issue
/// keeps it.
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
        IssueChange::Title(title) => {
            needs(&title, "a title").map(IssueChange::Title)
        }
        IssueChange::Body(body) => {
            Ok(IssueChange::Body(String::from(body.trim())))
        }
        IssueChange::Labels(labels) => {
            Ok(IssueChange::Labels(normal_labels(&labels)))
        }
        IssueChange::AddItem(text) => {
            needs(&text, "a checklist item").map(IssueChange::AddItem)
        }
        IssueChange::Note(text) => {
            needs(&text, "a note").map(IssueChange::Note)
        }
        other @ (IssueChange::Priority(_)
        | IssueChange::Status(_)
        | IssueChange::Check { .. }
        | IssueChange::RemoveItem(_)) => Ok(other),
    };
    checked
        .map(|change| (project, change))
        .map_err(|reason| Rejection::IssueNotSaved { reason })
}

/// `text` trimmed, unless that leaves nothing.
fn needs(text: &str, what: &str) -> Result<String, String> {
    let kept = text.trim();
    if kept.is_empty() {
        Err(format!("{what} needs text"))
    } else {
        Ok(String::from(kept))
    }
}

/// The message of the commit an edit makes in the home.
#[must_use]
pub fn message(edit: &Edit) -> String {
    let id = name(edit.issue);
    match &edit.change {
        Change::Create(fields) => {
            format!("Open issue {id}: {}", fields.title.trim())
        }
        Change::Issue(IssueChange::Title(_)) => format!("Retitle issue {id}"),
        Change::Issue(IssueChange::Body(_)) => {
            format!("Describe issue {id} anew")
        }
        Change::Issue(IssueChange::Labels(_)) => format!("Label issue {id}"),
        Change::Issue(IssueChange::Priority(priority)) => {
            format!("Give issue {id} {} priority", priority.word())
        }
        Change::Issue(IssueChange::Status(status)) => {
            format!("Move issue {id} to {}", status.word())
        }
        Change::Issue(IssueChange::Check { done: true, .. }) => {
            format!("Check off an item of issue {id}")
        }
        Change::Issue(IssueChange::Check { done: false, .. }) => {
            format!("Uncheck an item of issue {id}")
        }
        Change::Issue(IssueChange::AddItem(_)) => {
            format!("Add an item to issue {id}")
        }
        Change::Issue(IssueChange::RemoveItem(_)) => {
            format!("Remove an item from issue {id}")
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
    /// The value the field it sets had when it was made, as [`was`] gives
    /// it, so a replay can tell whether someone else changed it since.
    pub was: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change {
    Create(IssueFields),
    Issue(IssueChange),
}

/// How an edit meets the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The host makes the change now.
    Record,
    /// Again, on top of the remote, whose last change to the file was at
    /// `changed`, in seconds since the Unix epoch.
    Replay { changed: u64 },
}

/// What an edit does to the issue's file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    Write(String),
    /// The file has it already, or a newer change replaced it.
    Unchanged,
    /// The issue it changes is gone, so it is dropped.
    Gone,
    /// The file isn't an issue any more, for this reason; the edit waits.
    Unreadable(String),
}

/// The value the field `change` sets has in issue `id`'s file as it is
/// now: what [`Edit::was`] keeps.
#[must_use]
pub fn was(
    current: Option<&str>,
    id: IssueId,
    change: &Change,
) -> Option<String> {
    let Change::Issue(sets) = change else {
        return None;
    };
    parse(&name(id), current?)
        .ok()
        .and_then(|issue| field(&issue, sets))
}

fn field(issue: &Issue, change: &IssueChange) -> Option<String> {
    match change {
        IssueChange::Title(_) => Some(issue.title.clone()),
        IssueChange::Body(_) => Some(issue.body.clone()),
        IssueChange::Labels(_) => Some(issue.labels.join(", ")),
        IssueChange::Priority(_) => Some(String::from(issue.priority.word())),
        IssueChange::Status(_) => Some(String::from(issue.status.word())),
        IssueChange::Check { .. }
        | IssueChange::AddItem(_)
        | IssueChange::RemoveItem(_)
        | IssueChange::Note(_) => None,
    }
}

/// The edit on top of the issue's file as it is now, `None` when there is
/// none.
#[must_use]
pub fn apply(current: Option<&str>, edit: &Edit, mode: Mode) -> Applied {
    match (&edit.change, current) {
        (Change::Create(_), Some(_)) => Applied::Unchanged,
        (Change::Create(wanted), None) => fields(wanted)
            .map_or(Applied::Unchanged, |made| {
                Applied::Write(render(&opened(edit, made)))
            }),
        (Change::Issue(_), None) => Applied::Gone,
        (Change::Issue(change), Some(text)) => {
            let read = parse(&name(edit.issue), text)
                .and_then(|issue| Lines::of(text).map(|lines| (issue, lines)));
            match read {
                Err(reason) => Applied::Unreadable(reason),
                Ok((issue, lines)) => {
                    changed(lines, &issue, change, (edit, mode))
                        .map_or(Applied::Unchanged, |next| {
                            Applied::Write(next.text())
                        })
                }
            }
        }
    }
}

/// The file's lines with the change, or `None` when it changes nothing: it
/// is in the file already, or on a replay a newer change replaced it.
fn changed(
    lines: Lines,
    issue: &Issue,
    change: &IssueChange,
    (edit, mode): (&Edit, Mode),
) -> Option<Lines> {
    let replaying = match mode {
        Mode::Record => false,
        Mode::Replay { changed } => {
            if edit.was != field(issue, change) && edit.at < changed {
                return None;
            }
            true
        }
    };
    match change {
        IssueChange::Note(text) => {
            let note =
                format!("{} {}: {}", date(edit.at), edit.by, text.trim());
            noting(lines, issue, &note, replaying)
        }
        IssueChange::Check { .. }
        | IssueChange::AddItem(_)
        | IssueChange::RemoveItem(_) => checking(lines, change, replaying),
        IssueChange::Title(_)
        | IssueChange::Body(_)
        | IssueChange::Labels(_)
        | IssueChange::Priority(_)
        | IssueChange::Status(_) => setting(lines, issue, change),
    }
}

fn setting(
    mut lines: Lines,
    issue: &Issue,
    change: &IssueChange,
) -> Option<Lines> {
    match change {
        IssueChange::Title(title) if issue.title != *title => {
            lines.set("title", &quoted(title));
        }
        IssueChange::Body(body) if issue.body != *body => lines.body(body),
        IssueChange::Labels(labels) if issue.labels != *labels => {
            lines.set("labels", &labels_value(labels));
        }
        IssueChange::Priority(priority) if issue.priority != *priority => {
            lines.set("priority", &quoted(priority.word()));
        }
        IssueChange::Status(status) if issue.status != *status => {
            lines.set("status", &quoted(status.word()));
        }
        _ => return None,
    }
    Some(lines)
}

fn checking(
    mut lines: Lines,
    change: &IssueChange,
    replaying: bool,
) -> Option<Lines> {
    match change {
        IssueChange::Check { item, done } => {
            let (at, found) = lines.item(item)?;
            let line =
                lines.rest.get_mut(at).filter(|_| found.done != *done)?;
            *line = item_line(&ChecklistItem {
                done: *done,
                text: found.text,
            });
        }
        IssueChange::AddItem(text) => {
            if replaying && lines.item(text).is_some() {
                return None;
            }
            let added = ChecklistItem {
                done: false,
                text: text.clone(),
            };
            lines.append(
                CHECKLIST,
                Some(NOTES),
                Vec::from([item_line(&added)]),
            );
        }
        IssueChange::RemoveItem(text) => {
            let (at, _) = lines.item(text)?;
            lines.rest.remove(at);
        }
        _ => return None,
    }
    Some(lines)
}

/// Adds the note, unless a replay finds it in the file already: two notes
/// alike made on purpose both stay.
fn noting(
    mut lines: Lines,
    issue: &Issue,
    note: &str,
    replaying: bool,
) -> Option<Lines> {
    if replaying && issue.notes.iter().any(|kept| kept == note) {
        return None;
    }
    lines.append(NOTES, None, note_lines(note));
    Some(lines)
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
    Ok(IssueFields {
        title,
        body: String::from(wanted.body.trim()),
        priority: wanted.priority,
        labels: normal_labels(&wanted.labels),
    })
}

/// Each label trimmed, once, and none empty.
fn normal_labels(labels: &[String]) -> Vec<String> {
    labels.iter().map(|label| label.trim()).fold(
        Vec::<String>::new(),
        |mut kept, label| {
            if !label.is_empty() && !kept.iter().any(|known| known == label) {
                kept.push(String::from(label));
            }
            kept
        },
    )
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
    /// The status the issue moves to now that work on it starts.
    pub moved: Option<IssueStatus>,
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
        moved: matches!(issue.status, IssueStatus::Open | IssueStatus::Ready)
            .then_some(IssueStatus::InProgress),
    })
}

/// The agent's first message: the issue as its file has it, and what to
/// do with it.
fn brief(project: &str, issue: &Issue) -> String {
    let checklist = issue
        .checklist
        .iter()
        .map(item_line)
        .collect::<Vec<String>>();
    let notes = issue
        .notes
        .iter()
        .flat_map(|note| note_lines(note))
        .collect::<Vec<String>>();
    let section = |heading: &str, lines: &[String]| {
        if lines.is_empty() {
            String::new()
        } else {
            format!("\n{heading}:\n{}\n", lines.join("\n"))
        }
    };
    format!(
        "You are working on the issue \"{title}\" in {project}. You are on a branch cut from {branch}; commit your work on it as you go.\n\n{body}\n{checklist}{notes}\nWork through what is left. When you are done, or stuck, say so, and sum up what you did and what is left.",
        title = issue.title,
        branch = issue.branch,
        body = if issue.body.is_empty() {
            "The issue has no description."
        } else {
            &issue.body
        },
        checklist = section("Its checklist", &checklist),
        notes = section("Its notes so far", &notes),
    )
}

/// Where an issue's branch stands in a checkout, before a session on it
/// starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// The checkout's branch, if it has one.
    pub local: Option<String>,
    /// The remote's, if the checkout could fetch it.
    pub remote: Option<String>,
    /// The checkout's `HEAD`, which a new issue's branch starts from.
    pub head: String,
    /// The local branch is an ancestor of the remote's.
    pub behind: bool,
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
}

impl Branch {
    /// The branch follows the remote's, which carries the issue from host
    /// to host, unless it has commits of its own: then it stays, and the
    /// session works on top of them.
    #[must_use]
    pub fn next(self) -> Move {
        match (self.local, self.remote) {
            (None, None) => Move::Set {
                commit: self.head,
                old: None,
            },
            (None, Some(remote)) => Move::Set {
                commit: remote,
                old: None,
            },
            (Some(local), Some(remote)) if local != remote && self.behind => {
                Move::Set {
                    commit: remote,
                    old: Some(local),
                }
            }
            (Some(_), _) => Move::Keep,
        }
    }
}

#[cfg(test)]
mod tests;
