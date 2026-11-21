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
//! ([`Edit::replay`]). Then a field someone else changed since stays theirs
//! unless the edit is newer. Replay recognizes edits it already applied,
//! even when a separately submitted note has the same text.

use alloc::collections::BTreeMap;
use alloc::string::ToString;

use proto::{
    ChecklistAction, ChecklistItem, CommandId, Home, HostPath, Issue,
    IssueChange, IssueFields, IssueId, IssueStatus, Priority, ProjectId,
    Rejection,
};
use serde::{Deserialize, Serialize};
use toml::{Spanned, Table, Value};
use toml_parser::{Source, lexer::TokenKind};

use crate::prelude::*;

const CHECKLIST: &str = "## Checklist";
const NOTES: &str = "## Notes";

/// The issue's id as its file and branch name it: a lower-case ULID.
#[must_use]
fn name(id: IssueId) -> String {
    ulid::Ulid::from(id.0).to_string().to_lowercase()
}

/// The issue's file, from the home's root.
#[must_use]
fn file(project: &ProjectId, id: IssueId) -> String {
    format!("{}/{}.md", folder(project), name(id))
}

/// Where a project keeps its issues, from the home's root.
#[must_use]
fn folder(project: &ProjectId) -> String {
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

    fn has_marker(&self, marker: &str) -> bool {
        self.front.iter().any(|line| line == marker)
    }

    fn table(&self) -> Result<Table, String> {
        toml::from_str::<Table>(&self.front.join("\n"))
            .map_err(|err| format!("front matter: {err}"))
    }

    fn set(&mut self, key: &str, value: &str) -> Option<()> {
        let mut front = self.front.join("\n");
        let spans =
            toml::from_str::<BTreeMap<String, Spanned<Value>>>(&front).ok()?;
        if let Some(found) = spans.get(key) {
            let comments = array_with_comments(front.get(found.span())?, value);
            front.replace_range(
                found.span(),
                comments.as_deref().unwrap_or(value),
            );
        } else {
            let last_value = spans
                .values()
                .filter(|item| !item.get_ref().is_table())
                .map(|item| item.span().end)
                .max();
            if let Some(end) = last_value {
                let at = front
                    .get(end..)?
                    .find('\n')
                    .map_or(front.len(), |offset| end + offset);
                front.insert_str(at, &format!("\n{key} = {value}"));
            } else {
                front.insert_str(0, &format!("{key} = {value}\n"));
            }
        }
        self.front = front.split('\n').map(String::from).collect();
        Some(())
    }

    /// Where the body ends: at the checklist or the notes, whichever is
    /// first.
    fn body_end(&self) -> usize {
        markdown(&self.rest)
            .position(|(line, outside)| {
                outside && matches!(line.trim_end(), CHECKLIST | NOTES)
            })
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
        let mut headings = markdown(&self.rest)
            .enumerate()
            .skip(body_end)
            .filter(|(_, (line, outside))| *outside && line.starts_with("## "));
        let (start, _) =
            headings.find(|(_, (line, _))| line.trim_end() == heading)?;
        let end = headings.next().map_or(self.rest.len(), |(index, _)| index);
        Some((start, end))
    }

    fn items(&self) -> impl Iterator<Item = (usize, ChecklistItem)> + '_ {
        let mut checklist = false;
        markdown(&self.rest).enumerate().filter_map(
            move |(at, (line, outside))| {
                if outside && line.starts_with("## ") {
                    checklist = line.trim_end() == CHECKLIST;
                }
                (outside && checklist)
                    .then(|| item(line))
                    .flatten()
                    .map(|found| (at, found))
            },
        )
    }

    fn item(&self, text: &str) -> Option<(usize, ChecklistItem)> {
        self.items().find(|(_, found)| found.text == text)
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

fn array_with_comments(previous: &str, value: &str) -> Option<String> {
    let elements = value.strip_prefix('[')?.strip_suffix(']')?;
    let source = Source::new(previous);
    let mut comments = String::new();
    for token in source
        .lex()
        .filter(|token| token.kind() == TokenKind::Comment)
    {
        let comment = source.get(token)?;
        comments.push_str("    ");
        comments.push_str(comment.as_str());
        comments.push('\n');
    }
    if comments.is_empty() {
        return None;
    }
    if !elements.is_empty() {
        comments.push_str("    ");
        comments.push_str(elements);
        comments.push('\n');
    }
    Some(format!("[\n{comments}]"))
}

fn markdown(rest: &[String]) -> impl Iterator<Item = (&str, bool)> {
    let mut fence = None;
    rest.iter().map(move |line| {
        let previously_open = fence.is_some();
        if let Some((marker, width, suffix)) = fence_line(line) {
            match fence {
                Some((open, minimum))
                    if open == marker
                        && width >= minimum
                        && suffix.trim().is_empty() =>
                {
                    fence = None;
                }
                None if marker == b'~' || !suffix.contains('`') => {
                    fence = Some((marker, width));
                }
                _ => {}
            }
        }
        (line.as_str(), !previously_open && fence.is_none())
    })
}

fn fence_line(line: &str) -> Option<(u8, usize, &str)> {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return None;
    }
    let marker = *rest.as_bytes().first()?;
    let width = rest.bytes().take_while(|byte| *byte == marker).count();
    if width < 3 || !matches!(marker, b'`' | b'~') {
        return None;
    }
    Some((marker, width, rest.get(width..)?))
}

/// The issue in the file named `stem`.md.
///
/// # Errors
///
/// Why the file isn't an issue.
fn parse(stem: &str, text: &str) -> Result<Issue, String> {
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
    for (line, outside) in markdown(rest) {
        if !outside {
            if section.is_empty() {
                body.push(line);
            }
            continue;
        }
        match (line.trim_end(), section) {
            (heading @ (CHECKLIST | NOTES), _) => section = heading,
            (other, CHECKLIST | NOTES) if other.starts_with("## ") => {
                section = "other";
            }
            (_, "") => body.push(line),
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
fn render(issue: &Issue) -> Lines {
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
    lines
}

/// The issues of `project` in its folder's files, each `.md` file by its
/// name without the extension, by id; and why any file isn't one.
#[must_use]
fn listed(
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
fn locate(
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
fn opening(
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
fn changing(
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
        | IssueChange::Transition { .. }
        | IssueChange::Check { .. }
        | IssueChange::RemoveItem(_)
        | IssueChange::Checklist { .. }) => Ok(other),
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
fn message(edit: &EditData) -> String {
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
        Change::Issue(
            IssueChange::Status(status)
            | IssueChange::Transition { to: status, .. },
        ) => {
            format!("Move issue {id} to {}", status.word())
        }
        Change::Issue(
            IssueChange::Check { done: true, .. }
            | IssueChange::Checklist {
                action: ChecklistAction::SetDone(true),
                ..
            },
        ) => {
            format!("Check off an item of issue {id}")
        }
        Change::Issue(
            IssueChange::Check { done: false, .. }
            | IssueChange::Checklist {
                action: ChecklistAction::SetDone(false),
                ..
            },
        ) => {
            format!("Uncheck an item of issue {id}")
        }
        Change::Issue(IssueChange::AddItem(_)) => {
            format!("Add an item to issue {id}")
        }
        Change::Issue(
            IssueChange::RemoveItem(_)
            | IssueChange::Checklist {
                action: ChecklistAction::Remove,
                ..
            },
        ) => {
            format!("Remove an item from issue {id}")
        }
        Change::Issue(IssueChange::Note(_)) => format!("Note on issue {id}"),
    }
}

/// Who signs a note: the host, and the client that sent it.
#[must_use]
fn signer(host: &str, client: &str) -> String {
    format!("{host} · {client}")
}

/// A change a host made to an issue, kept until the remote has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edit {
    identity: EditIdentity,
    data: EditData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum EditIdentity {
    // The first two tags match the postcard Option<CommandId> encoding.
    Legacy,
    Command(CommandId),
    NoteOccurrence(usize),
    MigratedNote { ordinal: usize, marker: String },
}

/// The contents of an issue edit, also the original queue's disk format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditData {
    project: ProjectId,
    issue: IssueId,
    /// When the host took it, in seconds since the Unix epoch.
    at: u64,
    /// Who signs a note it adds.
    by: String,
    change: Change,
    /// The value the field it sets had when it was made, as [`was`] gives
    /// it, so a replay can tell whether someone else changed it since.
    was: Option<String>,
}

impl From<EditData> for Edit {
    fn from(data: EditData) -> Self {
        Self {
            identity: EditIdentity::Legacy,
            data,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Change {
    Create(IssueFields),
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
    Write(String),
    AlreadyRecorded,
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
fn was(current: Option<&str>, id: IssueId, change: &Change) -> Option<String> {
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
        IssueChange::Labels(_) => Some(labels_value(&issue.labels)),
        IssueChange::Priority(_) => Some(String::from(issue.priority.word())),
        IssueChange::Status(_) | IssueChange::Transition { .. } => {
            Some(String::from(issue.status.word()))
        }
        IssueChange::Check { .. }
        | IssueChange::AddItem(_)
        | IssueChange::RemoveItem(_)
        | IssueChange::Checklist { .. }
        | IssueChange::Note(_) => None,
    }
}

/// The edit on top of the issue's file as it is now, `None` when there is
/// none.
#[must_use]
fn apply(current: Option<&str>, edit: &Edit, mode: Mode) -> Applied {
    let data = &edit.data;
    let marker = match &edit.identity {
        EditIdentity::Command(id) => Some(edit_marker(*id)),
        EditIdentity::MigratedNote { marker, .. } => Some(marker.clone()),
        _ => None,
    };
    match (&data.change, current) {
        (Change::Create(_), Some(text)) => {
            match parse(&name(data.issue), text) {
                Ok(_) => Applied::AlreadyRecorded,
                Err(reason) => Applied::Unreadable(reason),
            }
        }
        (Change::Create(wanted), None) => fields(wanted)
            .map_or(Applied::Unchanged, |made| {
                marked(render(&opened(data, made)), marker, data.issue)
            }),
        (Change::Issue(_), None) => Applied::Gone,
        (Change::Issue(change), Some(text)) => {
            let read = parse(&name(data.issue), text)
                .and_then(|issue| Lines::of(text).map(|lines| (issue, lines)));
            match read {
                Err(reason) => Applied::Unreadable(reason),
                Ok((issue, lines)) => {
                    if marker
                        .as_deref()
                        .is_some_and(|expected| lines.has_marker(expected))
                    {
                        return Applied::AlreadyRecorded;
                    }
                    if let IssueChange::Checklist {
                        snapshot, index, ..
                    } = change
                        && (snapshot != &issue.checklist
                            || *index >= snapshot.len())
                    {
                        return match mode {
                            Mode::Record => Applied::Unreadable(String::from(
                                "The checklist changed. Select the item again.",
                            )),
                            Mode::Replay { .. } => {
                                marked(lines, marker, data.issue)
                            }
                        };
                    }
                    match changed(lines, &issue, change, (edit, mode)) {
                        Some(next) => marked(next, marker, data.issue),
                        None if marker.is_some()
                            && matches!(mode, Mode::Replay { .. }) =>
                        {
                            Lines::of(text)
                                .map_or_else(Applied::Unreadable, |lines| {
                                    marked(lines, marker, data.issue)
                                })
                        }
                        None => Applied::Unchanged,
                    }
                }
            }
        }
    }
}

fn edit_marker(id: CommandId) -> String {
    format!("# conductor-edit: {:032x}", id.0)
}

fn marked(mut lines: Lines, marker: Option<String>, issue: IssueId) -> Applied {
    lines.front.extend(marker);
    let text = lines.text();
    match parse(&name(issue), &text) {
        Ok(_) => Applied::Write(text),
        Err(reason) => Applied::Unreadable(reason),
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
    let data = &edit.data;
    let legacy_replay = match mode {
        Mode::Record => false,
        Mode::Replay { changed } => {
            if data.was != field(issue, change) && data.at < changed {
                return None;
            }
            !matches!(edit.identity, EditIdentity::Command(_))
        }
    };
    match change {
        IssueChange::Note(text) => {
            let note = data.rendered_note(text);
            let replay_occurrence = match (&edit.identity, mode) {
                (EditIdentity::Legacy, Mode::Replay { .. }) => Some(1),
                (
                    EditIdentity::NoteOccurrence(ordinal)
                    | EditIdentity::MigratedNote { ordinal, .. },
                    Mode::Replay { .. },
                ) => Some(*ordinal),
                _ => None,
            };
            noting(lines, issue, &note, replay_occurrence)
        }
        IssueChange::Check { .. }
        | IssueChange::AddItem(_)
        | IssueChange::RemoveItem(_)
        | IssueChange::Checklist { .. } => {
            checking(lines, change, legacy_replay)
        }
        IssueChange::Title(_)
        | IssueChange::Body(_)
        | IssueChange::Labels(_)
        | IssueChange::Priority(_)
        | IssueChange::Status(_)
        | IssueChange::Transition { .. } => setting(lines, issue, change),
    }
}

fn setting(
    mut lines: Lines,
    issue: &Issue,
    change: &IssueChange,
) -> Option<Lines> {
    match change {
        IssueChange::Title(title) if issue.title != *title => {
            lines.set("title", &quoted(title))?;
        }
        IssueChange::Body(body) if issue.body != *body => lines.body(body),
        IssueChange::Labels(labels) if issue.labels != *labels => {
            lines.set("labels", &labels_value(labels))?;
        }
        IssueChange::Priority(priority) if issue.priority != *priority => {
            lines.set("priority", &quoted(priority.word()))?;
        }
        IssueChange::Status(status) if issue.status != *status => {
            lines.set("status", &quoted(status.word()))?;
        }
        IssueChange::Transition { from, to } if issue.status == *from => {
            lines.set("status", &quoted(to.word()))?;
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
        IssueChange::Checklist { index, action, .. } => {
            let (at, found) = lines.items().nth(*index)?;
            match action {
                ChecklistAction::SetDone(done) if found.done != *done => {
                    *lines.rest.get_mut(at)? = item_line(&ChecklistItem {
                        done: *done,
                        text: found.text,
                    });
                }
                ChecklistAction::SetDone(_) => return None,
                ChecklistAction::Remove => {
                    lines.rest.remove(at);
                }
            }
        }
        _ => return None,
    }
    Some(lines)
}

fn noting(
    mut lines: Lines,
    issue: &Issue,
    note: &str,
    occurrence: Option<usize>,
) -> Option<Lines> {
    if occurrence.is_some_and(|needed| {
        issue
            .notes
            .iter()
            .filter(|kept| kept.as_str() == note)
            .count()
            >= needed
    }) {
        return None;
    }
    lines.append(NOTES, None, note_lines(note));
    Some(lines)
}

fn opened(edit: &EditData, made: IssueFields) -> Issue {
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
fn fields(wanted: &IssueFields) -> Result<IssueFields, String> {
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

/// The status an issue moves to once a session on it has started: work
/// on an `open` or `ready` one is in progress.
#[must_use]
const fn started(status: IssueStatus) -> Option<IssueStatus> {
    match status {
        IssueStatus::Open | IssueStatus::Ready => Some(IssueStatus::InProgress),
        IssueStatus::InProgress
        | IssueStatus::Review
        | IssueStatus::Done
        | IssueStatus::Dropped => None,
    }
}

/// The status an issue moves to once a session on it has finished: one
/// still being worked on is up for review when its branch gained commits.
#[must_use]
const fn finished(status: IssueStatus, landed: u64) -> Option<IssueStatus> {
    match (status, landed) {
        (_, 0)
        | (IssueStatus::Review | IssueStatus::Done | IssueStatus::Dropped, _) => {
            None
        }
        (
            IssueStatus::Open | IssueStatus::Ready | IssueStatus::InProgress,
            _,
        ) => Some(IssueStatus::Review),
    }
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
    format!("refs/heads/{}", named.unwrap_or_else(|| branch_of(id)))
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
