use serde::{Deserialize, Serialize};

use super::{Lookup, ReadFailure};
use crate::{IssueKey, prelude::*};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationChange {
    pub relation: String,
    pub targets: Vec<IssueKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationRequest {
    pub issue: Lookup,
    pub change: RelationChange,
    pub seen: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationStep {
    pub effect: String,
    pub request: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relating {
    pub revision: String,
    pub steps: Vec<RelationStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Unrelatable {
    #[error(transparent)]
    Read(ReadFailure),
    #[error("the tracker cannot hold the relation: {0}")]
    Unheld(String),
}
