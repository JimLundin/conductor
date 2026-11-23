//! The home's projects among the repositories on this machine.
//!
//! `home repos` shows which repositories under the home's folder, the home
//! among them, are each project's checkouts and where they stand; `home
//! repos sync` clones the missing ones; `home repos scan` registers the
//! rest and writes the project table of the home's `AGENTS.md`. The shell
//! lists the folders, runs Git and writes the files.

use alloc::collections::BTreeMap;
use alloc::string::ToString;

use proto::{Draws, ProjectId};
use serde::Serialize;

use crate::checkout::{Probe, is_checkout};
use crate::layout;
use crate::prelude::*;
use crate::project::{Project, parse};

/// The home itself, as a repository's path from the home.
pub const HOME: &str = ".";

/// `probes`, found as the host finds checkouts with the home at `home` as
/// both a known path and the one root, each by its path from the home: the
/// home's own as [`HOME`], one outside it as it is.
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

/// A project and the paths of its checkouts here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub name: String,
    pub remote: Option<String>,
    pub checkouts: Vec<String>,
}

/// Each project, by name, with its checkouts among `probes`.
#[must_use]
pub fn placed(projects: &[Project], probes: &[Probe]) -> Vec<Placed> {
    by_name(projects)
        .into_iter()
        .map(|project| Placed {
            name: project.name.clone(),
            remote: project.remote.clone(),
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

/// Every checkout's path, whose state the shell asks Git for.
#[must_use]
pub fn checkouts(placed: &[Placed]) -> Vec<String> {
    placed
        .iter()
        .flat_map(|project| project.checkouts.iter().cloned())
        .collect()
}

/// What `home repos` says: each project's checkouts here and their state,
/// from what `git status --porcelain --branch` printed in each, `None`
/// where it failed.
#[must_use]
pub fn report(
    placed: &[Placed],
    states: &BTreeMap<String, Option<String>>,
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
                        states.get(path).cloned().flatten().map_or_else(
                            || String::from("git status failed"),
                            |porcelain| state(&porcelain),
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

/// A checkout's branch, how far it is from its upstream, and how many of
/// its files changed.
fn state(porcelain: &str) -> String {
    let mut lines = porcelain.lines();
    let head = lines.next().unwrap_or_default();
    let branch = head.strip_prefix("## ").unwrap_or(head);
    let changed = lines.filter(|line| !line.is_empty()).count();
    if changed == 0 {
        String::from(branch)
    } else {
        format!("{branch}  [{changed} changed]")
    }
}

/// Why folders of the home's `projects/` aren't projects, as `home repos`
/// says it before its report.
#[must_use]
pub fn warnings(problems: &[String]) -> String {
    problems
        .iter()
        .map(|problem| format!("home: {problem}\n"))
        .collect::<Vec<String>>()
        .concat()
}

/// A clone `home repos sync` makes, or why it makes none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetch {
    pub said: String,
    /// The remote and the folder from the home it is cloned into.
    pub clone: Option<(String, String)>,
}

/// What `home repos sync` does: clones each project with a remote and no
/// checkout here into the home's folder of its name.
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

/// The repositories among `probes` that are no project's checkout.
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

/// The file at a repository's top that marks each standards profile.
const PROFILES: [(&str, &str); 3] = [
    ("pyproject.toml", "python"),
    ("Cargo.toml", "rust"),
    ("package.json", "typescript"),
];

/// The standards profiles a repository follows, by the `names` at its top.
#[must_use]
pub fn profiles(names: &[String]) -> Vec<String> {
    PROFILES
        .iter()
        .filter(|(marker, _)| names.iter().any(|name| name == marker))
        .map(|(_, profile)| String::from(*profile))
        .collect()
}

/// A repository that is no project's checkout, with the names at its top
/// and the draws for the id it is registered as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct New {
    pub probe: Probe,
    pub names: Vec<String>,
    pub draws: Draws,
}

/// What `home repos scan` reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scanning {
    /// The name of the home's folder, which titles the home itself.
    pub home: String,
    /// The home's projects, whose ids a new project can't take.
    pub projects: Vec<Project>,
    pub new: Vec<New>,
    /// Why any folder of the home's `projects/` isn't a project.
    pub problems: Vec<String>,
    /// The text of the home's `AGENTS.md`, or why it couldn't be read.
    pub doc: Result<String, String>,
}

/// What `home repos scan` writes and says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    /// Each file by its path from the home, with its text.
    pub writes: Vec<(String, String)>,
    pub said: String,
}

/// The home's file that lists its projects.
pub const DOC: &str = "AGENTS.md";

/// The line the project table starts at; the rest of it is the table's.
pub const START: &str = "<!-- repos:start";

/// The line the project table ends at.
pub const END: &str = "<!-- repos:end -->";

/// What `home repos scan` makes of `scanning`: a `project.toml` for each
/// new repository, in a folder of a fresh id, and the home's `AGENTS.md`
/// with its project table of every project.
///
/// # Errors
///
/// A folder of the home's `projects/` isn't a project, so any repository
/// might be its; or no draw gives a repository an id the home doesn't
/// hold.
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

/// A `project.toml`'s fields as scan writes them.
#[derive(Serialize)]
struct Described<'a> {
    name: String,
    title: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote: Option<&'a str>,
    root: &'a str,
    profiles: Vec<String>,
}

/// The project the repository `probe` is, as `id`, named after its folder,
/// and its `project.toml`'s text.
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

/// The project table: each project, by name, with its remote.
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

/// `doc` with `table` in place of the lines from its [`START`] line to its
/// [`END`] line.
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
