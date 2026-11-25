use alloc::{string::String, vec::Vec};

use crate::{DiffExtent, Repository, SnapshotFile, WorkingCopy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotAttempt(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedWorkspace {
    pub id: WorkspaceId,
    pub directory: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub repository: Repository,
    pub original: WorkingCopy,
    pub root: String,
    pub directory: String,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepare {
    pub directory: String,
    pub name: WorkspaceId,
    pub base: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    pub workspace: OwnedWorkspace,
    pub attempt: SnapshotAttempt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    pub workspace: OwnedWorkspace,
    pub revision: Revision,
    pub attempt: SnapshotAttempt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compare {
    pub workspace: OwnedWorkspace,
    pub revision: Revision,
    pub baseline: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    pub files: Vec<SnapshotFile>,
    pub extent: DiffExtent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreserveAndRemove {
    pub workspace: OwnedWorkspace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Integrate {
    pub workspace: OwnedWorkspace,
    pub target: String,
    pub attempt: IntegrationId,
}
