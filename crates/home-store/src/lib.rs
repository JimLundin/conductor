mod contract;
mod memory;

use std::fmt;

use home::canvas::{
    Broken, CanvasEntry, CanvasError, CanvasFile, CanvasKey, Held,
};
use home::{IssueId, ProjectId};

pub use contract::{Plant, contract};
pub use memory::Memory;

pub trait CanvasStore {
    /// # Errors
    ///
    /// The store cannot be read.
    fn all(&self) -> Result<Held<CanvasKey>, StoreError>;

    /// # Errors
    ///
    /// The store cannot be read.
    fn list(
        &self,
        project: &ProjectId,
        issue: &IssueId,
    ) -> Result<Held<Result<CanvasEntry, Broken>>, StoreError>;

    /// # Errors
    ///
    /// The store cannot be read, or the file is not a canvas.
    fn get(&self, key: &CanvasKey) -> Result<Option<CanvasFile>, StoreError>;

    /// # Errors
    ///
    /// The store cannot be written.
    fn put(
        &mut self,
        key: &CanvasKey,
        file: &CanvasFile,
    ) -> Result<(), StoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    Unavailable { reason: String },
    Invalid { key: CanvasKey, error: CanvasError },
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
        }
    }
}

impl std::error::Error for StoreError {}
