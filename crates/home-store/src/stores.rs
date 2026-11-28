use std::collections::BTreeMap;
use std::path::Path;

use home::canvas::{Broken, CanvasEntry, CanvasFile, CanvasKey, Held};
use home::flow::snapshot::Snapshot;
use home::project::Project;
use home::{IssueId, ProjectId};

use crate::{CanvasStore, Putting, StoreError};

pub type Kept = Box<dyn CanvasStore + Send + Sync>;

pub type Keeper = fn(&Path, &Snapshot, &Project) -> Option<Kept>;

pub struct Stores {
    home: Kept,
    trackers: BTreeMap<ProjectId, Kept>,
}

impl Stores {
    #[must_use]
    pub fn new(home: Kept) -> Self {
        Self {
            home,
            trackers: BTreeMap::new(),
        }
    }

    pub fn track(&mut self, project: ProjectId, store: Kept) {
        self.trackers.insert(project, store);
    }

    pub fn follow(&mut self, data: &Path, home: &Snapshot, keeper: Keeper) {
        self.trackers = home
            .projects()
            .iter()
            .filter(|project| project.backends.tracker.is_some())
            .filter_map(|project| {
                keeper(data, home, project).map(|store| (project.id, store))
            })
            .collect();
    }

    fn of(&self, project: ProjectId) -> &Kept {
        self.trackers.get(&project).unwrap_or(&self.home)
    }

    fn of_mut(&mut self, project: ProjectId) -> &mut Kept {
        self.trackers.get_mut(&project).unwrap_or(&mut self.home)
    }
}

impl CanvasStore for Stores {
    fn all(&self) -> Result<Held<CanvasKey>, StoreError> {
        let mut held = self.home.all()?;
        for store in self.trackers.values() {
            let kept = store.all()?;
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
        self.of(*project).list(project, issue)
    }

    fn get(&self, key: &CanvasKey) -> Result<Option<CanvasFile>, StoreError> {
        self.of(key.project).get(key)
    }

    fn put<'a>(
        &'a mut self,
        key: &'a CanvasKey,
        file: &'a CanvasFile,
    ) -> Putting<'a> {
        self.of_mut(key.project).put(key, file)
    }
}

#[cfg(test)]
mod tests {
    use home::canvas::{
        Block, Canvas, CanvasFile, CanvasKey, CanvasName, Stray, StrayReason,
    };
    use home::{IssueId, ProjectId, UnixMillis};

    use super::Stores;
    use crate::{CanvasStore, Memory};

    const TRACKED: ProjectId = ProjectId::new(2);

    fn file() -> Result<CanvasFile, String> {
        Ok(CanvasFile {
            written: UnixMillis(5),
            by: String::from("devbox·claude"),
            canvas: Canvas::new(
                String::from("Kept by its tracker"),
                Vec::from([Block::Prose {
                    text: String::from("The tracker keeps it."),
                }]),
            )
            .map_err(|error| error.to_string())?,
        })
    }

    fn key(project: ProjectId, issue: &str) -> Result<CanvasKey, String> {
        Ok(CanvasKey {
            project,
            issue: IssueId::new(issue).ok_or("issue")?,
            name: CanvasName::parse("design")
                .map_err(|error| error.to_string())?,
        })
    }

    fn stray(place: &str) -> Stray {
        Stray {
            place: String::from(place),
            reason: StrayReason::Unnamed,
        }
    }

    #[tokio::test]
    async fn a_tracked_project_s_canvases_are_kept_by_its_tracker_s_store()
    -> Result<(), String> {
        let mut tracker = Memory::default();
        let held = key(TRACKED, "146")?;
        tracker
            .put(&held, &file()?)
            .await
            .map_err(|error| error.to_string())?;
        let mut stores = Stores::new(Box::new(Memory::default()));
        stores.track(TRACKED, Box::new(tracker));
        let untracked = key(ProjectId::new(1), "0tm7aaaa")?;
        stores
            .put(&untracked, &file()?)
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(
            (
                stores.get(&held).map_err(|error| error.to_string())?,
                stores
                    .list(&TRACKED, &held.issue)
                    .map_err(|error| error.to_string())?
                    .canvases
                    .len(),
                stores.all().map_err(|error| error.to_string())?.canvases,
            ),
            (Some(file()?), 1, Vec::from([untracked, held]))
        );
        Ok(())
    }

    #[test]
    fn all_reports_the_strays_of_every_tracker_s_store() {
        let mut home = Memory::default();
        home.stray(ProjectId::new(1), None, stray("projects/7w85je/odd"));
        let mut tracker = Memory::default();
        tracker.stray(TRACKED, None, stray("issues/146#issuecomment-1"));
        let mut stores = Stores::new(Box::new(home));
        stores.track(TRACKED, Box::new(tracker));
        assert_eq!(
            stores.all().map(|held| held.strays),
            Ok(Vec::from([
                stray("projects/7w85je/odd"),
                stray("issues/146#issuecomment-1"),
            ]))
        );
    }
}
