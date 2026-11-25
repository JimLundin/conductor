use alloc::{format, string::String};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error(
        "the selected directory is not a Git repository with a committed HEAD"
    )]
    NotRepository,
    #[error("{0}")]
    Failed(String),
}

impl Error {
    #[must_use]
    pub fn failed(context: &str, error: &str) -> Self {
        Self::Failed(format!("{context}: {error}"))
    }
}

/// The root of the repository a session's worktree is added to, as the
/// host's OS spells it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Repository(pub String);

/// The directory of the owner's working copy a session was started in,
/// which its workspace isolates it from, as the host's OS spells it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkingCopy(pub String);

/// The Git worktree that holds a session's workspace, as the host's OS
/// spells it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Worktree(pub String);

/// Where a removed session's branch was kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Kept {
    /// In the checkout only: it has no `origin`.
    Local,
    Pushed {
        remote: String,
    },
    /// In the checkout only, since the push failed.
    PushFailed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotFile {
    pub path: String,
    pub change: DiffChange,
    pub patch: FilePatch,
}

/// What a snapshot did to a file, as Git's diff status says.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum DiffChange {
    /// Added: the whole file is new.
    Write,
    /// Modified, or any other status.
    Edit,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum FilePatch {
    Text(String),
    Binary,
    TooLarge,
    Unsupported { reason: String },
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum DiffExtent {
    Complete,
    Truncated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub change: DiffChange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    pub path: String,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merge {
    Tree(String),
    Conflicts(alloc::vec::Vec<String>),
}
