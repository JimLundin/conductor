use alloc::format;
use alloc::string::ToString;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use vcs::{
    DiffExtent, Error as GitError, Repository, SnapshotFile, WorkingCopy,
    Worktree,
};

use crate::log::HostSeq;
use crate::prelude::*;
use crate::session::{SessionDirectory, SessionId};

const BRANCHES: &str = "conductor/";

fn failure(context: &str, error: &str) -> GitError {
    GitError::failed(context, error)
}

#[must_use]
pub fn worktree(session: SessionId) -> String {
    session.to_string()
}

#[must_use]
pub fn branch(session: SessionId) -> String {
    format!("{BRANCHES}{}", worktree(session))
}

/// # Errors
/// The workspace branch is not owned by a session.
pub fn owner(workspace: &Workspace) -> Result<String, GitError> {
    let session = workspace
        .branch
        .strip_prefix(BRANCHES)
        .ok_or_else(|| failure("workspace branch", "not owned by Conductor"))?;
    SessionId::parse(session)
        .ok_or_else(|| failure("workspace branch", "not a session id"))?;
    Ok(String::from(session))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Workspace {
    pub repository: Repository,
    pub original: WorkingCopy,
    pub worktree: Worktree,
    /// The worktree's copy of the directory the session was started in.
    pub directory: SessionDirectory,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Snapshot {
    /// The existing `UserMessage` event for this turn; None for initial/autonomous turns.
    pub turn_start: Option<HostSeq>,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotDiff {
    pub snapshot: HostSeq,
    pub files: Vec<SnapshotFile>,
    pub extent: DiffExtent,
}

#[must_use]
pub fn ownership(workspace: &Workspace) -> vcs::plan::Ownership {
    vcs::plan::Ownership {
        repository: workspace.repository.0.clone(),
        original: workspace.original.0.clone(),
        worktree: workspace.worktree.0.clone(),
        directory: workspace.directory.0.clone(),
        branch: workspace.branch.clone(),
    }
}
