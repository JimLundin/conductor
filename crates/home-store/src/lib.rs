mod contract;
mod memory;
mod stores;

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use home::canvas::{
    Broken, CanvasEntry, CanvasError, CanvasFile, CanvasKey, Held,
};
use home::{IssueId, ProjectId};

pub use contract::{Plant, contract};
pub use memory::Memory;
pub use stores::{Keeper, Kept, Stores};

pub type Putting<'a> =
    Pin<Box<dyn Future<Output = Result<(), StoreError>> + Send + 'a>>;

pub trait CanvasStore {
    fn all(&self) -> Result<Held<CanvasKey>, StoreError>;

    fn list(
        &self,
        project: &ProjectId,
        issue: &IssueId,
    ) -> Result<Held<Result<CanvasEntry, Broken>>, StoreError>;

    fn get(&self, key: &CanvasKey) -> Result<Option<CanvasFile>, StoreError>;

    fn put<'a>(
        &'a mut self,
        key: &'a CanvasKey,
        file: &'a CanvasFile,
    ) -> Putting<'a>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    Unavailable {
        reason: String,
    },
    Refused {
        key: CanvasKey,
        reason: String,
    },
    Invalid {
        key: CanvasKey,
        error: Box<CanvasError>,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable { reason } => out.write_str(reason),
            Self::Invalid { key, error } => write!(
                out,
                "canvas {} of {}/{}: {error}",
                key.name, key.project, key.issue
            ),
            Self::Refused { key, reason } => write!(
                out,
                "canvas {} of {}/{}: {reason}",
                key.name, key.project, key.issue
            ),
        }
    }
}

impl std::error::Error for StoreError {}
