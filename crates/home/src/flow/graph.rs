//! Every project's issues read as the flow declares them, in one graph.
//!
//! An issue's front matter holds its id, title, kind, status and stage,
//! the fields its kind has, and the relations that start from its kind.
//! Anything else is a structural problem, as is a reference to no issue,
//! a target of a kind the relation doesn't allow, and a cycle where the
//! relation forbids one. Ids are global, so a relation may cross projects.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::ToString;

use proto::ProjectId;
use toml::{Table, Value};

use super::{Field, FieldType, Flow, Relation};
use crate::prelude::*;
use proto::ChecklistItem;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub id: String,
    pub project: ProjectId,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub stage: Option<String>,
    /// The declared fields it gives, by name.
    pub fields: BTreeMap<String, Datum>,
    /// The issues each relation from it points to, by relation.
    pub links: BTreeMap<String, Vec<String>>,
    /// The body's sections in order, the text before the first heading
    /// as one with no heading; the checklist and the log aren't among them.
    pub sections: Vec<Section>,
    /// The items of the `## Checklist` section.
    pub checklist: Vec<ChecklistItem>,
    pub log: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Empty for the text before the first heading.
    pub heading: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Datum {
    Text(String),
    Texts(Vec<String>),
    Bool(bool),
}

/// A line of an issue's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// When and by whom, when the line says so.
    pub at: Option<String>,
    pub by: Option<String>,
    pub event: Event,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Note,
    Stage { from: String, to: String },
    Status { from: String, to: String },
}

/// Something structurally wrong with an issue, where a field is the cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// `<project>/<stem>`.
    pub issue: String,
    pub field: Option<String>,
    pub message: String,
}

/// The issues `files` hold, as `flow` declares them, and what's
/// structurally wrong with any of them.
#[must_use]
pub fn read(flow: &Flow, files: &[File]) -> Graph {
    let mut issues = Vec::new();
    let mut problems = Vec::new();
    let mut unread = BTreeSet::new();
    for file in files {
        match issue(flow, file) {
            Ok(read_issue) => issues.push(read_issue),
            Err(problem) => {
                unread.insert(file.stem.as_str());
                problems.push(problem);
            }
        }
    }
    issues.sort_by(|one, other| one.id.cmp(&other.id));
    problems.extend(references(flow, &issues, &unread));
    problems.extend(cycles(flow, &issues));
    Graph { issues, problems }
}

fn issue(flow: &Flow, file: &File) -> Result<Issue, Problem> {
    let place = format!("{}/{}", file.project.0, file.stem);
    let fail = |field: Option<&str>, message: String| Problem {
        issue: place.clone(),
        field: field.map(ToOwned::to_owned),
        message,
    };
    let (front, body) = split(&file.text).ok_or_else(|| {
        fail(None, String::from("it has no +++ front matter"))
    })?;
    let mut table = toml::from_str::<Table>(front)
        .map_err(|err| fail(None, format!("its front matter: {err}")))?;
    let id = text(&mut table, "id")
        .filter(|id| *id == file.stem && is_id(id))
        .ok_or_else(|| {
            fail(
                Some("id"),
                String::from("is not its file's name, a lower-case ULID"),
            )
        })?;
    let title = text(&mut table, "title")
        .ok_or_else(|| fail(Some("title"), String::from("is missing")))?;
    let kind =
        text(&mut table, "kind").unwrap_or_else(|| flow.default_kind.clone());
    let declared = flow.kind(&kind).ok_or_else(|| {
        fail(Some("kind"), format!("no kind is called {kind:?}"))
    })?;
    let status = text(&mut table, "status")
        .ok_or_else(|| fail(Some("status"), String::from("is missing")))?;
    if flow.status(&status).is_none() {
        return Err(fail(
            Some("status"),
            format!("no status is called {status:?}"),
        ));
    }
    let stage = text(&mut table, "stage");
    if let Some(name) = stage.as_ref().filter(|name| flow.stage(name).is_none())
    {
        return Err(fail(
            Some("stage"),
            format!("no stage is called {name:?}"),
        ));
    }
    let mut fields = BTreeMap::new();
    let mut links = BTreeMap::new();
    for (key, value) in table {
        if let Some(field) = flow
            .fields
            .iter()
            .chain(&declared.fields)
            .find(|field| field.name == key)
        {
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
    let (sections, checklist, log) = body_of(body);
    Ok(Issue {
        id,
        project: file.project.clone(),
        title,
        kind,
        status,
        stage,
        fields,
        links,
        sections,
        checklist,
        log,
    })
}

/// The front matter between the `+++` lines, and the body after them.
fn split(text: &str) -> Option<(&str, &str)> {
    let rest = text.trim_start_matches('\u{feff}').strip_prefix("+++\n")?;
    let end = rest.find("\n+++")?;
    let front = rest.get(..=end)?;
    let after = rest.get(end + 4..)?;
    Some((front, after.strip_prefix('\n').unwrap_or(after)))
}

fn is_id(id: &str) -> bool {
    ulid::Ulid::from_string(id)
        .is_ok_and(|ulid| ulid.to_string().to_lowercase() == id)
}

fn text(table: &mut Table, key: &str) -> Option<String> {
    match table.remove(key) {
        Some(Value::String(text)) => Some(text),
        _ => None,
    }
}

fn datum(field: &Field, value: Value) -> Result<Datum, String> {
    match (&field.value, value) {
        (FieldType::Enum(words), Value::String(word)) => {
            if words.contains(&word) {
                Ok(Datum::Text(word))
            } else {
                Err(format!("{word:?} is not one of {}", words.join(", ")))
            }
        }
        (FieldType::Text | FieldType::Ref(_), Value::String(word)) => {
            Ok(Datum::Text(word))
        }
        (FieldType::Time, Value::Datetime(at)) => {
            Ok(Datum::Text(at.to_string()))
        }
        (FieldType::Time, Value::String(at)) => Ok(Datum::Text(at)),
        (FieldType::Bool, Value::Boolean(flag)) => Ok(Datum::Bool(flag)),
        (FieldType::Texts | FieldType::Refs(_), Value::Array(values)) => values
            .into_iter()
            .map(|value| match value {
                Value::String(word) => Ok(word),
                _ => Err(String::from("is not a list of text")),
            })
            .collect::<Result<Vec<String>, String>>()
            .map(Datum::Texts),
        _ => Err(String::from("is not a value of its type")),
    }
}

fn targets(relation: &Relation, value: Value) -> Result<Vec<String>, String> {
    match (relation.many, value) {
        (_, Value::String(id)) => Ok(Vec::from([id])),
        (true, Value::Array(values)) => values
            .into_iter()
            .map(|value| match value {
                Value::String(id) => Ok(id),
                _ => Err(String::from("is not a list of ids")),
            })
            .collect(),
        (false, Value::Array(_)) => {
            Err(String::from("points to one issue, not a list"))
        }
        _ => Err(String::from("is not an id")),
    }
}

/// The body's sections, the checklist's items and the log, reading
/// headings only outside code fences.
fn body_of(body: &str) -> (Vec<Section>, Vec<ChecklistItem>, Vec<Entry>) {
    let lines: Vec<String> = body.lines().map(ToOwned::to_owned).collect();
    let mut sections: Vec<Section> = Vec::new();
    let mut checklist = Vec::new();
    let mut log: Vec<Entry> = Vec::new();
    let mut part = Part::Section;
    for (line, outside) in markdown(&lines) {
        match (line.strip_prefix("## ").filter(|_| outside), part) {
            (Some(heading), _) => {
                part = match heading.trim() {
                    "Notes" => Part::Log,
                    "Checklist" => Part::Checklist,
                    named => {
                        sections.push(Section {
                            heading: named.to_owned(),
                            text: String::new(),
                        });
                        Part::Section
                    }
                };
            }
            (None, Part::Log) => logged(&mut log, line),
            (None, Part::Checklist) => checklist.extend(item(line)),
            (None, Part::Section) => written(&mut sections, line),
        }
    }
    for section in &mut sections {
        section.text = section.text.trim().to_owned();
    }
    sections.retain(|section| {
        !section.heading.is_empty() || !section.text.is_empty()
    });
    (sections, checklist, log)
}

#[derive(Debug, Clone, Copy)]
enum Part {
    Section,
    Checklist,
    Log,
}

fn written(sections: &mut Vec<Section>, line: &str) {
    if sections.is_empty() {
        sections.push(Section {
            heading: String::new(),
            text: String::new(),
        });
    }
    if let Some(section) = sections.last_mut() {
        section.text.push_str(line);
        section.text.push('\n');
    }
}

/// A log line, or the continuation of the last one when indented by two
/// spaces.
fn logged(log: &mut Vec<Entry>, line: &str) {
    if let Some(entry) = line.strip_prefix("- ") {
        log.push(entry_of(entry));
    } else if let (Some(more), Some(last)) =
        (line.strip_prefix("  "), log.last_mut())
    {
        last.text.push('\n');
        last.text.push_str(more);
    }
}

pub(crate) fn markdown(rest: &[String]) -> impl Iterator<Item = (&str, bool)> {
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

pub(crate) fn item(line: &str) -> Option<ChecklistItem> {
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

/// `<time> <who> <event>: <text>`, where the event is `note`,
/// `stage <a>→<b>` or `status <a>→<b>`; any other line is a note as written.
fn entry_of(line: &str) -> Entry {
    let structured = line.split_once(": ").and_then(|(head, text)| {
        let mut words = head.split(' ');
        let at = words.next()?;
        let by = words.next()?;
        let event = match (words.next()?, words.next(), words.next()) {
            ("note", None, _) => Event::Note,
            (name @ ("stage" | "status"), Some(change), None) => {
                let (before, after) = change.split_once('→')?;
                let (from, to) = (before.to_owned(), after.to_owned());
                if name == "stage" {
                    Event::Stage { from, to }
                } else {
                    Event::Status { from, to }
                }
            }
            _ => return None,
        };
        at.contains('T').then(|| Entry {
            at: Some(at.to_owned()),
            by: Some(by.to_owned()),
            event,
            text: text.to_owned(),
        })
    });
    structured.unwrap_or_else(|| Entry {
        at: None,
        by: None,
        event: Event::Note,
        text: line.to_owned(),
    })
}

/// Each reference to no issue, or to one of a kind its relation doesn't
/// allow. An issue whose file has a problem of its own still exists, so a
/// reference to it is left to that problem.
fn references(
    flow: &Flow,
    issues: &[Issue],
    unread: &BTreeSet<&str>,
) -> Vec<Problem> {
    let by_id: BTreeMap<&str, &Issue> = issues
        .iter()
        .map(|issue| (issue.id.as_str(), issue))
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
                let message = match by_id.get(id.as_str()) {
                    None if unread.contains(id.as_str()) => None,
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
                    issue: format!("{}/{}", issue.project.0, issue.id),
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
    let projects: BTreeMap<&str, &str> = issues
        .iter()
        .map(|issue| (issue.id.as_str(), issue.project.0.as_str()))
        .collect();
    let mut problems = Vec::new();
    for relation in flow.relations.iter().filter(|relation| !relation.cycles) {
        let edges: BTreeMap<&str, &[String]> = issues
            .iter()
            .filter_map(|issue| {
                issue
                    .links
                    .get(&relation.name)
                    .map(|ids| (issue.id.as_str(), ids.as_slice()))
            })
            .collect();
        let mut done: BTreeSet<&str> = BTreeSet::new();
        for issue in issues {
            let mut path = Vec::new();
            if let Some(cycle) =
                walk(&edges, issue.id.as_str(), &mut path, &mut done)
            {
                let message =
                    format!("is part of a cycle: {}", cycle.join(" → "));
                let members = cycle.iter().skip(1).filter_map(|id| {
                    projects.get(id.as_str()).map(|project| (*project, id))
                });
                problems.extend(members.map(|(project, id)| Problem {
                    issue: format!("{project}/{id}"),
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
fn walk<'a>(
    edges: &BTreeMap<&'a str, &'a [String]>,
    id: &'a str,
    path: &mut Vec<&'a str>,
    done: &mut BTreeSet<&'a str>,
) -> Option<Vec<String>> {
    if let Some(start) = path.iter().position(|seen| *seen == id) {
        let mut cycle: Vec<String> = path
            .get(start..)?
            .iter()
            .map(|seen| (*seen).to_owned())
            .collect();
        cycle.push(id.to_owned());
        return Some(cycle);
    }
    if !done.insert(id) {
        return None;
    }
    path.push(id);
    let found = edges
        .get(id)
        .into_iter()
        .flat_map(|ids| ids.iter())
        .find_map(|next| walk(edges, next, path, done));
    path.pop();
    found
}

#[cfg(test)]
mod tests;
