//! Which local repositories are a project's checkouts on this host.
//!
//! A repository is one when its history has the project's root commit and
//! its `origin` is the project's remote, whatever its path; a local-only
//! project's checkouts have no `origin`.

use alloc::collections::BTreeMap;
use alloc::string::ToString;

use crate::{Checkout, CheckoutLocation, Origin, ProjectId};
use vcs::{discovery::Probe, remote::normal};

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
