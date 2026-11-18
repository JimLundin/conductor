//! Which local repositories are a project's checkouts on this host.
//!
//! A repository is one when its history has the project's root commit and
//! its `origin` is the project's remote, whatever its path; a local-only
//! project's checkouts have no `origin`.

use alloc::collections::BTreeMap;

use proto::{Checkout, HostPath, Origin, ProjectId};

use crate::prelude::*;
use crate::project::Project;

/// A directory the host searches for checkouts, and the names of the
/// directories in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub path: String,
    pub entries: Vec<String>,
}

/// The paths to look for checkouts at, each once: those found before, then
/// in each root each project's usual path and each directory in it, then
/// the host's own clones.
#[must_use]
pub fn candidates(
    roots: &[Root],
    projects: &[Project],
    known: &[String],
    clones: &str,
) -> Vec<String> {
    let usual = roots.iter().flat_map(|root| {
        let paths = projects
            .iter()
            .filter_map(|project| project.path.as_deref());
        let entries = root
            .entries
            .iter()
            .map(String::as_str)
            .filter(|entry| !entry.starts_with('.'));
        paths.chain(entries).map(|name| join(&root.path, name))
    });
    let own = projects
        .iter()
        .map(|project| clone_path(clones, &project.id));
    known.iter().cloned().chain(usual).chain(own).fold(
        Vec::new(),
        |mut listed, path| {
            if !listed.contains(&path) {
                listed.push(path);
            }
            listed
        },
    )
}

/// Where the host clones `project`: `<clones>/<project>`.
#[must_use]
pub fn clone_path(clones: &str, project: &ProjectId) -> String {
    join(clones, &project.0)
}

fn join(directory: &str, name: &str) -> String {
    format!("{}/{name}", directory.trim_end_matches('/'))
}

/// What Git says of a repository at a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub path: String,
    /// The root commits of its `HEAD`'s history.
    pub roots: Vec<String>,
    pub remote: Option<String>,
}

/// The repository at `path`, if it is the top of one.
///
/// `top` is what `git rev-parse --show-toplevel` printed there, `roots`
/// what `git rev-list --max-parents=0 HEAD` did, and `remote` what `git
/// remote get-url origin` did, each `None` if it failed. A directory
/// inside another repository, or one with no commit, is none.
#[must_use]
pub fn probed(
    path: &str,
    top: Option<&str>,
    roots: Option<&str>,
    remote: Option<&str>,
) -> Option<Probe> {
    let commits = roots?.lines().map(str::to_owned).collect::<Vec<String>>();
    (top?.trim_end() == path && !commits.is_empty()).then(|| Probe {
        path: path.to_owned(),
        roots: commits,
        remote: remote
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_owned),
    })
}

/// Each project's checkouts among `probes`, in the probes' order. One at
/// the project's place in `clones` is the host's own clone.
#[must_use]
pub fn matched(
    projects: &[Project],
    probes: &[Probe],
    clones: &str,
) -> BTreeMap<ProjectId, Vec<Checkout>> {
    projects
        .iter()
        .map(|project| {
            let own = clone_path(clones, &project.id);
            let found = probes
                .iter()
                .filter(|probe| is_checkout(project, probe))
                .fold(Vec::<Checkout>::new(), |mut found, probe| {
                    if found.iter().all(|seen| seen.path.0 != probe.path) {
                        found.push(Checkout {
                            path: HostPath(probe.path.clone()),
                            origin: if probe.path == own {
                                Origin::Cloned
                            } else {
                                Origin::Found
                            },
                        });
                    }
                    found
                });
            (project.id.clone(), found)
        })
        .filter(|(_, found)| !found.is_empty())
        .collect()
}

fn is_checkout(project: &Project, probe: &Probe) -> bool {
    probe.roots.contains(&project.root)
        && project.remote.as_deref().map(normal)
            == probe.remote.as_deref().map(normal)
}

/// A remote as the same repository whichever way it is spelled: no
/// scheme, user or `.git`, with `host:path` as `host/path`, in lower case.
#[must_use]
pub fn normal(remote: &str) -> String {
    let trimmed = remote.trim().trim_end_matches('/');
    let url = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let unschemed = url.split_once("://").map_or(url, |(_, after)| after);
    let rest = unschemed
        .split_once('@')
        .map_or(unschemed, |(_, host)| host);
    let scp = !url.contains("://") && !rest.starts_with('/');
    let spelled = if scp {
        rest.replacen(':', "/", 1)
    } else {
        rest.to_owned()
    };
    spelled.to_lowercase()
}
