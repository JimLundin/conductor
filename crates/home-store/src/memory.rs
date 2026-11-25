use std::collections::BTreeMap;

use home::canvas::{CanvasEntry, CanvasFile, CanvasKey};
use home::{IssueId, ProjectId};

use crate::{CanvasStore, StoreError};

/// A store that keeps canvases in memory for as long as it lives.
#[derive(Debug, Default)]
pub struct Memory(BTreeMap<CanvasKey, CanvasFile>);

impl CanvasStore for Memory {
    fn all(&self) -> Result<Vec<CanvasKey>, StoreError> {
        Ok(self.0.keys().cloned().collect())
    }

    fn list(
        &self,
        project: &ProjectId,
        issue: &IssueId,
    ) -> Result<Vec<CanvasEntry>, StoreError> {
        Ok(self
            .0
            .iter()
            .filter(|(key, _)| key.project == *project && key.issue == *issue)
            .map(|(key, file)| CanvasEntry::of(key.name.clone(), file))
            .collect())
    }

    fn get(&self, key: &CanvasKey) -> Result<Option<CanvasFile>, StoreError> {
        Ok(self.0.get(key).cloned())
    }

    fn put(
        &mut self,
        key: &CanvasKey,
        file: &CanvasFile,
    ) -> Result<(), StoreError> {
        self.0.insert(key.clone(), file.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_in_memory_store_keeps_the_store_contract() {
        assert_eq!(
            super::super::contract(&mut super::Memory::default()),
            Ok(())
        );
    }
}
