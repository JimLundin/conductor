use std::collections::BTreeMap;

use home::canvas::{
    Broken, CanvasEntry, CanvasError, CanvasFile, CanvasKey, CanvasName, Held,
    Stray,
};
use home::{IssueId, ProjectId};

use crate::{CanvasStore, StoreError};

#[derive(Debug, Default)]
pub struct Memory {
    issues: BTreeMap<(ProjectId, IssueId), Folder>,
    strays: Vec<Stray>,
}

#[derive(Debug, Default)]
struct Folder {
    canvases: BTreeMap<CanvasName, Result<CanvasFile, CanvasError>>,
    strays: Vec<Stray>,
}

impl Memory {
    pub fn spoil(&mut self, key: CanvasKey, error: CanvasError) {
        self.folder(&key).canvases.insert(key.name, Err(error));
    }

    pub fn stray(
        &mut self,
        project: ProjectId,
        under: Option<IssueId>,
        stray: Stray,
    ) {
        match under {
            Some(issue) => self
                .issues
                .entry((project, issue))
                .or_default()
                .strays
                .push(stray),
            None => self.strays.push(stray),
        }
    }

    fn folder(&mut self, key: &CanvasKey) -> &mut Folder {
        self.issues.entry((key.project, key.issue)).or_default()
    }
}

impl Folder {
    fn listed(&self) -> Held<Result<CanvasEntry, Broken>> {
        Held {
            canvases: self
                .canvases
                .iter()
                .map(|(name, kept)| match kept {
                    Ok(file) => Ok(CanvasEntry::of(name.clone(), file)),
                    Err(error) => Err(Broken {
                        name: name.clone(),
                        error: error.clone(),
                    }),
                })
                .collect(),
            strays: self.strays.clone(),
        }
    }
}

impl CanvasStore for Memory {
    fn all(&self) -> Result<Held<CanvasKey>, StoreError> {
        Ok(Held {
            canvases: self
                .issues
                .iter()
                .flat_map(|(&(project, issue), folder)| {
                    folder.canvases.keys().map(move |name| CanvasKey {
                        project,
                        issue,
                        name: name.clone(),
                    })
                })
                .collect(),
            strays: self
                .issues
                .values()
                .flat_map(|folder| folder.strays.iter().cloned())
                .chain(self.strays.iter().cloned())
                .collect(),
        })
    }

    fn list(
        &self,
        project: &ProjectId,
        issue: &IssueId,
    ) -> Result<Held<Result<CanvasEntry, Broken>>, StoreError> {
        Ok(self
            .issues
            .get(&(*project, *issue))
            .map_or_else(Held::default, Folder::listed))
    }

    fn get(&self, key: &CanvasKey) -> Result<Option<CanvasFile>, StoreError> {
        self.issues
            .get(&(key.project, key.issue))
            .and_then(|folder| folder.canvases.get(&key.name))
            .cloned()
            .transpose()
            .map_err(|error| StoreError::Invalid {
                key: key.clone(),
                error,
            })
    }

    fn put(
        &mut self,
        key: &CanvasKey,
        file: &CanvasFile,
    ) -> Result<(), StoreError> {
        self.folder(key)
            .canvases
            .insert(key.name.clone(), Ok(file.clone()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use home::canvas::{CanvasError, Stray, StrayReason};

    use super::Memory;
    use crate::{Plant, contract};

    #[test]
    fn the_in_memory_store_keeps_the_store_contract() {
        assert_eq!(
            contract(
                &mut Memory::default(),
                |memory, key| memory.spoil(
                    key.clone(),
                    CanvasError::Json {
                        reason: String::from("expected value"),
                    },
                ),
                |memory, plant| match plant {
                    Plant::Unnamed { project, issue } => planted(
                        memory,
                        *project,
                        Some(*issue),
                        StrayReason::Unnamed,
                    ),
                    Plant::Folder { project, issue } => planted(
                        memory,
                        *project,
                        Some(*issue),
                        StrayReason::Folder,
                    ),
                    Plant::NotAnIssue(project) => {
                        planted(memory, *project, None, StrayReason::NotAnIssue)
                    }
                }
            ),
            Ok(())
        );
    }

    fn planted(
        memory: &mut Memory,
        project: home::ProjectId,
        issue: Option<home::IssueId>,
        reason: StrayReason,
    ) -> String {
        let place = format!("{project}/{issue:?}/{reason:?}");
        memory.stray(
            project,
            issue,
            Stray {
                place: place.clone(),
                reason,
            },
        );
        place
    }
}
