use std::collections::BTreeMap;

use home::canvas::{
    CanvasEntry, CanvasError, CanvasFile, CanvasKey, CanvasName, Held, Stray,
    StrayReason,
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
    pub fn spoil(&mut self, key: CanvasKey, error: CanvasError) -> String {
        let place = place(&key);
        self.folder(&key).canvases.insert(key.name, Err(error));
        place
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

fn place(key: &CanvasKey) -> String {
    format!("{}/{}/{}", key.project, key.issue, key.name)
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
    ) -> Result<Held<CanvasEntry>, StoreError> {
        let mut held = Held::default();
        let Some(folder) = self.issues.get(&(*project, *issue)) else {
            return Ok(held);
        };
        for (name, kept) in &folder.canvases {
            match kept {
                Ok(file) => {
                    held.canvases.push(CanvasEntry::of(name.clone(), file));
                }
                Err(error) => held.strays.push(Stray {
                    place: place(&CanvasKey {
                        project: *project,
                        issue: *issue,
                        name: name.clone(),
                    }),
                    reason: StrayReason::Invalid(error.clone()),
                }),
            }
        }
        held.strays.extend(folder.strays.iter().cloned());
        Ok(held)
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
