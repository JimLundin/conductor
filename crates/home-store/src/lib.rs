//! Where the home's canvases are kept (ADR 0006): the [`CanvasStore`]
//! contract, which the core names canvases to by [`CanvasKey`] and never
//! by where they are, an in-memory store and the [`contract`] suite every
//! store passes.

mod contract;
mod memory;

use std::fmt;

use home::canvas::{CanvasEntry, CanvasError, CanvasFile, CanvasKey};
use home::{IssueId, ProjectId};

pub use contract::contract;
pub use memory::Memory;

/// Keeps canvas files by their keys, typed; how it encodes them is its own.
pub trait CanvasStore {
    /// Every canvas's key, in key order.
    ///
    /// # Errors
    ///
    /// The store can't be read.
    fn all(&self) -> Result<Vec<CanvasKey>, StoreError>;

    /// The canvases of an issue, by name.
    ///
    /// # Errors
    ///
    /// The store can't be read, or holds one that isn't a canvas.
    fn list(
        &self,
        project: &ProjectId,
        issue: &IssueId,
    ) -> Result<Vec<CanvasEntry>, StoreError>;

    /// The canvas kept under `key`, none when there is none.
    ///
    /// # Errors
    ///
    /// The store can't be read, or holds one that isn't a canvas.
    fn get(&self, key: &CanvasKey) -> Result<Option<CanvasFile>, StoreError>;

    /// Keeps `file` under `key`, in place of any canvas kept there.
    ///
    /// # Errors
    ///
    /// The store can't be written.
    fn put(
        &mut self,
        key: &CanvasKey,
        file: &CanvasFile,
    ) -> Result<(), StoreError>;
}

/// Why a store couldn't do what it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The store couldn't be read or written.
    Unavailable { reason: String },
    /// What the store holds under `key` isn't a canvas.
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
