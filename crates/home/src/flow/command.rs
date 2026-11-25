use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::ToString;

use crate::IssueId;
use crate::flow::{FieldRole, FieldType, Flow, Severity};
use crate::issue::{
    ChecklistChange, Datum, Entry, ID, Issue, IssueChange, Item, KIND,
    LogEvent, NewIssue, Part, STAGE, STATUS, TITLE,
};
use serde::{Deserialize, Serialize};

use super::PATH;
use super::check::{check, of_issue};
use super::eval::Move;
use super::graph::branch_of;
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
      Lists tracked issues as JSON with freshness, coverage and refresh failures.
  home issue show <project> <reference> [--offline] [--data-dir <dir>]
      Reads one tracked issue by qualified ID or project reference such as '#42'.
      Both commands refresh by default. --offline reads the host cache.
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

Each write signs and dates the log and is read back against the flow: a
structural problem or a broken error rule refuses it, and anything softer
is written with a warning.
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
    BodyFrom { id: IssueId, path: String },
    Wrong(String),
    Usage,
}

impl Command {
    #[must_use]
    pub fn file(&self) -> Option<&str> {
        match self {
            Self::BodyFrom { path, .. } => Some(path),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edit {
    New {
        id: IssueId,
        issue: NewIssue,
    },
    Change {
        id: IssueId,
        changes: Vec<IssueChange>,
    },
}

impl Edit {
    #[must_use]
    pub fn id(&self) -> IssueId {
        match self {
            Self::New { id, .. } | Self::Change { id, .. } => id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    pub now_ms: u64,
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mutation {
    Set { field: String, value: Datum },
    Title(String),
    Move { part: Part, to: String },
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
pub struct Outcome {
    pub changes: Vec<Change>,
    pub output: String,
    pub failed: bool,
}

impl Outcome {
    #[must_use]
    pub fn ended(&self, saved: Result<(), String>) -> (String, u8) {
        let code = u8::from(self.failed || saved.is_err());
        let text = saved
            .map_or_else(|reason| refusal(&reason), |()| self.output.clone());
        (text, code)
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
            let kind = home.issue(&id).map_or("", |issue| issue.kind.as_str());
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
                changes: Vec::new(),
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
    match plan(home, &edit, context) {
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
            failed: false,
        },
        Err(reason) => refused(refusal(&reason)),
    }
}

const fn refused(output: String) -> Outcome {
    Outcome {
        changes: Vec::new(),
        output,
        failed: true,
    }
}

pub fn plan(
    home: &Snapshot,
    edit: &Edit,
    context: &Context,
) -> Result<Planned, String> {
    let flow = home.flow()?;
    let sign = Signer {
        at: stamp(context.now_ms),
        now_ms: context.now_ms,
        by: context.by.clone(),
    };
    let change = match edit {
        Edit::New { id, issue } => opened(home, flow, id, &sign, issue)?,
        Edit::Change { id, changes } => {
            let observation = home.observation(id).ok_or_else(|| {
                home.graph().problems.iter().find(|problem| problem.record == id.to_string())
                    .map_or_else(|| format!("no issue is called {id}"), |problem| format!("{id} doesn't read as the flow declares: {problem}"))
            })?;
            observation.editable().map_err(|reason| {
                format!("{id} doesn't read as the flow declares: {reason}")
            })?;
            changed(flow, observation, changes, &sign)?
        }
    };
    finished(home, flow, change)
}

struct Signer {
    now_ms: u64,
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
    change: Change,
) -> Result<Planned, String> {
    let (issue, _) = change.observation.editable()?;
    let after = home.with(change.observation.clone());
    let graph = after.graph();
    let name = home
        .projects()
        .iter()
        .find(|project| project.id == issue.project)
        .map_or_else(
            || issue.project.to_string(),
            |project| project.name.clone(),
        );
    let subject = format!("{name}/{}", issue.id);
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
        from: home.issue(&issue.id),
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
        created: Some(sign.now_ms),
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
            assign(flow, &mut issue, field, value)?;
        }
        mutations.push(Mutation::Set {
            field: field.clone(),
            value: normalized(value),
        });
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
                setting(flow, &mut issue, field, value)?;
                mutations.push(Mutation::Set {
                    field: field.clone(),
                    value: normalized(value),
                });
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
        _ => assign(flow, issue, key, value),
    }
}

fn assign(
    flow: &Flow,
    issue: &mut Issue,
    key: &str,
    value: &Datum,
) -> Result<(), String> {
    let kept = normalized(value);
    if flow.field(&issue.kind, key).is_some() {
        issue.fields.insert(String::from(key), kept);
    } else if let Some(relation) = flow.relations.iter().find(|relation| {
        relation.name == key
            && (relation.from.is_empty() || relation.from.contains(&issue.kind))
    }) {
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
            .map(|name| {
                IssueId::parse(name)
                    .ok_or_else(|| format!("{name} is no issue"))
            })
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
pub fn stamp(now_ms: u64) -> String {
    let seconds = now_ms / 1000;
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
