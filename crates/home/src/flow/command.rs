//! The `home` command line as data: what its words ask for, and what doing
//! it to the home's issues writes and says.
//!
//! The shell reads the files, the clock and some randomness, and writes
//! what [`run`] returns. Every write, the command line's and the app's
//! alike, is made by [`write`], which reads it back against the flow
//! before it's made: a structural problem or a broken `error` rule refuses
//! it, and anything softer is written with a warning.

use alloc::string::ToString;

use proto::{
    ChecklistChange, Issue, IssueChange, IssueId, Item, NewIssue, Part,
    ProjectId,
};
use toml::Value;

use super::check::{Finding, Inputs, check, of_issue};
use super::eval::Move;
use super::graph::{File, Graph, one, read};
use super::{FieldType, Flow, Role, Severity, load};
use crate::issue::Lines;
use crate::prelude::*;
use crate::project::{Project, listed};

pub const USAGE: &str = "usage:
  home check
  home issue new <project> <kind> <title> [<field>=<value>...]
  home issue move <id> stage|status <to> [<reason>...]
  home issue set <id> <field>=<value>...
  home issue note <id> <text>...
  home issue body <id> <text>...
  home issue body <id> --file <path>
  home issue checklist <id> add <text>...
  home issue checklist <id> check|uncheck|remove <n>
A list's value is comma-separated: blocked_by=<id>,<id>.
A checklist's items are numbered from 1.
";

const NOTES: &str = "## Notes";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Check,
    New(NewIssue),
    /// Changes to the issue `id`, made in one write.
    Change {
        id: String,
        changes: Vec<IssueChange>,
    },
    /// Words that make no command, and why.
    Wrong(String),
    Usage,
}

/// What the shell knows that the core can't: when it is, the id a new
/// issue takes, and who is writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    /// Milliseconds since the Unix epoch.
    pub now: u64,
    pub id: IssueId,
    /// As the log signs it, `<node>·<harness>`.
    pub by: String,
}

/// An issue file to write, whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Write {
    pub project: ProjectId,
    pub stem: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub writes: Vec<Write>,
    pub output: String,
    pub failed: bool,
}

/// The home an edit is made in: its flow, its projects and every issue
/// file.
#[derive(Debug, Clone, Copy)]
pub struct Issues<'a> {
    pub flow: &'a Flow,
    pub projects: &'a [Project],
    pub files: &'a [File],
}

/// An edit to the home's issues.
#[derive(Debug, Clone, Copy)]
pub enum Edit<'a> {
    New(&'a NewIssue),
    /// To the issue whose file is called `id`.
    Change {
        id: &'a str,
        changes: &'a [IssueChange],
    },
}

/// The file an edit writes, `<project name>/<id>` that names it, and what
/// the flow warns of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub write: Write,
    pub subject: String,
    pub warnings: Vec<String>,
}

/// The id a new issue made at `now`, in milliseconds since the Unix epoch,
/// takes.
#[must_use]
pub const fn fresh(now: u64, random: u128) -> IssueId {
    IssueId(ulid::Ulid::from_parts(now, random).0)
}

/// The command `args` ask for, after the program's name.
#[must_use]
pub fn command(args: &[String]) -> Command {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let owned =
        |words: &[&str]| words.iter().map(|word| (*word).to_owned()).collect();
    let joined = |first: &str, more: &[&str]| {
        core::iter::once(first)
            .chain(more.iter().copied())
            .collect::<Vec<&str>>()
            .join(" ")
    };
    let changing = |id: &str, change| Command::Change {
        id: id.to_owned(),
        changes: Vec::from([change]),
    };
    match words.as_slice() {
        ["check"] => Command::Check,
        ["issue", "new", project, kind, title, fields @ ..] => {
            Command::New(NewIssue {
                project: (*project).to_owned(),
                kind: (*kind).to_owned(),
                title: (*title).to_owned(),
                fields: owned(fields),
            })
        }
        [
            "issue",
            "move",
            id,
            part @ ("stage" | "status"),
            to,
            reason @ ..,
        ] => changing(
            id,
            IssueChange::Move {
                part: if *part == "stage" {
                    Part::Stage
                } else {
                    Part::Status
                },
                to: (*to).to_owned(),
                reason: reason.join(" "),
            },
        ),
        ["issue", "set", id, first, more @ ..] => core::iter::once(first)
            .chain(more)
            .map(|pair| {
                pair_of(pair).map(|(field, value)| IssueChange::Set {
                    field: field.to_owned(),
                    value: value.to_owned(),
                })
            })
            .collect::<Result<Vec<IssueChange>, String>>()
            .map_or_else(Command::Wrong, |changes| Command::Change {
                id: (*id).to_owned(),
                changes,
            }),
        ["issue", "note", id, first, more @ ..] => {
            changing(id, IssueChange::Note(joined(first, more)))
        }
        ["issue", "body", id, first, more @ ..] => {
            changing(id, IssueChange::Body(joined(first, more)))
        }
        ["issue", "checklist", id, "add", first, more @ ..] => changing(
            id,
            IssueChange::Checklist(ChecklistChange::Add(joined(first, more))),
        ),
        [
            "issue",
            "checklist",
            id,
            action @ ("check" | "uncheck" | "remove"),
            number,
        ] => number
            .parse::<usize>()
            .ok()
            .and_then(|from_one| from_one.checked_sub(1))
            .map_or_else(
                || {
                    Command::Wrong(format!(
                        "{number} is not an item's number, counted from 1"
                    ))
                },
                |index| {
                    let item = Item { index, seen: None };
                    changing(
                        id,
                        IssueChange::Checklist(match *action {
                            "remove" => ChecklistChange::Remove(item),
                            done => ChecklistChange::Check {
                                item,
                                done: done == "check",
                            },
                        }),
                    )
                },
            ),
        _ => Command::Usage,
    }
}

/// What doing `command` to the home `inputs` holds writes and says.
#[must_use]
pub fn run(command: &Command, inputs: &Inputs, context: &Context) -> Outcome {
    let edit = match command {
        Command::Usage => return refused(String::from(USAGE)),
        Command::Wrong(reason) => return refused(format!("home: {reason}\n")),
        Command::Check => {
            let report = check(inputs);
            return Outcome {
                writes: Vec::new(),
                output: report.text(),
                failed: report.failed(),
            };
        }
        Command::New(new) => Edit::New(new),
        Command::Change { id, changes } => Edit::Change { id, changes },
    };
    let written = inputs
        .flow
        .as_deref()
        .map_err(Clone::clone)
        .and_then(load)
        .map_err(|problem| format!("docs/agents/flow.toml: {problem}"))
        .and_then(|flow| {
            let projects = listed(inputs.projects.clone()).projects;
            let home = Issues {
                flow: &flow,
                projects: &projects,
                files: &inputs.issues,
            };
            write(&home, edit, context)
        });
    match written {
        Ok(Written {
            write,
            subject,
            warnings,
        }) => Outcome {
            output: format!(
                "wrote {subject}\n{}",
                warnings
                    .iter()
                    .map(|warning| format!("warn   {subject}  {warning}\n"))
                    .collect::<Vec<String>>()
                    .concat()
            ),
            writes: Vec::from([write]),
            failed: false,
        },
        Err(reason) => refused(format!("home: {reason}\n")),
    }
}

const fn refused(output: String) -> Outcome {
    Outcome {
        writes: Vec::new(),
        output,
        failed: true,
    }
}

/// What `edit` writes to the home's issues, read back against the flow
/// among the others.
///
/// # Errors
///
/// Why the edit can't be made, or what the flow refuses it for.
pub fn write(
    home: &Issues<'_>,
    edit: Edit<'_>,
    context: &Context,
) -> Result<Written, String> {
    let sign = Signer {
        at: stamp(context.now),
        by: context.by.clone(),
    };
    let (file, moved) = match edit {
        Edit::New(new) => (opened(home, context, &sign, new)?, false),
        Edit::Change { id, changes } => {
            let file = home
                .files
                .iter()
                .find(|file| file.stem == id)
                .ok_or_else(|| format!("no issue is called {id}"))?;
            let moved = changes
                .iter()
                .any(|change| matches!(change, IssueChange::Move { .. }));
            (changed(home.flow, file, changes, &sign)?, moved)
        }
    };
    finished(home, file, moved)
}

/// Who writes a log line, and when.
struct Signer {
    at: String,
    by: String,
}

impl Signer {
    /// The log's lines for `event`: the first signed and dated, any more
    /// of `text` indented under it.
    fn lines(&self, event: &str, text: &str) -> Vec<String> {
        let mut said = text.split('\n');
        let first = said.next().unwrap_or_default();
        core::iter::once(format!("- {} {} {event}: {first}", self.at, self.by))
            .chain(said.map(|more| format!("  {more}")))
            .collect()
    }
}

/// The file read back among the others, refused on a structural problem
/// or a broken `error` rule, else written with any warning.
fn finished(
    home: &Issues<'_>,
    file: File,
    moved: bool,
) -> Result<Written, String> {
    let flow = home.flow;
    let mut files: Vec<File> = home
        .files
        .iter()
        .filter(|other| {
            other.project != file.project || other.stem != file.stem
        })
        .cloned()
        .collect();
    files.push(file.clone());
    let graph = read(flow, &files);
    let place = format!("{}/{}", file.project.0, file.stem);
    let name = home
        .projects
        .iter()
        .find(|project| project.id == file.project)
        .map_or_else(|| file.project.0.clone(), |project| project.name.clone());
    let subject = format!("{name}/{}", file.stem);
    let mut problems: Vec<String> = graph
        .problems
        .iter()
        .filter(|problem| problem.issue == place)
        .map(|problem| {
            problem.field.as_ref().map_or_else(
                || problem.message.clone(),
                |field| format!("{field}: {}", problem.message),
            )
        })
        .collect();
    let before = if moved {
        read(flow, home.files)
    } else {
        Graph::default()
    };
    let findings: Vec<Finding> = graph
        .issues
        .iter()
        .find(|issue| {
            issue.project == file.project && issue.id.to_string() == file.stem
        })
        .map_or_else(Vec::new, |issue| {
            let from = before.issues.iter().find(|old| {
                (&old.project, &old.id) == (&issue.project, &issue.id)
            });
            let moving = if moved {
                Move {
                    from,
                    to: Some(issue),
                }
            } else {
                Move::default()
            };
            of_issue(flow, &graph, issue, moving, &subject)
        });
    problems.extend(
        findings
            .iter()
            .filter(|finding| finding.severity == Severity::Error)
            .map(|finding| finding.message.clone()),
    );
    if !problems.is_empty() {
        return Err(format!("refused {subject}: {}", problems.join("; ")));
    }
    Ok(Written {
        warnings: findings
            .into_iter()
            .map(|finding| finding.message)
            .collect(),
        subject,
        write: Write {
            project: file.project,
            stem: file.stem,
            text: file.text,
        },
    })
}

fn opened(
    home: &Issues<'_>,
    context: &Context,
    sign: &Signer,
    new: &NewIssue,
) -> Result<File, String> {
    let NewIssue {
        project,
        kind,
        title: wanted,
        fields,
    } = new;
    let flow = home.flow;
    let at = &sign.at;
    let folder = home
        .projects
        .iter()
        .find(|known| known.name == *project || known.id.0 == *project)
        .map(|known| known.id.clone())
        .ok_or_else(|| format!("no project is called {project}"))?;
    let declared = flow
        .kind(kind)
        .ok_or_else(|| format!("no kind is called {kind}"))?;
    let title = needs(wanted, "an issue needs a title")?;
    let status = flow
        .statuses
        .iter()
        .find(|status| status.role == Role::Idle)
        .map_or_else(String::new, |status| status.name.clone());
    let id = context.id.to_string();
    let mut front = Vec::from([
        format!("id = {}", quoted(&id)),
        format!("title = {}", quoted(title)),
        format!("kind = {}", quoted(kind)),
        format!("status = {}", quoted(&status)),
    ]);
    for pair in fields {
        let (key, raw) = pair_of(pair)?;
        front.push(format!("{key} = {}", value(flow, kind, key, raw)?));
    }
    if flow
        .field("created")
        .is_some_and(|field| field.value == FieldType::Time)
    {
        front.push(format!("created = {at}"));
    }
    let sections = declared
        .sections
        .iter()
        .map(|section| format!("## {section}\n\n"))
        .collect::<Vec<String>>()
        .concat();
    Ok(File {
        project: folder,
        stem: id,
        text: format!(
            "+++\n{}\n+++\n\n{sections}{NOTES}\n\n{}\n",
            front.join("\n"),
            sign.lines("note", "opened.").concat()
        ),
    })
}

/// The file with `changes` made, in order, and a line in its log for
/// each move and note, and one for the fields set.
fn changed(
    flow: &Flow,
    file: &File,
    changes: &[IssueChange],
    sign: &Signer,
) -> Result<File, String> {
    let issue = one(flow, file).map_err(|_| {
        format!("{} has problems of its own; see home check", file.stem)
    })?;
    let mut lines = Lines::of(&file.text)?;
    let mut set = Vec::new();
    for change in changes {
        match change {
            IssueChange::Move { part, to, reason } => {
                shifted(flow, &issue, &mut lines, sign, (*part, to, reason))?;
            }
            IssueChange::Set { field, value } => {
                setting(flow, &issue.kind, &mut lines, field, value)?;
                set.push(format!("{field}={value}"));
            }
            IssueChange::Note(said) => {
                let text = needs(said, "a note needs text")?;
                lines.append(NOTES, None, sign.lines("note", text));
            }
            IssueChange::Body(text) => lines.body(text.trim()),
            IssueChange::Checklist(item) => {
                checklist(&issue, &mut lines, item)?;
            }
        }
    }
    if !set.is_empty() {
        lines.append(
            NOTES,
            None,
            sign.lines("note", &format!("set {}.", set.join(", "))),
        );
    }
    Ok(File {
        text: lines.text(),
        ..file.clone()
    })
}

fn shifted(
    flow: &Flow,
    current: &Issue,
    lines: &mut Lines,
    sign: &Signer,
    (part, to, reason): (Part, &str, &str),
) -> Result<(), String> {
    let (key, from, known, usual) = match part {
        Part::Stage => {
            let from = current.stage.clone();
            let usual = from
                .as_ref()
                .and_then(|stage| flow.stage(stage))
                .is_some_and(|stage| stage.next.iter().any(|next| next == to));
            ("stage", from, flow.stage(to).is_some(), usual)
        }
        Part::Status => (
            "status",
            Some(current.status.clone()),
            flow.status(to).is_some(),
            true,
        ),
    };
    if !known {
        return Err(format!("no {key} is called {to}"));
    }
    lines
        .set(key, &quoted(to))
        .ok_or_else(|| String::from("its front matter doesn't read"))?;
    let unusual = if usual { "" } else { " (unusual move)" };
    let event = format!("{key} {}→{to}", from.as_deref().unwrap_or("none"));
    lines.append(
        NOTES,
        None,
        sign.lines(&event, &format!("{reason}{unusual}")),
    );
    Ok(())
}

fn setting(
    flow: &Flow,
    kind: &str,
    lines: &mut Lines,
    key: &str,
    given: &str,
) -> Result<(), String> {
    if matches!(key, "id" | "kind" | "status" | "stage") {
        return Err(format!(
            "{key} isn't set this way; use home issue move for a stage or status"
        ));
    }
    let raw = given.trim();
    if key == "title" {
        needs(raw, "an issue needs a title")?;
    }
    lines
        .set(key, &value(flow, kind, key, raw)?)
        .ok_or_else(|| String::from("its front matter doesn't read"))
}

fn checklist(
    issue: &Issue,
    lines: &mut Lines,
    change: &ChecklistChange,
) -> Result<(), String> {
    let missing = || String::from("its checklist doesn't read");
    match change {
        ChecklistChange::Add(text) => {
            lines.add_item(needs(text, "a checklist item needs text")?);
            Ok(())
        }
        ChecklistChange::Check { item, done } => {
            lines.check(picked(issue, item)?, *done).ok_or_else(missing)
        }
        ChecklistChange::Remove(item) => {
            lines.remove_item(picked(issue, item)?).ok_or_else(missing)
        }
    }
}

/// The item's place in the issue's checklist, if it's still the one
/// picked.
fn picked(issue: &Issue, item: &Item) -> Result<usize, String> {
    if item
        .seen
        .as_ref()
        .is_some_and(|seen| *seen != issue.checklist)
    {
        return Err(String::from(
            "the checklist changed; select the item again",
        ));
    }
    if item.index < issue.checklist.len() {
        Ok(item.index)
    } else {
        Err(format!("the checklist has no item {}", item.index + 1))
    }
}

/// `text` trimmed, unless that leaves nothing.
fn needs<'a>(text: &'a str, missing: &str) -> Result<&'a str, String> {
    let kept = text.trim();
    if kept.is_empty() {
        Err(String::from(missing))
    } else {
        Ok(kept)
    }
}

fn pair_of(pair: &str) -> Result<(&str, &str), String> {
    pair.split_once('=')
        .ok_or_else(|| format!("{pair:?} is not <field>=<value>"))
}

/// `raw` as TOML for `key` on an issue of `kind`: a list where the field
/// or relation takes several, a boolean where it takes one, else text.
fn value(
    flow: &Flow,
    kind: &str,
    key: &str,
    raw: &str,
) -> Result<String, String> {
    let field = flow
        .fields
        .iter()
        .chain(
            flow.kind(kind)
                .into_iter()
                .flat_map(|declared| &declared.fields),
        )
        .find(|field| field.name == key);
    let many = match field.map(|declared| &declared.value) {
        Some(FieldType::Bool) => {
            return match raw {
                "true" | "false" => Ok(raw.to_owned()),
                _ => Err(format!("home: {key} is true or false\n")),
            };
        }
        Some(FieldType::Texts | FieldType::Refs(_)) => true,
        Some(_) => false,
        None => flow
            .relations
            .iter()
            .any(|relation| relation.name == key && relation.many),
    };
    Ok(if many {
        Value::Array(
            raw.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(|item| Value::String(item.to_owned()))
                .collect(),
        )
        .to_string()
    } else {
        quoted(raw)
    })
}

fn quoted(text: &str) -> String {
    Value::String(text.to_owned()).to_string()
}

/// `now` as the log writes a time, RFC 3339 in UTC to the second.
#[must_use]
pub fn stamp(now: u64) -> String {
    let seconds = now / 1000;
    let (year, month, day) = proto::civil(seconds / 86_400);
    let of_day = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}

#[cfg(test)]
mod tests;
