use alloc::{string::String, vec::Vec};

use crate::plan::Ownership;
use crate::{DiffExtent, SnapshotFile};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepare {
    pub directory: String,
    pub name: String,
    pub branch: String,
    pub base: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    pub workspace: Ownership,
    pub expected_branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compare {
    pub workspace: Ownership,
    pub revision: String,
    pub baseline: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    pub files: Vec<SnapshotFile>,
    pub extent: DiffExtent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreserveAndRemove {
    pub workspace: Ownership,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntegrationId(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Integrate {
    pub workspace: Ownership,
    pub target: String,
    pub attempt: IntegrationId,
}
