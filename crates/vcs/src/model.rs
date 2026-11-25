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
    #[error("snapshot attempt {attempt} requires reconciliation: {reason}")]
    Reconciliation { attempt: String, reason: String },
}

impl Error {
    #[must_use]
    pub fn failed(context: &str, error: &str) -> Self {
        Self::Failed(format!("{context}: {error}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Repository(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkingCopy(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Worktree(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Kept {
    Local,
    Pushed { remote: String },
    PushFailed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotFile {
    pub path: String,
    pub change: DiffChange,
    pub patch: FilePatch,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum DiffChange {
    Write,
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
