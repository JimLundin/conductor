use alloc::collections::BTreeMap;
use alloc::string::ToString;
use vcs::status::Checkout;

use crate::{Draws, ProjectId};
use serde::Serialize;

use crate::checkout::is_checkout;
use crate::layout;
use crate::prelude::*;
use crate::project::{Project, parse};
use vcs::discovery::Probe;

pub const HOME: &str = ".";

pub mod verify;

#[must_use]
pub fn local(home: &str, probes: Vec<Probe>) -> Vec<Probe> {
    probes
        .into_iter()
        .map(|probe| Probe {
            path: from_home(home, &probe.path),
            ..probe
        })
        .collect()
}

fn from_home(home: &str, path: &str) -> String {
    path.strip_prefix(home.trim_end_matches(['/', '\\']))
        .and_then(|rest| {
            if rest.is_empty() {
                Some(HOME)
            } else {
                rest.strip_prefix(['/', '\\'])
            }
        })
        .map_or_else(|| path.to_owned(), str::to_owned)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub name: String,
    pub remote: Option<String>,
    pub checkouts: Vec<String>,
    pub profiles: Vec<String>,
    pub provides: Vec<String>,
}

#[must_use]
pub fn placed(projects: &[Project], probes: &[Probe]) -> Vec<Placed> {
    by_name(projects)
        .into_iter()
        .map(|project| Placed {
            name: project.name.clone(),
            remote: project.remote.clone(),
            profiles: project.profiles.clone(),
            provides: project.provides.clone(),
            checkouts: probes
                .iter()
                .filter(|probe| is_checkout(project, probe))
                .map(|probe| probe.path.clone())
                .collect(),
        })
        .collect()
}

fn by_name(projects: &[Project]) -> Vec<&Project> {
    let mut sorted: Vec<&Project> = projects.iter().collect();
    sorted.sort_by(|one, other| one.name.cmp(&other.name));
    sorted
}

#[must_use]
pub fn checkouts(placed: &[Placed]) -> Vec<String> {
    placed
        .iter()
        .flat_map(|project| project.checkouts.iter().cloned())
        .collect()
}

#[must_use]
pub fn report(
    placed: &[Placed],
    states: &BTreeMap<String, Option<Checkout>>,
) -> String {
    placed
        .iter()
        .flat_map(|project| {
            let name = &project.name;
            let lines: Vec<String> = project
                .checkouts
                .iter()
                .map(|path| {
                    let state =
                        states.get(path).and_then(Option::as_ref).map_or_else(
                            || String::from("git status failed"),
                            |state| match state.changed {
                                0 => state.to_string(),
                                changed => {
                                    format!("{state}  [{changed} changed]")
                                }
                            },
                        );
                    format!("{name:<22} {path}: {state}\n")
                })
                .collect();
            if lines.is_empty() {
                Vec::from([format!("{name:<22} not checked out here\n")])
            } else {
                lines
            }
        })
        .collect()
}

#[must_use]
pub fn warnings(problems: &[String]) -> String {
    problems
        .iter()
        .map(|problem| format!("home: {problem}\n"))
        .collect::<Vec<String>>()
        .concat()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetch {
    pub said: String,
    pub clone: Option<(String, String)>,
}

#[must_use]
pub fn sync(placed: &[Placed]) -> Vec<Fetch> {
    placed
        .iter()
        .filter(|project| project.checkouts.is_empty())
        .map(|project| {
            project.remote.as_ref().map_or_else(
                || Fetch {
                    said: format!(
                        "skip {}: local only, no remote\n",
                        project.name
                    ),
                    clone: None,
                },
                |remote| Fetch {
                    said: format!("clone {} into {}\n", remote, project.name),
                    clone: Some((remote.clone(), project.name.clone())),
                },
            )
        })
        .collect()
}

#[must_use]
pub fn unmatched(projects: &[Project], probes: &[Probe]) -> Vec<Probe> {
    probes
        .iter()
        .filter(|probe| {
            !projects.iter().any(|project| is_checkout(project, probe))
        })
        .cloned()
        .collect()
}

const PROFILE_MARKERS: [(&str, &str); 3] = [
    ("pyproject.toml", "python"),
    ("Cargo.toml", "rust"),
    ("package.json", "typescript"),
];

#[must_use]
pub fn profiles(names: &[String]) -> Vec<String> {
    PROFILE_MARKERS
        .iter()
        .filter(|(marker, _)| names.iter().any(|name| name == marker))
        .map(|(_, profile)| String::from(*profile))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct New {
    pub probe: Probe,
    pub names: Vec<String>,
    pub draws: Draws,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scanning {
    pub home: String,
    pub projects: Vec<Project>,
    pub new: Vec<New>,
    pub problems: Vec<String>,
    pub doc: Result<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    pub writes: Vec<(String, String)>,
    pub said: String,
}

pub const DOC: &str = "AGENTS.md";

pub const START: &str = "<!-- repos:start";

pub const END: &str = "<!-- repos:end -->";

pub fn scan(scanning: Scanning) -> Result<Scan, String> {
    let Scanning {
        home,
        projects,
        new,
        problems,
        doc,
    } = scanning;
    if !problems.is_empty() {
        return Err(format!(
            "scan registers nothing while the home's projects don't read:\n{}",
            problems.join("\n")
        ));
    }
    let mut held: Vec<ProjectId> =
        projects.iter().map(|project| project.id).collect();
    let added = new
        .iter()
        .map(|repository| {
            let id =
                ProjectId::mint(&repository.draws, &held).ok_or_else(|| {
                    format!("no fresh project id for {}", repository.probe.path)
                })?;
            held.push(id);
            described(&home, &repository.probe, &repository.names, id)
        })
        .collect::<Result<Vec<(Project, String)>, String>>()?;
    let mut all = projects;
    all.extend(added.iter().map(|(project, _)| project.clone()));
    let table = table(&all);
    let rewritten = doc.map(|text| regenerated(&text, &table));
    let said = added
        .iter()
        .zip(&new)
        .map(|((project, _), repository)| {
            format!(
                "added {} as {} from {}\n",
                project.name,
                layout::project(&project.id.to_string()),
                repository.probe.path
            )
        })
        .chain(
            rewritten
                .as_ref()
                .err()
                .map(|why| format!("{DOC}: {why}\n")),
        )
        .chain(
            by_name(&all)
                .iter()
                .map(|project| format!("{}\n", project.name)),
        )
        .collect();
    let writes = added
        .into_iter()
        .map(|(project, text)| {
            (layout::project_file(&project.id.to_string()), text)
        })
        .chain(rewritten.ok().map(|text| (String::from(DOC), text)))
        .collect();
    Ok(Scan { writes, said })
}

#[derive(Serialize)]
struct Described<'a> {
    name: String,
    title: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote: Option<&'a str>,
    root: &'a str,
    profiles: Vec<String>,
}

fn described(
    home: &str,
    probe: &Probe,
    names: &[String],
    id: ProjectId,
) -> Result<(Project, String), String> {
    let title = match probe.path.as_str() {
        HOME => home,
        path => path.rsplit('/').next().unwrap_or(path),
    };
    let file = Described {
        name: title
            .chars()
            .map(|c| c.to_ascii_lowercase())
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect(),
        title,
        remote: probe.remote.as_deref(),
        root: probe.roots.last().map_or("", String::as_str),
        profiles: profiles(names),
    };
    let text = toml::to_string(&file).map_err(|err| err.to_string())?;
    let project = parse(&id.to_string(), &text)
        .map_err(|why| format!("{}: {why}", probe.path))?;
    Ok((project, text))
}

#[must_use]
pub fn table(projects: &[Project]) -> String {
    let rows = by_name(projects)
        .iter()
        .map(|project| {
            format!(
                "| `{}` | {} |\n",
                project.name,
                project.remote.as_deref().unwrap_or("local only")
            )
        })
        .collect::<Vec<String>>()
        .concat();
    format!(
        "{START} (generated by home repos scan from projects/) -->\n\
         | Project | Remote |\n|---|---|\n{rows}{END}\n"
    )
}

#[must_use]
pub fn regenerated(doc: &str, table: &str) -> String {
    let (text, _) = doc.split_inclusive('\n').fold(
        (String::new(), false),
        |(mut text, inside), line| {
            if line.starts_with(START) {
                text.push_str(table);
                (text, true)
            } else if inside {
                (text, !line.starts_with(END))
            } else {
                text.push_str(line);
                (text, false)
            }
        },
    );
    text
}

#[cfg(test)]
mod tests;
