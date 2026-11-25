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
    pub directory: SessionDirectory,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Snapshot {
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
