//! The `home` command line as data: what its words ask for, and what doing
//! it to the home's issues writes and says.
//!
//! The shell reads the files, the clock and some randomness, and writes
//! what [`run`] returns. Every write is read back against the flow before
//! it's made: a structural problem or a broken `error` rule refuses it,
//! and anything softer is written with a warning.

use alloc::string::ToString;

use proto::ProjectId;
use toml::Value;

use super::check::{Finding, Inputs, check, of_issue};
use super::eval::Move;
use super::graph::{File, Graph, read};
use super::{FieldType, Flow, Role, Severity, load};
use crate::issue::Lines;
use crate::prelude::*;
use crate::project::listed;

pub const USAGE: &str = "usage:
  home check
  home issue new <project> <kind> <title> [<field>=<value>...]
  home issue move <id> stage|status <to> [<reason>...]
  home issue set <id> <field>=<value>...
  home issue note <id> <text>...
A list's value is comma-separated: blocked_by=<id>,<id>.
";

const NOTES: &str = "## Notes";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Check,
    New(New),
    Move {
        id: String,
        part: Part,
        to: String,
        reason: String,
    },
    Set {
        id: String,
        fields: Vec<String>,
    },
    Note {
        id: String,
        text: String,
    },
    Usage,
}

/// A new issue: its project, by name or id, its kind and title, and its
/// fields as `<field>=<value>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct New {
    pub project: String,
    pub kind: String,
    pub title: String,
    pub fields: Vec<String>,
}

/// What a move changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Stage,
    Status,
}

/// What the shell knows that the core can't: when it is, some randomness
/// for a new id, and who is writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    /// Milliseconds since the Unix epoch.
    pub now: u64,
    pub random: u128,
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

/// The command `args` ask for, after the program's name.
#[must_use]
pub fn command(args: &[String]) -> Command {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let owned =
        |words: &[&str]| words.iter().map(|word| (*word).to_owned()).collect();
    match words.as_slice() {
        ["check"] => Command::Check,
        ["issue", "new", project, kind, title, fields @ ..] => {
            Command::New(New {
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
        ] => Command::Move {
            id: (*id).to_owned(),
            part: if *part == "stage" {
                Part::Stage
            } else {
                Part::Status
            },
            to: (*to).to_owned(),
            reason: reason.join(" "),
        },
        ["issue", "set", id, first, more @ ..] => Command::Set {
            id: (*id).to_owned(),
            fields: core::iter::once(*first)
                .chain(more.iter().copied())
                .map(ToOwned::to_owned)
                .collect(),
        },
        ["issue", "note", id, first, more @ ..] => Command::Note {
            id: (*id).to_owned(),
            text: core::iter::once(*first)
                .chain(more.iter().copied())
                .collect::<Vec<&str>>()
                .join(" "),
        },
        _ => Command::Usage,
    }
}

/// What doing `command` to the home `inputs` holds writes and says.
#[must_use]
pub fn run(command: &Command, inputs: &Inputs, context: &Context) -> Outcome {
    match command {
        Command::Usage => refused(String::from(USAGE)),
        Command::Check => {
            let report = check(inputs);
            Outcome {
                writes: Vec::new(),
                output: report.text(),
                failed: report.failed(),
            }
        }
        edit => edited(edit, inputs, context).unwrap_or_else(refused),
    }
}

const fn refused(output: String) -> Outcome {
    Outcome {
        writes: Vec::new(),
        output,
        failed: true,
    }
}

fn edited(
    command: &Command,
    inputs: &Inputs,
    context: &Context,
) -> Result<Outcome, String> {
    let flow = inputs
        .flow
        .as_deref()
        .map_err(Clone::clone)
        .and_then(load)
        .map_err(|problem| {
            format!("home: docs/agents/flow.toml: {problem}\n")
        })?;
    let sign = Signer {
        at: stamp(context.now),
        by: context.by.clone(),
    };
    let (file, moved) = match command {
        Command::New(new) => {
            (opened(&flow, inputs, context, &sign, new)?, false)
        }
        Command::Move {
            id,
            part,
            to,
            reason,
        } => {
            let file = found(inputs, id)?;
            (
                shifted(&flow, inputs, file, &sign, (*part, to, reason))?,
                true,
            )
        }
        Command::Set { id, fields } => {
            let file = found(inputs, id)?;
            (changed(&flow, file, fields, &sign)?, false)
        }
        Command::Note { id, text } => {
            let file = found(inputs, id)?;
            (logged(file, &sign.line("note", text))?, false)
        }
        Command::Check | Command::Usage => return Err(String::from(USAGE)),
    };
    finished(&flow, inputs, file, moved)
}

/// Who writes a log line, and when.
struct Signer {
    at: String,
    by: String,
}

impl Signer {
    fn line(&self, event: &str, text: &str) -> String {
        format!("- {} {} {event}: {text}", self.at, self.by)
    }
}

fn found<'a>(inputs: &'a Inputs, id: &str) -> Result<&'a File, String> {
    inputs
        .issues
        .iter()
        .find(|file| file.stem == id)
        .ok_or_else(|| format!("home: no issue is called {id}\n"))
}

/// The file read back among the others, refused on a structural problem
/// or a broken `error` rule, else written with any warning.
fn finished(
    flow: &Flow,
    inputs: &Inputs,
    file: File,
    moved: bool,
) -> Result<Outcome, String> {
    let mut files: Vec<File> = inputs
        .issues
        .iter()
        .filter(|other| {
            other.project != file.project || other.stem != file.stem
        })
        .cloned()
        .collect();
    files.push(file.clone());
    let graph = read(flow, &files);
    let place = format!("{}/{}", file.project.0, file.stem);
    let name = listed(inputs.projects.clone())
        .projects
        .into_iter()
        .find(|project| project.id == file.project)
        .map_or_else(|| file.project.0.clone(), |project| project.name);
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
        read(flow, &inputs.issues)
    } else {
        Graph::default()
    };
    let findings: Vec<Finding> = graph
        .issues
        .iter()
        .find(|issue| {
            (&issue.project, &issue.id) == (&file.project, &file.stem)
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
        return Err(format!(
            "home: refused {subject}: {}\n",
            problems.join("; ")
        ));
    }
    let warnings = findings
        .iter()
        .map(|finding| format!("warn   {subject}  {}\n", finding.message))
        .collect::<Vec<String>>()
        .concat();
    Ok(Outcome {
        output: format!("wrote {subject}\n{warnings}"),
        writes: Vec::from([Write {
            project: file.project,
            stem: file.stem,
            text: file.text,
        }]),
        failed: false,
    })
}

fn opened(
    flow: &Flow,
    inputs: &Inputs,
    context: &Context,
    sign: &Signer,
    new: &New,
) -> Result<File, String> {
    let New {
        project,
        kind,
        title,
        fields,
    } = new;
    let at = &sign.at;
    let folder = listed(inputs.projects.clone())
        .projects
        .into_iter()
        .find(|known| known.name == *project || known.id.0 == *project)
        .map(|known| known.id)
        .ok_or_else(|| format!("home: no project is called {project}\n"))?;
    let declared = flow
        .kind(kind)
        .ok_or_else(|| format!("home: no kind is called {kind}\n"))?;
    let status = flow
        .statuses
        .iter()
        .find(|status| status.role == Role::Idle)
        .map_or_else(String::new, |status| status.name.clone());
    let id = ulid::Ulid::from_parts(context.now, context.random)
        .to_string()
        .to_lowercase();
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
            "+++\n{}\n+++\n\n{sections}{NOTES}\n\n- {at} {} note: opened.\n",
            front.join("\n"),
            sign.by
        ),
    })
}

fn shifted(
    flow: &Flow,
    inputs: &Inputs,
    file: &File,
    sign: &Signer,
    (part, to, reason): (Part, &str, &str),
) -> Result<File, String> {
    let current = read(flow, &inputs.issues)
        .issues
        .into_iter()
        .find(|issue| {
            (&issue.project, &issue.id) == (&file.project, &file.stem)
        })
        .ok_or_else(|| {
            format!(
                "home: {} has problems of its own; see home check\n",
                file.stem
            )
        })?;
    let (key, from, known, usual) = match part {
        Part::Stage => {
            let from = current.stage;
            let usual = from
                .as_ref()
                .and_then(|stage| flow.stage(stage))
                .is_some_and(|stage| stage.next.iter().any(|next| next == to));
            ("stage", from, flow.stage(to).is_some(), usual)
        }
        Part::Status => (
            "status",
            Some(current.status),
            flow.status(to).is_some(),
            true,
        ),
    };
    if !known {
        return Err(format!("home: no {key} is called {to}\n"));
    }
    let mut lines = Lines::of(&file.text)
        .map_err(|problem| format!("home: {problem}\n"))?;
    lines
        .set(key, &quoted(to))
        .ok_or_else(|| String::from("home: its front matter doesn't read\n"))?;
    let unusual = if usual { "" } else { " (unusual move)" };
    let event = format!("{key} {}→{to}", from.as_deref().unwrap_or("none"));
    lines.append(
        NOTES,
        None,
        Vec::from([sign.line(&event, &format!("{reason}{unusual}"))]),
    );
    Ok(File {
        text: lines.text(),
        ..file.clone()
    })
}

fn changed(
    flow: &Flow,
    file: &File,
    fields: &[String],
    sign: &Signer,
) -> Result<File, String> {
    let kind = read(flow, core::slice::from_ref(file))
        .issues
        .first()
        .map_or_else(|| flow.default_kind.clone(), |issue| issue.kind.clone());
    let mut lines = Lines::of(&file.text)
        .map_err(|problem| format!("home: {problem}\n"))?;
    for pair in fields {
        let (key, raw) = pair_of(pair)?;
        if matches!(key, "id" | "kind" | "status" | "stage") {
            return Err(format!(
                "home: {key} isn't set this way; use home issue move for a stage or status\n"
            ));
        }
        lines
            .set(key, &value(flow, &kind, key, raw)?)
            .ok_or_else(|| {
                String::from("home: its front matter doesn't read\n")
            })?;
    }
    lines.append(
        NOTES,
        None,
        Vec::from([sign.line("note", &format!("set {}.", fields.join(", ")))]),
    );
    Ok(File {
        text: lines.text(),
        ..file.clone()
    })
}

fn logged(file: &File, line: &str) -> Result<File, String> {
    let mut lines = Lines::of(&file.text)
        .map_err(|problem| format!("home: {problem}\n"))?;
    lines.append(NOTES, None, Vec::from([line.to_owned()]));
    Ok(File {
        text: lines.text(),
        ..file.clone()
    })
}

fn pair_of(pair: &str) -> Result<(&str, &str), String> {
    pair.split_once('=')
        .ok_or_else(|| format!("home: {pair:?} is not <field>=<value>\n"))
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
