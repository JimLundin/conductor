//! The home (docs/home.md): the projects the fleet works on, and where each
//! is checked out on this host.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::flow::Flow;
use crate::{HostPath, Issue, IssueId};

/// A project by its folder's name in the home's `projects/`, a lowercase
/// ULID. Its name and title may change; its id never does.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct ProjectId(pub String);

/// The host's clone of the home, and the projects it lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Home {
    /// The home's remote, as the host was given it.
    pub remote: String,
    /// When the clone last matched the remote, in seconds since the Unix
    /// epoch on the host's clock.
    pub synced: Option<u64>,
    pub sync: HomeSync,
    /// Issue changes the host has made that the remote doesn't have yet.
    pub unpushed: u64,
    /// The clone's `docs/agents/flow.toml`, which its issues are read
    /// against; `None` while it has none that loads, and so no issues.
    pub flow: Option<Box<Flow>>,
    /// As the clone lists them, by id.
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// By id, which orders them by when they were opened.
    pub issues: Vec<Issue>,
}

/// A repository on this host whose root commit and remote are a project's.
/// A session's project follows from its worktree's repository, which is
/// one of these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkout {
    pub path: HostPath,
    pub origin: Origin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    /// Found in a directory the host searches.
    Found,
    /// The host cloned it into its data dir.
    Cloned,
}

/// Where the host's own clone of a project stands, after "Clone here".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cloning {
    Idle,
    Running,
    Failed { reason: String },
}

/// Where a new session runs: a directory, a project's checkout, or an
/// issue's project's checkout on a branch cut from the issue's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartAt {
    Directory(HostPath),
    Project(ProjectId),
    Issue(IssueId),
}
