//! The `home` command line as data: what its words ask for, and what doing
//! it to the home's issues writes and says.
//!
//! The shell reads the files, the clock and some randomness, and writes
//! what [`run`] returns. Every write, the command line's and the app's
//! alike, is made by [`write()`], which reads it back against the flow
//! before it's made: a structural problem or a broken `error` rule refuses
//! it, and anything softer is written with a warning.

use alloc::string::ToString;

use crate::flow::{FieldRole, FieldType, Flow, Severity};
use crate::issue::{
    ChecklistChange, Datum, Entry, Issue, IssueChange, Item, LogEvent,
    NewIssue, Part,
};
use crate::{Draws, IssueId, ProjectId};
use serde::{Deserialize, Serialize};
use toml::Value;

use super::PATH;
use super::check::{Finding, check, of_issue};
use super::eval::Move;
use super::file::{Document, ID, KIND, STAGE, STATUS, TITLE};
use super::graph::{File, parsed};
use super::role::opening;
use super::snapshot::Snapshot;
use crate::prelude::*;

/// What `home` says alone: every command and what it means.
pub const USAGE: &str = "usage: home <command>, from anywhere in the home

  home check
      Checks every project and issue against the flow; fails only on errors.
  home issue new <project> <kind> <title> [<field>=<value>...]
      Opens an issue of the kind in the project, named by its name or id,
      with the kind's sections and a fresh id, which it prints. Give the
      title as one argument.
  home issue move <id> stage|status <to> [--signed-off] [<reason>...]
      Moves the stage or status, the only way to change either, and logs
      the move and its reason, marking a move off the stage's usual next
      ones as unusual. Entering a stage with a sign-off takes --signed-off.
  home issue set <id> <field>=<value>...
      Sets any other field or relation, title, parent and blocked_by among
      them. A list's value is comma-separated: blocked_by=<id>,<id>.
  home issue note <id> <text>...
      Appends a note to the log.
  home issue body <id> <text>...
  home issue body <id> --file <path>
      Replaces the body, everything above its checklist and notes, with the
      text or the file's; give a body of several lines or sections by file.
  home issue checklist <id> add <text>...
      Adds an item to the checklist.
  home issue checklist <id> check|uncheck|remove <n>
      Checks, unchecks or removes the checklist's nth item, from 1.
  home repos
      Shows each project's checkouts on this machine and their state.
  home repos sync
      Clones each project with a remote and no checkout here.
  home repos scan
      Registers each repository here that matches no project, under a fresh
      id, and rewrites the project list in AGENTS.md.
  home skills
      Installs the home's skills for every harness, at user level, keeping
      any skill installed some other way.

Each write signs and dates the log and is read back against the flow: a
structural problem or a broken error rule refuses it, and anything softer
is written with a warning.
";

/// What the command line's words ask for: a command on the home's issues,
/// or one the shell does in the home's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    Flow(Command),
    Local(Local),
}

/// What the shell does in the home's folder, among its repositories and
/// skills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Local {
    /// `home repos`: each project's checkouts here and their state.
    Status,
    /// `home repos sync`: clones each project not checked out here.
    Sync,
    /// `home repos scan`: registers each repository no project has.
    Scan,
    /// `home skills`: installs the home's skills for each harness.
    Skills,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Check,
    Edit(Edit),
    /// The issue's body becomes the text of the file at `path`, from where
    /// the command runs, once the shell has read it.
    BodyFrom {
        id: IssueId,
        path: String,
    },
    /// Words that make no command, and why.
    Wrong(String),
    Usage,
}

impl Command {
    /// The file the shell reads before running it.
    #[must_use]
    pub fn file(&self) -> Option<&str> {
        match self {
            Self::BodyFrom { path, .. } => Some(path),
            _ => None,
        }
    }
}

/// An edit to one of the home's issues, the command line's and the app's
/// alike.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edit {
    /// Opens issue `id`.
    New { id: IssueId, issue: NewIssue },
    /// Changes to one issue, made in one write.
    Change {
        id: IssueId,
        changes: Vec<IssueChange>,
    },
}

impl Edit {
    /// The issue it edits.
    #[must_use]
    pub const fn id(&self) -> IssueId {
        match self {
            Self::New { id, .. } | Self::Change { id, .. } => *id,
        }
    }
}

/// What the shell knows that the core can't: when it is, and who is
/// writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    /// Milliseconds since the Unix epoch.
    pub now: u64,
    /// As the log signs it, `<node>·<harness>`.
    pub by: String,
    /// A comment the written file's front matter ends with.
    pub mark: Option<String>,
}

/// An issue file to write, whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Write {
    pub project: ProjectId,
    pub stem: String,
    pub text: String,
}

impl Write {
    /// Where it is written, from the home's root.
    #[must_use]
    pub fn path(&self) -> String {
        crate::layout::issue_file(&self.project, &self.stem)
    }

    /// The folder it is written in, from the home's root, made first since
    /// a project's first issue finds none.
    #[must_use]
    pub fn folder(&self) -> String {
        crate::layout::issues(&self.project)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub writes: Vec<Write>,
    pub output: String,
    pub failed: bool,
}

impl Outcome {
    /// What the command line says once the shell has saved the writes, or
    /// only why it couldn't, and its exit code.
    #[must_use]
    pub fn ended(&self, saved: Result<(), String>) -> (String, u8) {
        let code = u8::from(self.failed || saved.is_err());
        let text = saved
            .map_or_else(|reason| refusal(&reason), |()| self.output.clone());
        (text, code)
    }
}

/// What the command line says, and its exit code, where no folder at or
/// above holds a flow.
#[must_use]
pub fn homeless() -> (String, u8) {
    (
        refusal(&format!("no home here: no folder above holds {PATH}")),
        2,
    )
}

/// A reason as the command line says it.
fn refusal(reason: &str) -> String {
    format!("home: {reason}\n")
}

/// The file an edit writes, `<project name>/<id>` that names it, and what
/// the flow warns of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub write: Write,
    pub subject: String,
    pub warnings: Vec<String>,
}

/// The id a new issue made at `now`, in seconds since the Unix epoch,
/// takes: the first of `draws` that gives one the home has no file of.
#[must_use]
pub fn fresh(home: &Snapshot, now: u64, draws: &Draws) -> Option<IssueId> {
    let held: Vec<IssueId> = home
        .files()
        .iter()
        .filter_map(|file| IssueId::parse(&file.stem))
        .collect();
    IssueId::mint(now, draws, &held)
}

/// What `args`, after the program's name, ask of `home`: what [`command`]
/// makes of them, unless they ask for something the shell does in the
/// home's folder.
#[must_use]
pub fn asked(
    args: &[String],
    home: &Snapshot,
    fresh: Option<IssueId>,
) -> Asked {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["repos"] => Asked::Local(Local::Status),
        ["repos", "sync"] => Asked::Local(Local::Sync),
        ["repos", "scan"] => Asked::Local(Local::Scan),
        ["skills"] => Asked::Local(Local::Skills),
        _ => Asked::Flow(command(args, home, fresh)),
    }
}

/// The command `args` ask for in `home`, after the program's name.
///
/// A project is named by its name or id, each `<field>=<value>` is a value
/// of the field's type, a list where it takes several, and a new issue
/// takes the id `fresh`, if there is one.
#[must_use]
pub fn command(
    args: &[String],
    home: &Snapshot,
    fresh: Option<IssueId>,
) -> Command {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let joined = |first: &str, more: &[&str]| {
        core::iter::once(first)
            .chain(more.iter().copied())
            .collect::<Vec<&str>>()
            .join(" ")
    };
    match words.as_slice() {
        ["check"] => Command::Check,
        ["issue", "new", project, kind, title, fields @ ..] => {
            opening_command(home, fresh, [project, kind, title], fields)
        }
        ["issue", "body", id, "--file", path] => {
            named(id).map_or_else(Command::Wrong, |issue| Command::BodyFrom {
                id: issue,
                path: (*path).to_owned(),
            })
        }
        ["issue", "set", id, pairs @ ..] if !pairs.is_empty() => {
            setting_command(home, id, pairs)
        }
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
        ] => checklist_command(id, action, number),
        [
            "issue",
            "move",
            id,
            part @ ("stage" | "status"),
            to,
            rest @ ..,
        ] => {
            let (signed_off, reason) = match rest {
                ["--signed-off", reason @ ..] => (true, reason),
                reason => (false, reason),
            };
            changing(
                id,
                IssueChange::Move {
                    part: if *part == "stage" {
                        Part::Stage
                    } else {
                        Part::Status
                    },
                    to: (*to).to_owned(),
                    reason: reason.join(" "),
                    signed_off,
                },
            )
        }
        _ => Command::Usage,
    }
}

/// The issue a command line's word names.
fn named(word: &str) -> Result<IssueId, String> {
    IssueId::parse(word).ok_or_else(|| format!("no issue is called {word}"))
}

fn changing(word: &str, change: IssueChange) -> Command {
    named(word).map_or_else(Command::Wrong, |id| {
        Command::Edit(Edit::Change {
            id,
            changes: Vec::from([change]),
        })
    })
}

fn opening_command(
    home: &Snapshot,
    fresh: Option<IssueId>,
    [name, kind, title]: [&str; 3],
    pairs: &[&str],
) -> Command {
    home.project(name)
        .map(|known| known.id)
        .ok_or_else(|| format!("no project is called {name}"))
        .and_then(|project| {
            let id = fresh.ok_or_else(|| {
                String::from(
                    "no issue id is free: the clock is before 2026, or every draw is taken",
                )
            })?;
            Ok(Edit::New {
                id,
                issue: NewIssue {
                    project,
                    kind: String::from(kind),
                    title: String::from(title),
                    fields: settings(home, kind, pairs)?,
                },
            })
        })
        .map_or_else(Command::Wrong, Command::Edit)
}

fn setting_command(home: &Snapshot, word: &str, pairs: &[&str]) -> Command {
    named(word)
        .and_then(|id| {
            let kind = home.issue(id).map_or("", |issue| issue.kind.as_str());
            let changes = settings(home, kind, pairs)?
                .into_iter()
                .map(|(field, value)| match field.as_str() {
                    TITLE => IssueChange::Title(said(&value)),
                    _ => IssueChange::Set { field, value },
                })
                .collect();
            Ok(Edit::Change { id, changes })
        })
        .map_or_else(Command::Wrong, Command::Edit)
}

fn checklist_command(id: &str, action: &str, number: &str) -> Command {
    let place = number
        .parse::<usize>()
        .ok()
        .and_then(|from_one| from_one.checked_sub(1));
    place.map_or_else(
        || {
            Command::Wrong(format!(
                "{number} is not an item's number, counted from 1"
            ))
        },
        |index| {
            let item = Item { index, seen: None };
            changing(
                id,
                IssueChange::Checklist(match action {
                    "remove" => ChecklistChange::Remove(item),
                    done => ChecklistChange::Check {
                        item,
                        done: done == "check",
                    },
                }),
            )
        },
    )
}

/// Each `<field>=<value>` of `pairs` as the field and its value, for an
/// issue of `kind`.
fn settings(
    home: &Snapshot,
    kind: &str,
    pairs: &[&str],
) -> Result<Vec<(String, Datum)>, String> {
    let flow = home.flow()?;
    pairs
        .iter()
        .map(|pair| {
            let (field, raw) = pair
                .split_once('=')
                .ok_or_else(|| format!("{pair:?} is not <field>=<value>"))?;
            Ok((String::from(field), typed(flow, kind, field, raw)?))
        })
        .collect()
}

/// `given` as the value of `field` on an issue of `kind`: a list where the
/// field or relation takes several, a boolean where it takes one, else
/// text.
fn typed(
    flow: &Flow,
    kind: &str,
    field: &str,
    given: &str,
) -> Result<Datum, String> {
    let raw = given.trim();
    let many = match flow.field(kind, field).map(|declared| &declared.value) {
        Some(FieldType::Bool) => {
            return match raw {
                "true" => Ok(Datum::Bool(true)),
                "false" => Ok(Datum::Bool(false)),
                _ => Err(format!("{field} is true or false")),
            };
        }
        Some(FieldType::Texts) => true,
        Some(_) => false,
        None => flow
            .relations
            .iter()
            .any(|relation| relation.name == field && relation.many),
    };
    Ok(if many {
        Datum::Texts(
            raw.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(String::from)
                .collect(),
        )
    } else {
        Datum::Text(String::from(raw))
    })
}

/// A value as the command line gives it, a list's comma-separated.
#[must_use]
pub fn said(value: &Datum) -> String {
    match value {
        Datum::Text(text) => text.clone(),
        Datum::Texts(texts) => texts.join(","),
        Datum::Bool(flag) => flag.to_string(),
    }
}

/// What doing `command` to `home` writes and says, given the text of the
/// file it asked for, if it asked for one.
#[must_use]
pub fn run(
    command: Command,
    file: Option<Result<String, String>>,
    home: &Snapshot,
    context: &Context,
) -> Outcome {
    let edit = match command {
        Command::Usage => return refused(String::from(USAGE)),
        Command::Wrong(reason) => return refused(refusal(&reason)),
        Command::Check => {
            let report = check(home);
            return Outcome {
                writes: Vec::new(),
                output: report.text(),
                failed: report.failed(),
            };
        }
        Command::BodyFrom { id, path } => {
            match file.unwrap_or_else(|| Err(format!("{path} wasn't read"))) {
                Ok(text) => Edit::Change {
                    id,
                    changes: Vec::from([IssueChange::Body(text)]),
                },
                Err(reason) => return refused(refusal(&reason)),
            }
        }
        Command::Edit(edit) => edit,
    };
    match write(home, &edit, context) {
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
        Err(reason) => refused(refusal(&reason)),
    }
}

const fn refused(output: String) -> Outcome {
    Outcome {
        writes: Vec::new(),
        output,
        failed: true,
    }
}

/// What `edit` writes to `home`'s issues, read back against the flow among
/// the others.
///
/// # Errors
///
/// Why the edit can't be made, or what the flow refuses it for.
pub fn write(
    home: &Snapshot,
    edit: &Edit,
    context: &Context,
) -> Result<Written, String> {
    let flow = home.flow()?;
    let sign = Signer {
        at: stamp(context.now),
        by: context.by.clone(),
        mark: context.mark.clone(),
    };
    let file = match edit {
        Edit::New { id, issue } => opened(home, flow, *id, &sign, issue)?,
        Edit::Change { id, changes } => {
            let stem = id.to_string();
            let file = home
                .files()
                .iter()
                .find(|file| file.stem == stem)
                .ok_or_else(|| format!("no issue is called {id}"))?;
            changed(flow, file, changes, &sign)?
        }
    };
    finished(home, flow, file)
}

/// Who writes a file, and when.
struct Signer {
    at: String,
    by: String,
    mark: Option<String>,
}

impl Signer {
    /// The log's entry of `event`, signed and dated.
    fn entry(&self, event: LogEvent, text: &str) -> Entry {
        Entry {
            at: Some(self.at.clone()),
            by: Some(self.by.clone()),
            event,
            text: String::from(text),
        }
    }

    /// The document's text as it is written, marked.
    fn text(&self, mut document: Document) -> String {
        if let Some(mark) = &self.mark {
            document.comment(mark);
        }
        document.text()
    }
}

/// The file read back among the others, weighed as a move from the issue
/// it was, refused on a structural problem or a broken `error` rule, else
/// written with any warning.
fn finished(
    home: &Snapshot,
    flow: &Flow,
    file: File,
) -> Result<Written, String> {
    let after = home.with(file.clone());
    let graph = after.graph();
    let name = home
        .projects()
        .iter()
        .find(|project| project.id == file.project)
        .map_or_else(
            || file.project.to_string(),
            |project| project.name.clone(),
        );
    let subject = format!("{name}/{}", file.stem);
    let mut problems: Vec<String> = graph
        .problems
        .iter()
        .filter(|problem| {
            problem.project == file.project && problem.stem == file.stem
        })
        .map(ToString::to_string)
        .collect();
    let findings: Vec<Finding> = graph
        .issues
        .iter()
        .find(|issue| {
            issue.project == file.project && issue.id.to_string() == file.stem
        })
        .map_or_else(Vec::new, |issue| {
            let moving = Move {
                from: home.issue(issue.id),
                to: Some(issue),
            };
            let issues: Vec<&Issue> = graph.issues.iter().collect();
            of_issue(flow, &issues, issue, moving, &subject)
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
    home: &Snapshot,
    flow: &Flow,
    id: IssueId,
    sign: &Signer,
    new: &NewIssue,
) -> Result<File, String> {
    let NewIssue {
        project,
        kind,
        title: wanted,
        fields,
    } = new;
    let at = &sign.at;
    if !home.projects().iter().any(|known| known.id == *project) {
        return Err(format!("no project is called {project}"));
    }
    let declared = flow
        .kind(kind)
        .ok_or_else(|| format!("no kind is called {kind}"))?;
    let title = needs(wanted, "an issue needs a title")?;
    let status = opening(flow).map_or_else(String::new, String::from);
    let stem = id.to_string();
    let mut front = Vec::from([
        (String::from(ID), quoted(&stem)),
        (String::from(TITLE), quoted(title)),
        (String::from(KIND), quoted(kind)),
        (String::from(STATUS), quoted(&status)),
    ]);
    front.extend(
        fields
            .iter()
            .map(|(key, value)| (key.clone(), toml_of(value))),
    );
    if let Some(created) = flow.field_with(kind, FieldRole::Created) {
        front.push((created.name.clone(), at.clone()));
    }
    let document = Document::new(
        &front,
        &declared.sections,
        &sign.entry(LogEvent::Note, "opened."),
    );
    Ok(File {
        project: *project,
        stem,
        text: sign.text(document),
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
    let (issue, mut document) = parsed(flow, file).map_err(|problem| {
        format!("{} doesn't read as the flow declares: {problem}", file.stem)
    })?;
    let mut set = Vec::new();
    for change in changes {
        match change {
            IssueChange::Move {
                part,
                to,
                reason,
                signed_off,
            } => {
                let (key, event, text) =
                    shifted(flow, &issue, (*part, to, reason), *signed_off)?;
                document.set(key, &quoted(to)).ok_or_else(unread)?;
                document.log_entry(&sign.entry(event, &text));
            }
            IssueChange::Set { field, value } => {
                setting(&mut document, field, value)?;
                set.push(format!("{field}={}", said(value)));
            }
            IssueChange::Title(title) => {
                let kept = needs(title, "an issue needs a title")?;
                document.set(TITLE, &quoted(kept)).ok_or_else(unread)?;
                set.push(format!("{TITLE}={kept}"));
            }
            IssueChange::Note(note) => {
                let text = needs(note, "a note needs text")?;
                document.log_entry(&sign.entry(LogEvent::Note, text));
            }
            IssueChange::Body(text) => document.body(text.trim()),
            IssueChange::Checklist(item) => {
                checklist(&issue, &mut document, item)?;
            }
        }
    }
    if !set.is_empty() {
        document.log_entry(
            &sign.entry(LogEvent::Note, &format!("set {}.", set.join(", "))),
        );
    }
    Ok(File {
        text: sign.text(document),
        ..file.clone()
    })
}

/// The key a move sets, the log's event for it, and the log's text, which
/// says when the owner signed off.
///
/// # Errors
///
/// No stage or status is called `to`, or the move enters a stage with a
/// sign-off unsigned.
fn shifted(
    flow: &Flow,
    current: &Issue,
    (part, to, reason): (Part, &str, &str),
    signed_off: bool,
) -> Result<(&'static str, LogEvent, String), String> {
    let (key, from, known, usual) = match part {
        Part::Stage => {
            let from = current.stage.clone();
            let usual = from
                .as_ref()
                .and_then(|stage| flow.stage(stage))
                .is_some_and(|stage| stage.next.iter().any(|next| next == to));
            (STAGE, from, flow.stage(to).is_some(), usual)
        }
        Part::Status => (
            STATUS,
            Some(current.status.clone()),
            flow.status(to).is_some(),
            true,
        ),
    };
    if !known {
        return Err(format!("no {key} is called {to}"));
    }
    if part == Part::Stage
        && from.as_deref() != Some(to)
        && !signed_off
        && flow.stage(to).is_some_and(|stage| stage.sign_off)
    {
        return Err(format!(
            "entering the stage {to} needs its sign-off; fix: add `--signed-off` once the owner has signed off"
        ));
    }
    let (before, after) = (
        from.unwrap_or_else(|| String::from("none")),
        String::from(to),
    );
    let event = match part {
        Part::Stage => LogEvent::Stage {
            from: before,
            to: after,
        },
        Part::Status => LogEvent::Status {
            from: before,
            to: after,
        },
    };
    let unusual = if usual { "" } else { " (unusual move)" };
    let said = match (signed_off, reason) {
        (false, _) => String::from(reason),
        (true, "") => String::from("signed off"),
        (true, _) => format!("signed off: {reason}"),
    };
    Ok((key, event, format!("{said}{unusual}")))
}

/// Sets a declared field or relation; an issue's own words change by
/// changes of their own.
fn setting(
    document: &mut Document,
    key: &str,
    value: &Datum,
) -> Result<(), String> {
    match key {
        STAGE | STATUS => {
            Err(format!("a {key} changes by a move, not by setting a field"))
        }
        ID | KIND => Err(format!("an issue's {key} doesn't change")),
        TITLE => Err(String::from(
            "the title changes by retitling, not by setting a field",
        )),
        _ => document.set(key, &toml_of(value)).ok_or_else(unread),
    }
}

fn unread() -> String {
    String::from("its front matter doesn't read")
}

/// A value as TOML, its text trimmed and a list without empty items.
fn toml_of(value: &Datum) -> String {
    match value {
        Datum::Text(text) => quoted(text.trim()),
        Datum::Texts(texts) => Value::Array(
            texts
                .iter()
                .map(|item| item.trim())
                .filter(|item| !item.is_empty())
                .map(|item| Value::String(item.to_owned()))
                .collect(),
        )
        .to_string(),
        Datum::Bool(flag) => Value::Boolean(*flag).to_string(),
    }
}

fn checklist(
    issue: &Issue,
    document: &mut Document,
    change: &ChecklistChange,
) -> Result<(), String> {
    let missing = || String::from("its checklist doesn't read");
    match change {
        ChecklistChange::Add(text) => {
            document.add_item(needs(text, "a checklist item needs text")?);
            Ok(())
        }
        ChecklistChange::Check { item, done } => document
            .check(picked(issue, item)?, *done)
            .ok_or_else(missing),
        ChecklistChange::Remove(item) => document
            .remove_item(picked(issue, item)?)
            .ok_or_else(missing),
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

fn quoted(text: &str) -> String {
    Value::String(text.to_owned()).to_string()
}

/// `now` as the log writes a time, RFC 3339 in UTC to the second.
#[must_use]
pub fn stamp(now: u64) -> String {
    let seconds = now / 1000;
    let (year, month, day) = crate::calendar::civil(seconds / 86_400);
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
