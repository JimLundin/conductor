use super::Observation;
use crate::{IssueId, ProjectId, UnixMillis, backend::Refusal, prelude::*};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lookup {
    pub project: ProjectId,
    pub id: IssueId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListPage {
    pub project: ProjectId,
    pub continuation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub records: Vec<Observation>,
    pub next: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error,
)]
pub enum ReadFailure {
    #[error("tracker refused the read: {0:?}")]
    Refused(Refusal),
    #[error("invalid tracker configuration")]
    InvalidConfiguration,
    #[error("host credential is missing")]
    CredentialMissing,
    #[error("host credential has expired")]
    CredentialExpired,
    #[error("host credential store is unavailable")]
    CredentialStoreUnavailable,
    #[error("tracker refused authentication")]
    AuthenticationRefused,
    #[error("tracker denied permission")]
    PermissionDenied,
    #[error("tracker rate limited the request")]
    RateLimited,
    #[error("issue or tracker unavailable or inaccessible")]
    Unavailable,
    #[error("issue was deleted")]
    Deleted,
    #[error("issue identity or repository changed")]
    IdentityConflict,
    #[error("tracker returned an invalid response")]
    InvalidResponse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadMode {
    Refresh,
    Cached,
    Failed(ReadFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Freshness {
    Fresh,
    Stale,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReadStatus {
    pub freshness: Freshness,
    pub complete: bool,
    pub observed_at: Option<UnixMillis>,
    pub failure: Option<ReadFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IssueListing {
    pub records: Vec<Observation>,
    pub status: ReadStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IssueReading {
    pub record: Option<Observation>,
    pub status: ReadStatus,
}
