use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use home::canvas::{
    Broken, CanvasEntry, CanvasFile, CanvasKey, CanvasName, Held, Stray,
    StrayReason,
};
use home::layout::{self, PROJECTS};
use home::{IssueId, ProjectId};
use home_store::{CanvasStore, Putting, StoreError, Wrote};

const SUFFIX: &str = ".json";

#[derive(Debug, Clone)]
pub struct FolderStore {
    root: PathBuf,
}

struct Found {
    name: String,
    folder: bool,
}

impl FolderStore {
    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn issue(project: ProjectId, issue: &IssueId) -> String {
        format!("{}/{}", layout::issues(&project), segment(issue))
    }

    fn place(key: &CanvasKey) -> String {
        format!(
            "{}/{}{SUFFIX}",
            Self::issue(key.project, &key.issue),
            key.name
        )
    }

    fn path(&self, key: &CanvasKey) -> PathBuf {
        self.root.join(Self::place(key))
    }

    fn found(&self, folder: &str) -> Result<Vec<Found>, StoreError> {
        let path = self.root.join(folder);
        let entries = match fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok(Vec::new());
            }
            Err(error) => return Err(unavailable(&path, &error)),
        };
        let mut found = entries
            .map(|read| {
                let entry = read.map_err(|error| unavailable(&path, &error))?;
                Ok(Found {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    folder: entry.path().is_dir(),
                })
            })
            .collect::<Result<Vec<Found>, StoreError>>()?;
        found.sort_by(|one, other| one.name.cmp(&other.name));
        Ok(found)
    }

    fn named(
        &self,
        project: ProjectId,
        issue: &IssueId,
    ) -> Result<Held<CanvasName>, StoreError> {
        let folder = Self::issue(project, issue);
        let mut held = Held::default();
        for found in self.found(&folder)? {
            let named = found
                .name
                .strip_suffix(SUFFIX)
                .and_then(|stem| CanvasName::parse(stem).ok());
            match (found.folder, named) {
                (false, Some(name)) => held.canvases.push(name),
                (true, _) => held.strays.push(Stray {
                    place: format!("{folder}/{}", found.name),
                    reason: StrayReason::Folder,
                }),
                (false, None) => held.strays.push(Stray {
                    place: format!("{folder}/{}", found.name),
                    reason: StrayReason::Unnamed,
                }),
            }
        }
        held.canvases.sort();
        Ok(held)
    }

    fn keys(&self, project: ProjectId) -> Result<Held<CanvasKey>, StoreError> {
        let folder = layout::issues(&project);
        let mut held = Held::default();
        for found in self.found(&folder)?.into_iter().filter(|f| f.folder) {
            let Some(issue) =
                unsegment(&found.name).filter(|id| segment(id) == found.name)
            else {
                held.strays.push(Stray {
                    place: format!("{folder}/{}", found.name),
                    reason: StrayReason::NotAnIssue,
                });
                continue;
            };
            let named = self.named(project, &issue)?;
            held.canvases.extend(named.canvases.into_iter().map(|name| {
                CanvasKey {
                    project,
                    issue: issue.clone(),
                    name,
                }
            }));
            held.strays.extend(named.strays);
        }
        Ok(held)
    }
}

fn segment(issue: &IssueId) -> String {
    issue
        .as_str()
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
                char::from(byte).into()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn unsegment(text: &str) -> Option<IssueId> {
    let mut bytes = text.bytes();
    let mut decoded = Vec::new();
    while let Some(byte) = bytes.next() {
        decoded.push(if byte == b'%' {
            let high = char::from(bytes.next()?).to_digit(16)?;
            let low = char::from(bytes.next()?).to_digit(16)?;
            u8::try_from(high * 16 + low).ok()?
        } else {
            byte
        });
    }
    IssueId::new(&String::from_utf8(decoded).ok()?)
}

impl CanvasStore for FolderStore {
    fn all(&self) -> Result<Held<CanvasKey>, StoreError> {
        let mut held = Held::default();
        for project in self
            .found(PROJECTS)?
            .iter()
            .filter(|found| found.folder)
            .filter_map(|found| ProjectId::parse(&found.name))
        {
            let kept = self.keys(project)?;
            held.canvases.extend(kept.canvases);
            held.strays.extend(kept.strays);
        }
        held.canvases.sort();
        Ok(held)
    }

    fn list(
        &self,
        project: &ProjectId,
        issue: &IssueId,
    ) -> Result<Held<Result<CanvasEntry, Broken>>, StoreError> {
        let named = self.named(*project, issue)?;
        let mut held = Held {
            canvases: Vec::new(),
            strays: named.strays,
        };
        for name in named.canvases {
            let key = CanvasKey {
                project: *project,
                issue: issue.clone(),
                name,
            };
            match self.get(&key) {
                Ok(Some(file)) => {
                    held.canvases.push(Ok(CanvasEntry::of(key.name, &file)));
                }
                Ok(None) => {
                    return Err(StoreError::Unavailable {
                        reason: format!(
                            "{}: gone while read",
                            self.path(&key).display()
                        ),
                    });
                }
                Err(StoreError::Invalid { error, .. }) => {
                    held.canvases.push(Err(Broken {
                        name: key.name,
                        error: *error,
                    }));
                }
                Err(error) => return Err(error),
            }
        }
        Ok(held)
    }

    fn get(&self, key: &CanvasKey) -> Result<Option<CanvasFile>, StoreError> {
        let path = self.path(key);
        match fs::read_to_string(&path) {
            Ok(text) => CanvasFile::parse(&text).map(Some).map_err(|error| {
                StoreError::Invalid {
                    key: key.clone(),
                    error: Box::new(error),
                }
            }),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(unavailable(&path, &error)),
        }
    }

    fn put<'a>(
        &'a mut self,
        key: &'a CanvasKey,
        file: &'a CanvasFile,
    ) -> Putting<'a> {
        Box::pin(async move {
            let path = self.path(key);
            fs::create_dir_all(
                self.root.join(Self::issue(key.project, &key.issue)),
            )
            .and_then(|()| fs::write(&path, file.encode()))
            .map(|()| Wrote {
                home_file: Some(Self::place(key)),
            })
            .map_err(|error| unavailable(&path, &error))
        })
    }

    fn touches(&self, key: &CanvasKey) -> Vec<String> {
        Vec::from([Self::place(key)])
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
    use home::layout;
    use home::{IssueId, ProjectId, UnixMillis};
    use home_store::{CanvasStore, Plant, StoreError};

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
            issue: IssueId::new("0v29xv1q").ok_or("issue")?,
            name: CanvasName::parse(name)?,
        })
    }

    #[tokio::test]
    async fn the_folder_store_keeps_the_store_contract() {
        let scratch = Scratch::new("contract");
        let root = scratch.0.clone();
        assert_eq!(
            home_store::contract(
                &mut FolderStore::new(scratch.0.clone()),
                async |_, key| spoiled(&root, key),
                async |_, plant| Some(planted(&root, plant))
            )
            .await,
            Ok(())
        );
    }

    fn spoiled(root: &std::path::Path, key: &CanvasKey) {
        let folder = root
            .join(layout::issues(&key.project))
            .join(super::segment(&key.issue));
        let made = fs::create_dir_all(&folder).and_then(|()| {
            fs::write(folder.join(format!("{}.json", key.name)), "{}")
        });
        assert!(made.is_ok(), "spoiling {key:?}: {made:?}");
    }

    fn planted(root: &std::path::Path, plant: &Plant) -> String {
        let (folder, file) = match plant {
            Plant::Folder { project, issue } => (
                format!(
                    "{}/{}/notes.json",
                    layout::issues(project),
                    super::segment(issue)
                ),
                None,
            ),
            Plant::Unnamed { project, issue } => (
                format!(
                    "{}/{}",
                    layout::issues(project),
                    super::segment(issue)
                ),
                Some(String::from("Bad Name.json")),
            ),
            Plant::NotAnIssue(project) => {
                (format!("{}/not an issue", layout::issues(project)), None)
            }
        };
        let made = fs::create_dir_all(root.join(&folder)).and_then(|()| {
            file.as_ref().map_or(Ok(()), |named| {
                fs::write(root.join(&folder).join(named), "{}")
            })
        });
        assert!(made.is_ok(), "planting {plant:?} in {folder}: {made:?}");
        file.map_or_else(|| folder.clone(), |named| format!("{folder}/{named}"))
    }

    #[tokio::test]
    async fn a_canvas_is_kept_as_json_in_its_issue_s_folder_by_name_which_put_reports()
    -> Result<(), Box<dyn std::error::Error>> {
        let scratch = Scratch::new("canvas-path");
        let file = CanvasFile {
            written: UnixMillis(5),
            by: String::from("devbox·claude"),
            canvas: Canvas::new(
                String::from("t"),
                vec![Block::Prose {
                    text: String::from("x"),
                }],
            )?,
        };
        let wrote = FolderStore::new(scratch.0.clone())
            .put(&key("design-tree")?, &file)
            .await?;
        assert_eq!(
            wrote.home_file.as_deref(),
            Some("projects/a1b2c3/issues/0v29xv1q/design-tree.json")
        );
        let kept = fs::read_to_string(
            scratch
                .0
                .join("projects/a1b2c3/issues/0v29xv1q/design-tree.json"),
        )?;
        assert_eq!(CanvasFile::parse(&kept), Ok(file));
        Ok(())
    }

    #[test]
    fn a_canvas_put_touches_the_home_file_it_will_write_before_writing_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let scratch = Scratch::new("canvas-touches");
        let store = FolderStore::new(scratch.0.clone());
        assert_eq!(
            store.touches(&key("design-tree")?),
            Vec::from([String::from(
                "projects/a1b2c3/issues/0v29xv1q/design-tree.json"
            )])
        );
        assert!(!scratch.0.exists());
        Ok(())
    }

    #[tokio::test]
    async fn an_issue_id_with_path_characters_is_one_canvas_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let scratch = Scratch::new("qualified-canvas");
        let mut store = FolderStore::new(scratch.0.clone());
        let mut key = key("plan")?;
        key.issue = IssueId::new("../42").ok_or("issue")?;
        let file = CanvasFile {
            written: UnixMillis(5),
            by: String::from("author"),
            canvas: Canvas::new(
                String::from("Plan"),
                vec![Block::Prose {
                    text: String::from("text"),
                }],
            )?,
        };
        store.put(&key, &file).await?;
        assert!(
            scratch
                .0
                .join("projects/a1b2c3/issues/%2E%2E%2F42/plan.json")
                .is_file()
        );
        assert_eq!(store.get(&key)?, Some(file));
        assert_eq!(store.all()?.canvases, [key]);
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
        assert_eq!(store.all()?.canvases, [key("design")?]);
        Ok(())
    }

    #[test]
    fn a_folder_the_store_cant_read_is_unavailable_not_empty()
    -> Result<(), Box<dyn std::error::Error>> {
        let scratch = Scratch::new("canvas-unreadable");
        let issues = scratch.0.join("projects/a1b2c3/issues");
        fs::create_dir_all(issues.parent().ok_or("parent")?)?;
        fs::write(&issues, "not a folder")?;
        let store = FolderStore::new(scratch.0.clone());
        let key = key("design")?;
        assert!(matches!(
            store.list(&key.project, &key.issue),
            Err(StoreError::Unavailable { .. })
        ));
        assert!(matches!(store.all(), Err(StoreError::Unavailable { .. })));
        Ok(())
    }
}
