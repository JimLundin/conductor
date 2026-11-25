use std::collections::BTreeMap;
use std::sync::MutexGuard;

use home::canvas::{CanvasEntry, CanvasFile, CanvasKey};
use home::{IssueId, ProjectId};

use crate::{CanvasStore, StoreError};

/// A store that keeps canvases in memory for as long as it lives.
#[derive(Debug, Default)]
#[expect(
    clippy::disallowed_types,
    reason = "the store contract puts through a shared reference, so a store in memory holds its canvases behind a lock"
)]
pub struct Memory(std::sync::Mutex<BTreeMap<CanvasKey, CanvasFile>>);

impl Memory {
    fn held(
        &self,
    ) -> Result<MutexGuard<'_, BTreeMap<CanvasKey, CanvasFile>>, StoreError>
    {
        self.0.lock().map_err(|poisoned| StoreError::Unavailable {
            reason: poisoned.to_string(),
        })
    }
}

impl CanvasStore for Memory {
    fn all(&self) -> Result<Vec<CanvasKey>, StoreError> {
        Ok(self.held()?.keys().cloned().collect())
    }

    fn list(
        &self,
        project: &ProjectId,
        issue: &IssueId,
    ) -> Result<Vec<CanvasEntry>, StoreError> {
        Ok(self
            .held()?
            .iter()
            .filter(|(key, _)| key.project == *project && key.issue == *issue)
            .map(|(key, file)| CanvasEntry::of(key.name.clone(), file))
            .collect())
    }

    fn get(&self, key: &CanvasKey) -> Result<Option<CanvasFile>, StoreError> {
        Ok(self.held()?.get(key).cloned())
    }

    fn put(
        &self,
        key: &CanvasKey,
        file: &CanvasFile,
    ) -> Result<(), StoreError> {
        self.held()?.insert(key.clone(), file.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_in_memory_store_keeps_the_store_contract() {
        assert_eq!(super::super::contract(&super::Memory::default()), Ok(()));
    }
}
