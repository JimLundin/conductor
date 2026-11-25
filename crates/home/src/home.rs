//! The home (docs/home.md): the projects the fleet works on, and where each
//! is checked out on this host.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::flow::Flow;
use crate::issue::{EditId, IssueChange, NewIssue, Observation};
use crate::{IssueId, ProjectId};

/// The host's clone of the home, and the projects it lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Home {
    /// The home's remote, as the host was given it.
    pub remote: String,
    /// When the clone last matched the remote, in seconds since the Unix
    /// epoch on the host's clock.
    #[serde(rename = "synced")]
    pub synced_secs: Option<u64>,
    pub sync: HomeSync,
    /// Issue changes the host has made that the remote doesn't have yet.
    pub unpushed: u64,
    /// The clone's `docs/agents/flow.toml`, which its issues are read
    /// against; `None` while it has none that loads, and so no issues.
    pub flow: Option<Box<Flow>>,
    /// As the clone lists them, by id.
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum HomeSync {
    /// The first clone is under way.
    Syncing,
    /// The last clone or pull succeeded.
    Synced,
    /// The host has a clone, but the last pull failed, for this reason: the
    /// projects are as of the last sync.
    Behind { reason: String },
    /// The host has no clone, for this reason, and so no projects.
    Failed { reason: String },
}

/// A project as the home describes it, and its checkouts on this host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Project {
    pub id: ProjectId,
    /// Unique in the home: what a person types to name it.
    pub name: String,
    /// What the app shows.
    pub title: String,
    /// `None` for a project that is local only, which can't be cloned.
    pub remote: Option<String>,
    pub profiles: Vec<String>,
    /// The profiles whose checks and configs it holds, for any project
    /// that names them.
    pub provides: Vec<String>,
    /// Every checkout of it on this host, the one a session starts in
    /// first. None: it isn't checked out here.
    pub checkouts: Vec<Checkout>,
    pub cloning: Cloning,
    /// By creation time, with identity breaking ties.
    pub issues: Vec<Observation>,
}

/// A repository on this host whose root commit and remote are a project's.
/// A session's project follows from its worktree's repository, which is
/// one of these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Checkout {
    pub path: CheckoutLocation,
    pub origin: Origin,
}

/// Where a project is checked out on a host, as the host's OS spells it.
/// Only the host reads it: a browser on a phone can't know a Windows
/// path's rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CheckoutLocation(pub String);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum Origin {
    /// Found in a directory the host searches.
    Found,
    /// The host cloned it into its data dir.
    Cloned,
}

/// Where the host's own clone of a project stands, after "Clone here".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Cloning {
    Idle,
    Running,
    Failed { reason: String },
}

/// A command about the host's clone of the home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum HomeCommand {
    /// Pulls the home now. `Accepted` once it has; how it went is in the
    /// home's `sync`.
    Sync,
    /// Clones the project into the host's data dir, for a project with no
    /// checkout here. `Accepted` once it has; how it went is in the
    /// project's `cloning`.
    Clone { project: ProjectId },
    /// Opens an issue, as `home issue new` does. Answers `IssueWritten`
    /// once the host has written it to its clone of the home; pushing it
    /// follows, and how that went is in the home's `sync`.
    ///
    /// The app mints `edit` once per edit and sends it again with each
    /// retry, so the home writes the edit once.
    CreateIssue { edit: EditId, issue: NewIssue },
    /// Answers `IssueWritten` once written, like `CreateIssue`.
    ChangeIssue {
        edit: EditId,
        issue: IssueId,
        change: IssueChange,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum HomeResult {
    Accepted,
    /// The issue a `CreateIssue` or `ChangeIssue` wrote, and what the
    /// flow warns of it.
    IssueWritten {
        issue: IssueId,
        warnings: Vec<String>,
    },
    Refused(HomeRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum HomeRefusal {
    /// The host was given no home.
    NoHome,
    UnknownProject,
    /// The project has no checkout on the host.
    NotCheckedOut,
    /// The project is local only: there is no remote to clone.
    NoRemote,
    UnknownIssue,
    /// The home can't be written now, for this reason.
    IssueNotSaved {
        reason: String,
    },
    /// The flow refuses the edit, for this reason, as the `home` command
    /// line would.
    IssueRefused {
        reason: String,
    },
}

impl HomeRefusal {
    /// Why the host refused the command, for a person to read.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::NoHome => String::from(
                "the host has no home; start it with --home <remote>",
            ),
            Self::UnknownProject => {
                String::from("the host's home lists no such project")
            }
            Self::NotCheckedOut => String::from(
                "the project isn't checked out on the host; clone it there first",
            ),
            Self::NoRemote => String::from(
                "the project is local only, so there is nothing to clone",
            ),
            Self::UnknownIssue => {
                String::from("the host's home lists no such issue")
            }
            Self::IssueNotSaved { reason } => {
                format!("the issue was not saved: {reason}")
            }
            Self::IssueRefused { reason } => reason.clone(),
        }
    }
}
