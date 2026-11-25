//! Which local repositories are a project's checkouts on this host.
//!
//! A repository is one when its history has the project's root commit and
//! its `origin` is the project's remote, whatever its path; a local-only
//! project's checkouts have no `origin`.

use alloc::collections::BTreeMap;
use alloc::string::ToString;

use crate::{Checkout, CheckoutLocation, Origin, ProjectId};

use crate::prelude::*;
use crate::project::Project;

/// A directory the host searches for checkouts, and the directories in it
/// and in those, each by its path relative to it (`apps/conductor`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    pub path: String,
    pub entries: Vec<String>,
}

const SET_ASIDE: [&str; 2] = ["_old", "Archives"];

/// The paths to look for checkouts at, each once: the `known` ones, then
/// each directory in each root that isn't hidden, in a hidden one or in
/// one set aside.
#[must_use]
pub fn candidates(roots: &[Root], known: &[String]) -> Vec<String> {
    let searched = roots.iter().flat_map(|root| {
        root.entries
            .iter()
            .filter(|entry| {
                !entry.split('/').any(|name| name.starts_with('.'))
                    && !SET_ASIDE
                        .contains(&entry.split('/').next().unwrap_or_default())
            })
            .map(|entry| join(&root.path, entry))
    });
    known.iter().cloned().chain(searched).fold(
        Vec::new(),
        |mut listed, path| {
            if !listed.contains(&path) {
                listed.push(path);
            }
            listed
        },
    )
}

/// Where the host clones each of `projects`, as [`clone_path`] says.
#[must_use]
pub fn clone_paths(clones: &str, projects: &[Project]) -> Vec<String> {
    projects
        .iter()
        .map(|project| clone_path(clones, &project.id))
        .collect()
}

/// Where the host clones `project`: `<clones>/<project>`.
#[must_use]
pub fn clone_path(clones: &str, project: &ProjectId) -> String {
    join(clones, &project.to_string())
}

fn join(directory: &str, name: &str) -> String {
    format!("{}/{name}", directory.trim_end_matches('/'))
}

/// What Git says of a repository at a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub path: String,
    /// The root commit of its `HEAD`'s first-parent history.
    pub roots: Vec<String>,
    pub remote: Option<String>,
}

/// What a shell asks Git at a candidate path, in [`probed`]'s order: the
/// top of the repository there, the root of its `HEAD`'s first-parent
/// history, and its `origin`.
pub const ASKED: [&[&str]; 3] = [
    &["rev-parse", "--show-toplevel"],
    &["rev-list", "--max-parents=0", "--first-parent", "HEAD"],
    &["remote", "get-url", "origin"],
];

/// The repository at `path`, if it is the top of one.
///
/// `top`, `roots` and `remote` are what Git printed for each of [`ASKED`]
/// there, each `None` if it failed. A directory inside another repository,
/// or one with no commit, is none.
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
                            path: CheckoutLocation(probe.path.clone()),
                            origin: if probe.path == own {
                                Origin::Cloned
                            } else {
                                Origin::Found
                            },
                        });
                    }
                    found
                });
            (project.id, found)
        })
        .filter(|(_, found)| !found.is_empty())
        .collect()
}

/// Whether the repository `probe` is a checkout of `project`.
#[must_use]
pub fn is_checkout(project: &Project, probe: &Probe) -> bool {
    probe.roots.contains(&project.root)
        && project.remote.as_deref().map(normal)
            == probe.remote.as_deref().map(normal)
}

/// The repository address suitable for display.
#[must_use]
pub fn display(remote: &str) -> String {
    match remote.split_once("://") {
        Some((scheme, address)) => {
            let (host, location) = authority(address);
            let path = public_path(location);
            format!("{scheme}://{host}{path}")
        }
        None => scp(remote).map_or_else(
            || remote.to_owned(),
            |(host, path)| format!("{host}:{path}"),
        ),
    }
}

fn authority(address: &str) -> (&str, &str) {
    let (login, path) = address
        .split_at(address.find(['/', '?', '#']).unwrap_or(address.len()));
    (without_user(login), path)
}

fn public_path(path: &str) -> &str {
    path.split(['?', '#']).next().unwrap_or_default()
}

fn without_user(login: &str) -> &str {
    login.rsplit_once('@').map_or(login, |(_, host)| host)
}

fn scp(remote: &str) -> Option<(&str, &str)> {
    let offset = remote
        .find('[')
        .filter(|at| *at == 0 || remote.as_bytes().get(at - 1) == Some(&b'@'))
        .and_then(|at| remote.get(at..)?.find(']').map(|end| at + end + 1))
        .unwrap_or(0);
    let separator = remote.get(offset..)?.find(':')? + offset;
    let (login, path_with_separator) = remote.split_at(separator);
    let path = path_with_separator.strip_prefix(':')?;
    if login.is_empty()
        || login.contains(['/', '\\'])
        || (login.len() == 1 && path.starts_with(['/', '\\']))
    {
        return None;
    }
    Some((without_user(login), path))
}

/// Compares repository addresses with case-insensitive network hosts.
#[must_use]
pub fn normal(remote: &str) -> String {
    let network = match remote.split_once("://") {
        Some((scheme, address)) => {
            (!scheme.eq_ignore_ascii_case("file")).then(|| {
                let (host, path) = authority(address);
                (host, public_path(path))
            })
        }
        None => scp(remote),
    };
    network.map_or_else(
        || format!("local:{remote}"),
        |(host, path)| {
            let trimmed = path.trim_matches('/');
            let repository = trimmed.strip_suffix(".git").unwrap_or(trimmed);
            format!("network:{}/{repository}", host.to_ascii_lowercase())
        },
    )
}
