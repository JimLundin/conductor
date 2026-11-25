use std::collections::BTreeMap;

use home::canvas::{
    CanvasEntry, CanvasError, CanvasFile, CanvasKey, Held, Stray, StrayReason,
};
use home::{IssueId, ProjectId};

use crate::{CanvasStore, StoreError};

#[derive(Debug, Default)]
pub struct Memory {
    canvases: BTreeMap<CanvasKey, Result<CanvasFile, CanvasError>>,
    strays: Vec<(ProjectId, Option<IssueId>, Stray)>,
}

impl Memory {
    pub fn spoil(&mut self, key: CanvasKey, error: CanvasError) -> String {
        let place = place(&key);
        self.canvases.insert(key, Err(error));
        place
    }

    pub fn stray(
        &mut self,
        project: ProjectId,
        issue: Option<IssueId>,
        stray: Stray,
    ) {
        self.strays.push((project, issue, stray));
    }
}

fn place(key: &CanvasKey) -> String {
    format!("{}/{}/{}", key.project, key.issue, key.name)
}

impl CanvasStore for Memory {
    fn all(&self) -> Result<Held<CanvasKey>, StoreError> {
        Ok(Held {
            canvases: self.canvases.keys().cloned().collect(),
            strays: self
                .strays
                .iter()
                .map(|(_, _, stray)| stray.clone())
                .collect(),
        })
    }

    fn list(
        &self,
        project: &ProjectId,
        issue: &IssueId,
    ) -> Result<Held<CanvasEntry>, StoreError> {
        let mut held = Held::default();
        self.canvases
            .iter()
            .filter(|(key, _)| key.project == *project && key.issue == *issue)
            .for_each(|(key, kept)| match kept {
                Ok(file) => {
                    held.canvases.push(CanvasEntry::of(key.name.clone(), file));
                }
                Err(error) => held.strays.push(Stray {
                    place: place(key),
                    reason: StrayReason::Invalid(error.clone()),
                }),
            });
        held.strays.extend(
            self.strays
                .iter()
                .filter(|(at, under, _)| {
                    at == project && *under == Some(*issue)
                })
                .map(|(_, _, stray)| stray.clone()),
        );
        Ok(held)
    }

    fn get(&self, key: &CanvasKey) -> Result<Option<CanvasFile>, StoreError> {
        self.canvases
            .get(key)
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
        self.canvases.insert(key.clone(), Ok(file.clone()));
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
            contract(&mut Memory::default(), |memory, plant| match plant {
                Plant::Broken(key) => memory.spoil(
                    key.clone(),
                    CanvasError::Json {
                        reason: String::from("expected value"),
                    },
                ),
                Plant::Unnamed { project, issue } => {
                    planted(
                        memory,
                        *project,
                        Some(*issue),
                        StrayReason::Unnamed,
                    )
                }
                Plant::NotAnIssue(project) => {
                    planted(memory, *project, None, StrayReason::NotAnIssue)
                }
            }),
            Ok(())
        );
    }

    fn planted(
        memory: &mut Memory,
        project: home::ProjectId,
        issue: Option<home::IssueId>,
        reason: StrayReason,
    ) -> String {
        let place = format!("{project}/{issue:?}/stray");
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
