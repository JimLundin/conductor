use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::ToString;

use crate::canvas::{Canvas, CanvasFile, CanvasKey, CanvasName, Held};
use crate::flow::{FieldRole, FieldType, Flow, Severity};
use crate::issue::{
    ChecklistChange, Datum, Entry, ID, Issue, IssueChange, IssueListing,
    IssueReading, Item, KIND, LogEvent, NewIssue, Part, STAGE, STATUS, TITLE,
};
use crate::{IssueId, IssueKey, ProjectId, UnixMillis, UnixSecs};
use serde::{Deserialize, Serialize};

use super::PATH;
use super::check::{canvases, check, listed, of_issue};
use super::eval::Move;
use super::graph::branch_of;
use super::resolve::Mode;
use super::role::opening;
use super::snapshot::Snapshot;
use crate::issue::body::Body;
use crate::prelude::*;

pub const USAGE: &str = "usage: home <command>, from anywhere in the home

  home project access <project> [host-data-dir]
      Checks the configured credential's access to the project repository.
  home project capabilities <project>
      Validates backend configuration and shows available project operations.
  home project operation-status <handle> [--data-dir <dir>]
      Shows the operation, its confirmed steps and any step awaiting evidence.
  home check
      Checks every project and issue against the flow; fails only on errors.
  home issue list <project> [--offline] [--data-dir <dir>]
      Lists the project's issues as JSON, from its tracker or the home, with
      freshness, coverage and refresh failures.
  home issue show <project> <id> [--offline] [--data-dir <dir>]
      Reads one issue of the project as JSON, by its tracker's ID.
      Both commands refresh by default. --offline reads the host cache.
  home issue new <project> <kind> <title> [<field>=<value>...]
      Opens an issue of the kind in the project, named by its name or id,
      with the kind's sections and a fresh id, which it prints. Give the
      title as one argument.
  home issue move <project> <id> stage|status <to> [--signed-off] [<reason>...]
      Moves the stage or status, the only way to change either, and logs
      the move and its reason, marking a move off the stage's usual next
      ones as unusual. Entering a stage with a sign-off takes --signed-off.
  home issue set <project> <id> <field>=<value>...
      Sets any other field or relation, title, parent and blocked_by among
      them. A list's value is comma-separated: blocked_by=<id>,<id>.
      Name another project's issue as <project> <id>, the project by its
      name or id, and quote the pair: parent=\"standards 146\".
  home issue note <project> <id> <text>...
      Appends a note to the log.
  home issue body <project> <id> <text>...
  home issue body <project> <id> --file <path>
      Replaces the body, everything above its checklist and notes, with the
      text or the file's; give a body of several lines or sections by file.
  home issue checklist <project> <id> add <text>...
      Adds an item to the checklist.
  home issue checklist <project> <id> check|uncheck|remove <n>
      Checks, unchecks or removes the checklist's nth item, from 1.
  home canvas put <project> <id> <name> [--file <path>]
      Writes the issue's canvas of that name, in place of any written
      before, from the file's JSON or else stdin's, dated and signed.
  home canvas show <project> <id> [<name>]
      Prints the issue's canvas of that name, or every canvas it has, as
      markdown.
  home canvas schema
      Prints the JSON Schema of the canvas that canvas put reads.
  home repos
      Shows each project's checkouts on this machine and their state.
  home repos --verify
      Shows the same, then each project whose default branch head lacks a
      recorded pass of its profiles, with the command that runs them.
  home repos sync
      Clones each project with a remote and no checkout here.
  home repos scan
      Registers each repository here that matches no project, under a fresh
      id, and rewrites the project list in AGENTS.md.
  home skills
      Installs the home's skills for every harness, at user level, keeping
      any skill installed some other way.

A command names an issue by its project, by name or id, and the ID its
tracker gave it. An ID the cache lacks refreshes the project's tracker once.

Each write signs and dates the log and is read back against the flow: a
structural problem or a broken error rule refuses it, and anything softer
is written with a warning. A write the home keeps is committed to the home,
signed by HOME_BY.
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    Flow(Command),
    Local(Local),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Local {
    Status,
    Verify,
    Sync,
    Scan,
    Skills,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Check,
    Edit(Edit),
    BodyFrom {
        issue: IssueKey,
        path: String,
    },
    CanvasPut {
        key: CanvasKey,
        from: Source,
    },
    CanvasShow(Wanted),
    CanvasSchema,
    IssueShow {
        issue: IssueKey,
        mode: Mode,
        data: Option<String>,
    },
    Wrong(String),
    Usage,
}

pub type Resolved = BTreeMap<IssueKey, Result<IssueReading, String>>;

impl Command {
    #[must_use]
    pub fn needs(&self) -> Option<Need> {
        match self {
            Self::BodyFrom { path, .. } => {
                Some(Need::Read(Source::File(path.clone())))
            }
            Self::CanvasPut { from, .. } => Some(Need::Read(from.clone())),
            Self::CanvasShow(wanted) => Some(Need::Canvases(wanted.clone())),
            Self::Check => Some(Need::Check),
            _ => None,
        }
    }

    #[must_use]
    pub fn issue(&self) -> Option<IssueKey> {
        match self {
            Self::Edit(edit @ Edit::Change { .. }) => Some(edit.key()),
            Self::BodyFrom { issue, .. } | Self::IssueShow { issue, .. } => {
                Some(issue.clone())
            }
            Self::CanvasPut { key, .. } => Some(of_canvas(key)),
            Self::CanvasShow(wanted) => Some(IssueKey {
                project: wanted.project,
                id: wanted.issue.clone(),
            }),
            _ => None,
        }
    }

    #[must_use]
    pub fn issues(&self, given: Option<&Given>) -> BTreeSet<IssueKey> {
        match (self, given) {
            (
                Self::Check,
                Some(Given::Check(Checked {
                    canvases: Ok(held), ..
                })),
            ) => held
                .canvases
                .iter()
                .map(|fetched| of_canvas(&fetched.key))
                .collect(),
            _ => self.issue().into_iter().collect(),
        }
    }

    #[must_use]
    pub const fn mode(&self) -> Mode {
        match self {
            Self::IssueShow { mode, .. } => *mode,
            _ => Mode::Cached,
        }
    }

    #[must_use]
    pub fn data(&self) -> Option<&str> {
        match self {
            Self::IssueShow { data, .. } => data.as_deref(),
            _ => None,
        }
    }
}

#[must_use]
pub fn of_canvas(key: &CanvasKey) -> IssueKey {
    IssueKey {
        project: key.project,
        id: key.issue.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    File(String),
    Stdin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Need {
    Read(Source),
    Canvases(Wanted),
    Check,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wanted {
    pub project: ProjectId,
    pub issue: IssueId,
    pub name: Option<CanvasName>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Given {
    Text(Result<String, String>),
    Canvases(Result<Held<Fetched>, String>),
    Check(Checked),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub canvases: Result<Held<Fetched>, String>,
    pub listings: Vec<Listed>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub project: ProjectId,
    pub listing: Result<IssueListing, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub key: CanvasKey,
    pub got: Result<Option<CanvasFile>, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edit {
    New {
        id: IssueId,
        issue: NewIssue,
    },
    Change {
        project: ProjectId,
        id: IssueId,
        changes: Vec<IssueChange>,
    },
}

impl Edit {
    #[must_use]
    pub fn key(&self) -> IssueKey {
        match self {
            Self::New { id, issue } => IssueKey {
                project: issue.project,
                id: id.clone(),
            },
            Self::Change { project, id, .. } => IssueKey {
                project: *project,
                id: id.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub now: UnixMillis,
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mutation {
    Set {
        field: String,
        value: Datum,
    },
    Relate {
        relation: String,
        many: bool,
        targets: Vec<IssueKey>,
    },
    Title(String),
    Move {
        part: Part,
        to: String,
    },
    Body(String),
    Checklist(ChecklistChange),
    Note(Entry),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub observation: crate::issue::Observation,
    pub created: bool,
    pub mutations: Vec<Mutation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasWrite {
    pub key: CanvasKey,
    pub file: CanvasFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub changes: Vec<Change>,
    pub canvases: Vec<CanvasWrite>,
    pub message: Option<String>,
    pub output: String,
    pub failed: bool,
}

impl Outcome {
    pub fn named(&mut self, home: &Snapshot, ids: &[IssueId]) {
        for (change, id) in self.changes.iter().zip(ids) {
            let project = change.observation.project();
            let planned = change.observation.id();
            if planned != id {
                self.output = self.output.replace(
                    &subject(home, project, planned),
                    &subject(home, project, id),
                );
            }
        }
    }

    #[must_use]
    pub fn ended(&self, saved: Result<(), String>) -> (String, u8) {
        let code = u8::from(self.failed || saved.is_err());
        let text = saved
            .map_or_else(|reason| refusal(&reason), |()| self.output.clone());
        (text, code)
    }

    #[must_use]
    pub fn commit_message(&self, home_files: &[String]) -> Option<&str> {
        self.message.as_deref().filter(|_| !home_files.is_empty())
    }
}

#[must_use]
pub fn homeless() -> (String, u8) {
    (
        refusal(&format!("no home here: no folder above holds {PATH}")),
        2,
    )
}

fn refusal(reason: &str) -> String {
    format!("home: {reason}\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub change: Change,
    pub subject: String,
    pub warnings: Vec<String>,
}

#[must_use]
pub fn asked(
    args: &[String],
    home: &Snapshot,
    fresh: Option<IssueId>,
) -> Asked {
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["repos"] => Asked::Local(Local::Status),
        ["repos", "--verify"] => Asked::Local(Local::Verify),
        ["repos", "sync"] => Asked::Local(Local::Sync),
        ["repos", "scan"] => Asked::Local(Local::Scan),
        ["skills"] => Asked::Local(Local::Skills),
        _ => Asked::Flow(command(args, home, fresh)),
    }
}

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
        ["issue", "show", project, id, flags @ ..] => {
            showing_issue(home, [project, id], flags)
        }
        ["issue", "body", project, id, "--file", path] => {
            key(home, [project, id]).map_or_else(Command::Wrong, |issue| {
                Command::BodyFrom {
                    issue,
                    path: (*path).to_owned(),
                }
            })
        }
        ["canvas", "put", project, id, name] => {
            putting(home, [project, id, name], Source::Stdin)
        }
        ["canvas", "put", project, id, name, "--file", path] => {
            putting(home, [project, id, name], Source::File((*path).to_owned()))
        }
        ["canvas", "schema"] => Command::CanvasSchema,
        ["canvas", "show", project, id] => showing(home, [project, id], None),
        ["canvas", "show", project, id, name] => {
            showing(home, [project, id], Some(name))
        }
        ["issue", "set", project, id, pairs @ ..] if !pairs.is_empty() => {
            setting_command(home, [project, id], pairs)
        }
        ["issue", "note", project, id, first, more @ ..] => changing(
            home,
            [project, id],
            IssueChange::Note(joined(first, more)),
        ),
        ["issue", "body", project, id, first, more @ ..] => changing(
            home,
            [project, id],
            IssueChange::Body(joined(first, more)),
        ),
        ["issue", "checklist", project, id, "add", first, more @ ..] => {
            changing(
                home,
                [project, id],
                IssueChange::Checklist(ChecklistChange::Add(joined(
                    first, more,
                ))),
            )
        }
        [
            "issue",
            "checklist",
            project,
            id,
            action @ ("check" | "uncheck" | "remove"),
            number,
        ] => checklist_command(home, [project, id], action, number),
        [
            "issue",
            "move",
            project,
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
                home,
                [project, id],
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

fn key(home: &Snapshot, [name, word]: [&str; 2]) -> Result<IssueKey, String> {
    let project = home
        .project(name)
        .map(|known| known.id)
        .ok_or_else(|| format!("no project is called {name}"))?;
    let id = home.ids().read(project, word)?;
    Ok(IssueKey { project, id })
}

fn canvas_name(word: &str) -> Result<CanvasName, String> {
    CanvasName::parse(word).map_err(|bad| bad.to_string())
}

fn showing_issue(
    home: &Snapshot,
    naming: [&str; 2],
    flags: &[&str],
) -> Command {
    let mut mode = Mode::Fresh;
    let mut data = None;
    let mut rest = flags;
    while let [flag, after @ ..] = rest {
        rest = match (*flag, after) {
            ("--offline", _) if mode == Mode::Fresh => {
                mode = Mode::Offline;
                after
            }
            ("--data-dir", [dir, beyond @ ..])
                if data.is_none() && !dir.starts_with("--") =>
            {
                data = Some(String::from(*dir));
                beyond
            }
            _ => return Command::Usage,
        };
    }
    key(home, naming).map_or_else(Command::Wrong, |issue| Command::IssueShow {
        issue,
        mode,
        data,
    })
}

fn putting(
    home: &Snapshot,
    [project, id, name]: [&str; 3],
    from: Source,
) -> Command {
    key(home, [project, id])
        .and_then(|issue| {
            Ok(Command::CanvasPut {
                key: CanvasKey {
                    project: issue.project,
                    issue: issue.id,
                    name: canvas_name(name)?,
                },
                from,
            })
        })
        .unwrap_or_else(Command::Wrong)
}

fn showing(home: &Snapshot, naming: [&str; 2], name: Option<&str>) -> Command {
    key(home, naming)
        .and_then(|issue| {
            Ok(Command::CanvasShow(Wanted {
                project: issue.project,
                issue: issue.id,
                name: name.map(canvas_name).transpose()?,
            }))
        })
        .unwrap_or_else(Command::Wrong)
}

fn changing(
    home: &Snapshot,
    naming: [&str; 2],
    change: IssueChange,
) -> Command {
    key(home, naming).map_or_else(Command::Wrong, |issue| {
        Command::Edit(Edit::Change {
            project: issue.project,
            id: issue.id,
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

fn setting_command(
    home: &Snapshot,
    naming: [&str; 2],
    pairs: &[&str],
) -> Command {
    key(home, naming)
        .and_then(|IssueKey { project, id }| {
            let kind = home
                .issue(project, &id)
                .map_or("", |issue| issue.kind.as_str());
            let changes = settings(home, kind, pairs)?
                .into_iter()
                .map(|(field, value)| match field.as_str() {
                    TITLE => IssueChange::Title(said(&value)),
                    _ => IssueChange::Set { field, value },
                })
                .collect();
            Ok(Edit::Change {
                project,
                id,
                changes,
            })
        })
        .map_or_else(Command::Wrong, Command::Edit)
}

fn checklist_command(
    home: &Snapshot,
    issue: [&str; 2],
    action: &str,
    number: &str,
) -> Command {
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
                home,
                issue,
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

#[must_use]
pub fn said(value: &Datum) -> String {
    match value {
        Datum::Text(text) => text.clone(),
        Datum::Texts(texts) => texts.join(","),
        Datum::Bool(flag) => flag.to_string(),
    }
}

#[must_use]
pub fn run(
    command: Command,
    given: Option<Given>,
    resolved: &Resolved,
    home: &Snapshot,
    context: &Context,
) -> Outcome {
    let found = match command.issue().map(|key| found(resolved, &key)) {
        Some(Err(reason)) => return refused(refusal(&reason)),
        Some(Ok(reading)) => Some(reading),
        None => None,
    };
    let edit = match command {
        Command::Usage => return refused(String::from(USAGE)),
        Command::Wrong(reason) => return refused(refusal(&reason)),
        Command::IssueShow { .. } => return issue_shown(found),
        Command::Check => {
            let mut report = check(home);
            let checked = match given {
                Some(Given::Check(checked)) => checked,
                _ => Checked {
                    canvases: Err(String::from(
                        "the store's canvases weren't read",
                    )),
                    listings: Vec::new(),
                },
            };
            report
                .findings
                .extend(canvases(home, checked.canvases, resolved));
            report.extend(listed(home, &checked.listings));
            return Outcome {
                output: report.text(),
                failed: report.failed(),
                ..said_only()
            };
        }
        Command::BodyFrom { issue, path } => match text(given, &path) {
            Ok(text) => Edit::Change {
                project: issue.project,
                id: issue.id,
                changes: Vec::from([IssueChange::Body(text)]),
            },
            Err(reason) => return refused(refusal(&reason)),
        },
        Command::CanvasPut { key, from } => {
            let place = match &from {
                Source::File(path) => path.as_str(),
                Source::Stdin => "stdin",
            };
            return put(home, key, text(given, place), context);
        }
        Command::CanvasShow(wanted) => return shown(home, &wanted, given),
        Command::CanvasSchema => {
            return Outcome {
                output: Canvas::schema(),
                ..said_only()
            };
        }
        Command::Edit(edit) => edit,
    };
    let observed = found.and_then(|reading| reading.record.as_ref());
    match plan(home, &edit, observed, context) {
        Ok(Planned {
            change,
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
            changes: Vec::from([change]),
            canvases: Vec::new(),
            message: Some(crate::issue::message(&edit)),
            failed: false,
        },
        Err(reason) => refused(refusal(&reason)),
    }
}

fn found<'a>(
    resolved: &'a Resolved,
    key: &IssueKey,
) -> Result<&'a IssueReading, String> {
    resolved
        .get(key)
        .ok_or_else(|| format!("{key} wasn't looked up"))?
        .as_ref()
        .map_err(Clone::clone)
}

fn issue_shown(found: Option<&IssueReading>) -> Outcome {
    let Some(reading) = found else {
        return refused(refusal("the issue wasn't looked up"));
    };
    match serde_json::to_string_pretty(reading) {
        Ok(json) => Outcome {
            output: format!("{json}\n"),
            failed: reading.status.failure.is_some(),
            ..said_only()
        },
        Err(error) => refused(refusal(&error.to_string())),
    }
}

const fn refused(output: String) -> Outcome {
    Outcome {
        changes: Vec::new(),
        canvases: Vec::new(),
        message: None,
        output,
        failed: true,
    }
}

fn text(given: Option<Given>, from: &str) -> Result<String, String> {
    match given {
        Some(Given::Text(text)) => text,
        _ => Err(format!("{from} wasn't read")),
    }
}

fn put(
    home: &Snapshot,
    key: CanvasKey,
    read: Result<String, String>,
    context: &Context,
) -> Outcome {
    let named = canvas_named(home, &key);
    let parsed = read.and_then(|text| {
        Canvas::parse(&text).map_err(|error| format!("{named}: {error}"))
    });
    match parsed {
        Ok(canvas) => Outcome {
            changes: Vec::new(),
            message: Some(format!(
                "Write canvas {} of issue {}",
                key.name, key.issue
            )),
            output: format!("wrote {named}\n"),
            canvases: Vec::from([CanvasWrite {
                key,
                file: CanvasFile {
                    written: context.now,
                    by: context.by.clone(),
                    canvas,
                },
            }]),
            failed: false,
        },
        Err(reason) => refused(refusal(&reason)),
    }
}

fn shown(home: &Snapshot, wanted: &Wanted, given: Option<Given>) -> Outcome {
    let subject = subject(home, wanted.project, &wanted.issue);
    let fetched = match given {
        Some(Given::Canvases(fetched)) => fetched,
        _ => Err(format!("the canvases of {subject} weren't fetched")),
    };
    let held = match fetched {
        Ok(held) => held,
        Err(reason) => return refused(refusal(&reason)),
    };
    let mut drawn = Vec::new();
    let mut broken = Vec::new();
    for Fetched { key, got } in &held.canvases {
        match got {
            Ok(None) => {}
            Ok(Some(file)) => drawn.push(file.canvas.markdown()),
            Err(reason) => broken.push(refusal(&format!(
                "{}: {reason}",
                canvas_named(home, key)
            ))),
        }
    }
    broken.extend(
        held.strays.iter().map(|stray| {
            refusal(&format!("{}: {}", stray.place, stray.reason))
        }),
    );
    match (&wanted.name, drawn.is_empty() && broken.is_empty()) {
        (Some(name), true) => refused(refusal(&format!(
            "no canvas is called {name} on {subject}"
        ))),
        (None, true) => Outcome {
            output: format!("{subject} has no canvases\n"),
            ..said_only()
        },
        (_, false) => Outcome {
            output: drawn.join("\n") + &broken.concat(),
            failed: !broken.is_empty(),
            ..said_only()
        },
    }
}

pub(crate) fn canvas_named(home: &Snapshot, key: &CanvasKey) -> String {
    format!(
        "canvas {} of {}",
        key.name,
        subject(home, key.project, &key.issue)
    )
}

const fn said_only() -> Outcome {
    Outcome {
        changes: Vec::new(),
        canvases: Vec::new(),
        message: None,
        output: String::new(),
        failed: false,
    }
}

pub fn plan(
    home: &Snapshot,
    edit: &Edit,
    observed: Option<&crate::issue::Observation>,
    context: &Context,
) -> Result<Planned, String> {
    let flow = home.flow()?;
    let sign = Signer {
        at: stamp(context.now.secs()),
        now: context.now,
        by: context.by.clone(),
    };
    let change = match edit {
        Edit::New { id, issue } => opened(home, flow, id, &sign, issue)?,
        Edit::Change {
            project,
            id,
            changes,
        } => {
            let observation = observed.ok_or_else(|| {
                format!("no issue {id} in {}", named(home, *project))
            })?;
            observation.editable().map_err(|reason| {
                format!("{id} doesn't read as the flow declares: {reason}")
            })?;
            changed(home, flow, observation, changes, &sign)?
        }
    };
    finished(
        home,
        flow,
        observed.and_then(|before| before.issue()),
        change,
    )
}

struct Signer {
    now: UnixMillis,
    at: String,
    by: String,
}

impl Signer {
    fn entry(&self, event: LogEvent, text: &str) -> Entry {
        Entry {
            at: Some(self.at.clone()),
            by: Some(self.by.clone()),
            event,
            text: String::from(text),
        }
    }
}

fn finished(
    home: &Snapshot,
    flow: &Flow,
    before: Option<&Issue>,
    change: Change,
) -> Result<Planned, String> {
    let (issue, _) = change.observation.editable()?;
    let after = home.with(change.observation.clone());
    let graph = after.graph();
    let subject = subject(home, issue.project, &issue.id);
    let mut problems: Vec<String> = graph
        .problems
        .iter()
        .filter(|problem| {
            problem.project == issue.project
                && problem.record == issue.id.to_string()
        })
        .map(ToString::to_string)
        .collect();
    let moving = Move {
        from: before,
        to: Some(issue),
    };
    let issues: Vec<&Issue> = graph.issues.iter().collect();
    let findings = of_issue(flow, &issues, issue, moving, &subject);
    problems.extend(
        findings
            .iter()
            .filter(|finding| finding.severity == Severity::Error)
            .map(|finding| finding.message.clone()),
    );
    if !problems.is_empty() {
        return Err(format!("refused {subject}: {}", problems.join("; ")));
    }
    Ok(Planned {
        warnings: findings
            .into_iter()
            .map(|finding| finding.message)
            .collect(),
        subject,
        change,
    })
}

pub(crate) fn subject(
    home: &Snapshot,
    project: ProjectId,
    stem: impl core::fmt::Display,
) -> String {
    format!("{}/{stem}", named(home, project))
}

#[must_use]
pub fn named(home: &Snapshot, project: ProjectId) -> String {
    home.projects()
        .iter()
        .find(|known| known.id == project)
        .map_or_else(|| project.to_string(), |known| known.name.clone())
}

fn opened(
    home: &Snapshot,
    flow: &Flow,
    id: &IssueId,
    sign: &Signer,
    new: &NewIssue,
) -> Result<Change, String> {
    let NewIssue {
        project,
        kind,
        title: wanted,
        fields,
    } = new;
    if !home.projects().iter().any(|known| known.id == *project) {
        return Err(format!("no project is called {project}"));
    }
    let declared = flow
        .kind(kind)
        .ok_or_else(|| format!("no kind is called {kind}"))?;
    let title = String::from(needs(wanted, "an issue needs a title")?);
    let mut issue = Issue {
        id: id.clone(),
        project: *project,
        created: Some(sign.now),
        title,
        kind: kind.clone(),
        status: opening(flow).map_or_else(String::new, String::from),
        stage: None,
        fields: BTreeMap::new(),
        links: BTreeMap::new(),
        body: String::new(),
        sections: Vec::new(),
        checklist: Vec::new(),
        log: Vec::new(),
        notes: Vec::new(),
        branch: branch_of(id, None),
        canvases: Vec::new(),
    };
    let mut mutations = Vec::new();
    let mut names = BTreeSet::from([ID, TITLE, KIND, STATUS]);
    if let Some(created) = flow.field_with(kind, FieldRole::Created) {
        names.insert(&created.name);
    }
    for (field, value) in fields {
        if !names.insert(field) {
            return Err(format!("{field}: is given more than once"));
        }
        if field == STAGE {
            issue.stage = value.text().map(str::trim).map(String::from);
        } else {
            assign(home, flow, &mut issue, field, value)?;
        }
        mutations.push(kept(flow, &issue, field, value));
    }
    if let Some(created) = flow.field_with(kind, FieldRole::Created) {
        let value = Datum::Text(sign.at.clone());
        issue.fields.insert(created.name.clone(), value.clone());
        mutations.push(Mutation::Set {
            field: created.name.clone(),
            value,
        });
    }
    issue.branch = branch_of(
        id,
        flow.field_with(kind, FieldRole::Branch)
            .and_then(|field| issue.text(&field.name)),
    );
    let entry = sign.entry(LogEvent::Note, "opened.");
    let body = Body::new(&declared.sections, &entry);
    mutations.push(Mutation::Note(entry.clone()));
    issue.log.push(entry);
    Ok(Change {
        observation: crate::issue::Observation::mapped(issue, body),
        created: true,
        mutations,
    })
}

fn changed(
    home: &Snapshot,
    flow: &Flow,
    observation: &crate::issue::Observation,
    changes: &[IssueChange],
    sign: &Signer,
) -> Result<Change, String> {
    let (current, original_body) = observation.editable()?;
    let mut issue = current.clone();
    let mut body = original_body.clone();
    let mut mutations = Vec::new();
    let mut set = Vec::new();
    for change in changes {
        match change {
            IssueChange::Move {
                part,
                to,
                reason,
                signed_off,
            } => {
                let (_, event, text) =
                    shifted(flow, current, (*part, to, reason), *signed_off)?;
                match part {
                    Part::Stage => issue.stage = Some(to.clone()),
                    Part::Status => issue.status.clone_from(to),
                }
                mutations.push(Mutation::Move {
                    part: *part,
                    to: to.clone(),
                });
                let entry = sign.entry(event, &text);
                body.log_entry(&entry);
                mutations.push(Mutation::Note(entry));
            }
            IssueChange::Set { field, value } => {
                setting(home, flow, &mut issue, field, value)?;
                mutations.push(kept(flow, &issue, field, value));
                set.push(format!("{field}={}", said(value)));
            }
            IssueChange::Title(title) => {
                let kept = needs(title, "an issue needs a title")?;
                issue.title = String::from(kept);
                mutations.push(Mutation::Title(issue.title.clone()));
                set.push(format!("{TITLE}={kept}"));
            }
            IssueChange::Note(note) => {
                let entry = sign
                    .entry(LogEvent::Note, needs(note, "a note needs text")?);
                body.log_entry(&entry);
                mutations.push(Mutation::Note(entry));
            }
            IssueChange::Body(text) => {
                body.body(text.trim());
                mutations.push(Mutation::Body(String::from(text.trim())));
            }
            IssueChange::Checklist(item) => {
                checklist(current, &mut body, item)?;
                mutations.push(Mutation::Checklist(item.clone()));
            }
        }
    }
    if !set.is_empty() {
        let entry =
            sign.entry(LogEvent::Note, &format!("set {}.", set.join(", ")));
        body.log_entry(&entry);
        mutations.push(Mutation::Note(entry));
    }
    issue.branch = branch_of(
        &issue.id,
        flow.field_with(&issue.kind, FieldRole::Branch)
            .and_then(|field| issue.text(&field.name)),
    );
    issue
        .log
        .extend(mutations.iter().filter_map(|mutation| match mutation {
            Mutation::Note(entry) => Some(entry.clone()),
            _ => None,
        }));
    let updated = crate::issue::Observation::mapped(issue, body);
    Ok(Change {
        observation: crate::issue::Observation {
            state: updated.state,
            ..observation.clone()
        },
        created: false,
        mutations,
    })
}

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

fn setting(
    home: &Snapshot,
    flow: &Flow,
    issue: &mut Issue,
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
        _ => assign(home, flow, issue, key, value),
    }
}

fn kept(flow: &Flow, issue: &Issue, field: &str, value: &Datum) -> Mutation {
    match (relation_of(flow, issue, field), issue.links.get(field)) {
        (Some(relation), Some(targets)) => Mutation::Relate {
            relation: String::from(field),
            many: relation.many,
            targets: targets.clone(),
        },
        _ => Mutation::Set {
            field: String::from(field),
            value: normalized(value),
        },
    }
}

fn relation_of<'a>(
    flow: &'a Flow,
    issue: &Issue,
    key: &str,
) -> Option<&'a super::Relation> {
    if flow.field(&issue.kind, key).is_some() {
        return None;
    }
    flow.relations.iter().find(|relation| {
        relation.name == key && relation.starts_from(&issue.kind)
    })
}

fn related(
    home: &Snapshot,
    from: ProjectId,
    name: &str,
) -> Result<IssueKey, String> {
    match name.split_whitespace().collect::<Vec<_>>().as_slice() {
        [id] => Ok(IssueKey {
            project: from,
            id: home.ids().read(from, id)?,
        }),
        [called, id] => {
            let project = home
                .project(called)
                .map(|known| known.id)
                .ok_or_else(|| format!("no project is called {called}"))?;
            Ok(IssueKey {
                project,
                id: home.ids().read(project, id)?,
            })
        }
        _ => Err(format!("{name:?} is not <id> or <project> <id>")),
    }
}

fn assign(
    home: &Snapshot,
    flow: &Flow,
    issue: &mut Issue,
    key: &str,
    value: &Datum,
) -> Result<(), String> {
    let kept = normalized(value);
    if flow.field(&issue.kind, key).is_some() {
        issue.fields.insert(String::from(key), kept);
    } else if let Some(relation) = relation_of(flow, issue, key) {
        let names = match (&kept, relation.many) {
            (Datum::Text(name), _) => Vec::from([name.clone()]),
            (Datum::Texts(names), true) => names.clone(),
            (Datum::Texts(_), false) => {
                return Err(String::from("points to one issue, not a list"));
            }
            (Datum::Bool(_), _) => return Err(String::from("is not an id")),
        };
        let targets = names
            .iter()
            .map(|name| related(home, issue.project, name))
            .collect::<Result<Vec<_>, _>>()?;
        issue.links.insert(String::from(key), targets);
    } else {
        issue.fields.insert(String::from(key), kept);
    }
    Ok(())
}

fn normalized(value: &Datum) -> Datum {
    match value {
        Datum::Text(text) => Datum::Text(String::from(text.trim())),
        Datum::Texts(texts) => Datum::Texts(
            texts
                .iter()
                .map(|text| text.trim())
                .filter(|text| !text.is_empty())
                .map(String::from)
                .collect(),
        ),
        Datum::Bool(flag) => Datum::Bool(*flag),
    }
}

fn checklist(
    issue: &Issue,
    body: &mut Body,
    change: &ChecklistChange,
) -> Result<(), String> {
    let missing = || String::from("its checklist doesn't read");
    match change {
        ChecklistChange::Add(text) => {
            body.add_item(needs(text, "a checklist item needs text")?);
            Ok(())
        }
        ChecklistChange::Check { item, done } => {
            body.check(picked(issue, item)?, *done).ok_or_else(missing)
        }
        ChecklistChange::Remove(item) => {
            body.remove_item(picked(issue, item)?).ok_or_else(missing)
        }
    }
}

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

fn needs<'a>(text: &'a str, missing: &str) -> Result<&'a str, String> {
    let kept = text.trim();
    if kept.is_empty() {
        Err(String::from(missing))
    } else {
        Ok(kept)
    }
}

#[must_use]
pub fn stamp(at: UnixSecs) -> String {
    let UnixSecs(seconds) = at;
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
