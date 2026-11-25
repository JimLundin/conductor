use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use home::canvas::{CanvasEntry, CanvasFile, CanvasKey, CanvasName};
use home::layout::{self, PROJECTS};
use home::{IssueId, ProjectId};
use home_store::{CanvasStore, StoreError};

const SUFFIX: &str = ".json";

/// The store that keeps canvases in the home's folder, each at
/// `projects/<project>/issues/<issue>/<name>.json`, so they are committed
/// and synced with the issues.
#[derive(Debug, Clone)]
pub struct FolderStore {
    root: PathBuf,
}

impl FolderStore {
    /// The store of the home whose root is `root`.
    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn issue(&self, project: ProjectId, issue: IssueId) -> PathBuf {
        self.root
            .join(layout::issues(&project))
            .join(issue.to_string())
    }

    fn path(&self, key: &CanvasKey) -> PathBuf {
        self.issue(key.project, key.issue)
            .join(format!("{}{SUFFIX}", key.name))
    }

    fn names(&self, project: ProjectId, issue: IssueId) -> Vec<CanvasName> {
        let mut names: Vec<CanvasName> =
            super::names(&self.issue(project, issue))
                .iter()
                .filter_map(|file| file.strip_suffix(SUFFIX))
                .filter_map(|stem| CanvasName::parse(stem).ok())
                .collect();
        names.sort();
        names
    }

    fn keys(&self, project: ProjectId) -> Vec<CanvasKey> {
        super::folders(&self.root.join(layout::issues(&project)))
            .iter()
            .filter_map(|folder| IssueId::parse(folder))
            .flat_map(|issue| {
                self.names(project, issue).into_iter().map(move |name| {
                    CanvasKey {
                        project,
                        issue,
                        name,
                    }
                })
            })
            .collect()
    }

    fn held(&self, key: &CanvasKey) -> Result<CanvasFile, StoreError> {
        self.get(key)?.ok_or_else(|| StoreError::Unavailable {
            reason: format!("{}: gone while read", self.path(key).display()),
        })
    }
}

impl CanvasStore for FolderStore {
    fn all(&self) -> Result<Vec<CanvasKey>, StoreError> {
        let mut keys: Vec<CanvasKey> =
            super::folders(&self.root.join(PROJECTS))
                .iter()
                .filter_map(|folder| ProjectId::parse(folder))
                .flat_map(|project| self.keys(project))
                .collect();
        keys.sort();
        Ok(keys)
    }

    fn list(
        &self,
        project: &ProjectId,
        issue: &IssueId,
    ) -> Result<Vec<CanvasEntry>, StoreError> {
        self.names(*project, *issue)
            .into_iter()
            .map(|name| {
                let key = CanvasKey {
                    project: *project,
                    issue: *issue,
                    name,
                };
                let file = self.held(&key)?;
                Ok(CanvasEntry::of(key.name, &file))
            })
            .collect()
    }

    fn get(&self, key: &CanvasKey) -> Result<Option<CanvasFile>, StoreError> {
        let path = self.path(key);
        match fs::read_to_string(&path) {
            Ok(text) => CanvasFile::parse(&text).map(Some).map_err(|error| {
                StoreError::Invalid {
                    key: key.clone(),
                    error,
                }
            }),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(unavailable(&path, &error)),
        }
    }

    fn put(
        &mut self,
        key: &CanvasKey,
        file: &CanvasFile,
    ) -> Result<(), StoreError> {
        let path = self.path(key);
        fs::create_dir_all(self.issue(key.project, key.issue))
            .and_then(|()| fs::write(&path, file.encode()))
            .map_err(|error| unavailable(&path, &error))
    }
}

fn unavailable(path: &Path, error: &io::Error) -> StoreError {
    StoreError::Unavailable {
        reason: format!("{}: {error}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use home::canvas::{Block, Canvas, CanvasFile, CanvasKey, CanvasName};
    use home::{IssueId, ProjectId};
    use home_store::{CanvasStore, StoreError};

    use super::FolderStore;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            Self(
                std::env::temp_dir()
                    .join(format!("home-files-{name}-{}", std::process::id())),
            )
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _removed = fs::remove_dir_all(&self.0);
        }
    }

    fn key(name: &str) -> Result<CanvasKey, Box<dyn std::error::Error>> {
        Ok(CanvasKey {
            project: ProjectId::parse("a1b2c3").ok_or("project")?,
            issue: IssueId::parse("0v29xv1q").ok_or("issue")?,
            name: CanvasName::parse(name)?,
        })
    }

    #[test]
    fn the_folder_store_keeps_the_store_contract() {
        let scratch = Scratch::new("contract");
        assert_eq!(
            home_store::contract(&mut FolderStore::new(scratch.0.clone())),
            Ok(())
        );
    }

    #[test]
    fn a_canvas_is_kept_as_json_in_its_issue_s_folder_by_name()
    -> Result<(), Box<dyn std::error::Error>> {
        let scratch = Scratch::new("canvas-path");
        let file = CanvasFile {
            written: 5,
            by: String::from("devbox·claude"),
            canvas: Canvas {
                title: String::from("t"),
                blocks: vec![Block::Prose {
                    text: String::from("x"),
                }],
            },
        };
        FolderStore::new(scratch.0.clone()).put(&key("design-tree")?, &file)?;
        let kept = fs::read_to_string(
            scratch
                .0
                .join("projects/a1b2c3/issues/0v29xv1q/design-tree.json"),
        )?;
        assert_eq!(CanvasFile::parse(&kept), Ok(file));
        Ok(())
    }

    #[test]
    fn a_kept_file_that_isnt_a_canvas_is_refused_as_invalid_under_its_key()
    -> Result<(), Box<dyn std::error::Error>> {
        let scratch = Scratch::new("canvas-invalid");
        let folder = scratch.0.join("projects/a1b2c3/issues/0v29xv1q");
        fs::create_dir_all(&folder)?;
        fs::write(folder.join("design.json"), "{}")?;
        let store = FolderStore::new(scratch.0.clone());
        assert!(matches!(
            store.get(&key("design")?),
            Err(StoreError::Invalid { key: at, .. }) if at == key("design")?
        ));
        assert_eq!(store.all()?, [key("design")?]);
        Ok(())
    }
}
