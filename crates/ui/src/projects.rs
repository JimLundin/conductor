//! The projects view: each paired host's home and how it syncs, then each
//! project the homes list, with its checkouts on each host that lists it.

use alloc::collections::BTreeMap;

use crate::prelude::*;
use proto::{
    Checkout, Cloning, CommandBody, Home, HomeSync, Origin, ProjectId,
};

use crate::host::{Host, HostId, by_name};
use crate::issues::{self, Drafts, IssueList};
use crate::{At, Effect, Intent, ReplyTo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectIntent {
    /// Pull the host's home now.
    Sync,
    /// Clone the project into the host's data dir.
    Clone(ProjectId),
}

impl ProjectIntent {
    pub(crate) fn send(self, at: At) -> Vec<Effect> {
        let body = match self {
            Self::Sync => CommandBody::SyncHome,
            Self::Clone(project) => CommandBody::CloneProject { project },
        };
        Vec::from([at.send(body, ReplyTo::Notice)])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectsScreen {
    pub subtitle: &'static str,
    pub homes: Vec<HomeLine>,
    pub projects: Vec<ProjectGroup>,
    /// What the screen says when no home lists a project.
    pub empty: Option<String>,
    /// The last home command's refusal.
    pub notice: Option<String>,
}

/// A host's home, in a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeLine {
    pub key: String,
    pub host: String,
    /// Also its `data-sync`: `offline`, `none`, `syncing`, `synced`,
    /// `behind` or `failed`.
    pub state: &'static str,
    pub text: String,
    /// Pulls the home now, on a live host with a home.
    pub sync: Option<Intent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectGroup {
    pub key: String,
    pub title: String,
    /// Its name, which a person types to name it, and its profiles.
    pub detail: String,
    /// One row for each host whose home lists it.
    pub hosts: Vec<CheckoutRow>,
    pub issues: IssueList,
}

/// Where a project is checked out on one host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutRow {
    pub key: String,
    pub host: String,
    /// Also its `data-checkout`: `checked-out`, `missing`, `cloning` or
    /// `failed`.
    pub state: &'static str,
    /// Each checkout's path, and whether the host cloned it.
    pub paths: Vec<String>,
    pub text: Option<String>,
    pub clone: Option<CloneButton>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneButton {
    pub intent: Intent,
    /// What the button says to a screen reader: which project, on which
    /// host.
    pub label: String,
}

/// The screen over every paired host. A project's issues are read from
/// the first live host whose home lists it, else the first that does.
pub fn screen(
    hosts: &BTreeMap<HostId, Host>,
    drafts: &Drafts,
    notice: Option<String>,
) -> ProjectsScreen {
    let listed = by_name(hosts);
    let mut rows = BTreeMap::<ProjectId, Vec<CheckoutRow>>::new();
    let mut read =
        BTreeMap::<ProjectId, (HostId, &Host, &proto::Project)>::new();
    for (id, host) in &listed {
        for project in host
            .fold
            .home
            .as_ref()
            .map_or(&[][..], |home| &home.projects)
        {
            rows.entry(project.id.clone())
                .or_default()
                .push(row(*id, host, project));
            let better = read.get(&project.id).is_none_or(|(_, first, _)| {
                host.fold.live() && !first.fold.live()
            });
            if better {
                read.insert(project.id.clone(), (*id, host, project));
            }
        }
    }
    let projects = read
        .into_iter()
        .map(|(key, (id, host, project))| ProjectGroup {
            key: key.0.clone(),
            title: project.title.clone(),
            detail: detail(project),
            hosts: rows.remove(&key).unwrap_or_default(),
            issues: issues::list(project, Some((id, host)), drafts),
        })
        .collect::<Vec<ProjectGroup>>();
    ProjectsScreen {
        subtitle: "The projects each host's home lists, and where each is checked out there.",
        homes: listed.iter().map(|(id, host)| home_line(*id, host)).collect(),
        empty: projects.is_empty().then(|| {
            String::from(
                "No home lists a project yet. A host lists its home's projects once it has cloned it.",
            )
        }),
        projects,
        notice,
    }
}

fn detail(project: &proto::Project) -> String {
    core::iter::once(&project.name)
        .chain(&project.profiles)
        .cloned()
        .collect::<Vec<String>>()
        .join(" · ")
}

fn home_line(id: HostId, host: &Host) -> HomeLine {
    let name = host.name();
    let (state, text) = match (
        host.fold.status.as_ref(),
        host.fold.home.as_ref(),
    ) {
        (None, _) => ("offline", format!("Not connected to {name} yet.")),
        (Some(_), None) => (
            "none",
            format!(
                "{name} has no home. Start it with --home <remote> to list its projects."
            ),
        ),
        (Some(_), Some(home)) => sync_line(home),
    };
    HomeLine {
        key: id.key(),
        sync: (host.fold.live() && host.fold.home.as_ref().is_some())
            .then_some(Intent::Project(id, ProjectIntent::Sync)),
        host: name,
        state,
        text,
    }
}

fn sync_line(home: &Home) -> (&'static str, String) {
    let remote = &home.remote;
    match &home.sync {
        HomeSync::Syncing => {
            ("syncing", format!("Cloning the home from {remote}…"))
        }
        HomeSync::Synced => (
            "synced",
            format!("Home synced {} from {remote}.", when(home.synced)),
        ),
        HomeSync::Behind { reason } => (
            "behind",
            format!(
                "Home as of {}; the last pull from {remote} failed: {reason}",
                when(home.synced)
            ),
        ),
        HomeSync::Failed { reason } => (
            "failed",
            format!("No copy of the home from {remote}: {reason}"),
        ),
    }
}

fn row(id: HostId, host: &Host, project: &proto::Project) -> CheckoutRow {
    let name = host.name();
    let (state, text) = match (&project.cloning, project.checkouts.first()) {
        (Cloning::Running, _) => ("cloning", Some(String::from("Cloning…"))),
        (_, Some(_)) => ("checked-out", None),
        (Cloning::Failed { reason }, None) => {
            ("failed", Some(format!("Clone failed: {reason}")))
        }
        (Cloning::Idle, None) => {
            ("missing", Some(String::from("Not checked out")))
        }
    };
    let offered = state != "checked-out"
        && state != "cloning"
        && project.remote.is_some();
    CheckoutRow {
        key: id.key(),
        paths: project.checkouts.iter().map(path).collect(),
        clone: (offered && host.fold.live()).then(|| CloneButton {
            intent: Intent::Project(
                id,
                ProjectIntent::Clone(project.id.clone()),
            ),
            label: format!("Clone {} on {name}", project.title),
        }),
        host: name,
        state,
        text,
    }
}

fn path(checkout: &Checkout) -> String {
    match checkout.origin {
        Origin::Found => checkout.path.0.clone(),
        Origin::Cloned => format!("{} (cloned here)", checkout.path.0),
    }
}

/// A time in seconds since the Unix epoch as a date and a UTC time, or
/// "never".
#[must_use]
pub fn when(seconds: Option<u64>) -> String {
    seconds.map_or_else(
        || String::from("never"),
        |at| {
            let (days, rest) = (at / 86_400, at % 86_400);
            let (year, month, day) = proto::civil(days);
            format!(
                "{year}-{month:02}-{day:02} {:02}:{:02} UTC",
                rest / 3600,
                rest % 3600 / 60
            )
        },
    )
}
