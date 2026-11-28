use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use home::backend::{Backends, Capability};
use home::canvas::{Broken, CanvasEntry, CanvasFile, CanvasKey, Held};
use home::flow::command::Change;
use home::flow::snapshot::Snapshot;
use home::issue::storage::ReadHome;
use home::issue::{IssueListing, IssueReading, Lookup, ReadFailure, ReadMode};
use home::project::Project;
use home::tracker::{self, Tracker};
use home::{IssueId, IssueKey, ProjectId};
use home_store::{CanvasStore, Kept, Memory, Putting, StoreError};

use crate::issues::IssueCache;
use crate::operations::Operations;
use crate::{IssueEdit, IssueTracker, Registration, Registry};

pub struct Reader {
    pub registration: Registration,
    pub tracker: Arc<dyn IssueTracker>,
}

pub trait Connector: Send + Sync {
    fn connect(&self) -> Result<Reader, ReadFailure>;
}

pub type Refreshing<'a> =
    Pin<Box<dyn Future<Output = Result<(), ReadFailure>> + Send + 'a>>;

pub trait TrackerStore: CanvasStore + Send + Sync {
    fn refresh<'a>(&'a self, listing: &'a IssueListing) -> Refreshing<'a>;
}

impl TrackerStore for Memory {
    fn refresh<'a>(&'a self, _: &'a IssueListing) -> Refreshing<'a> {
        Box::pin(async { Ok(()) })
    }
}

pub struct Connection {
    pub canvases: Box<dyn TrackerStore>,
    pub connector: Arc<dyn Connector>,
}

pub type Connect = fn(&Path, &Snapshot, &Project) -> Result<Connection, String>;

struct Configured {
    backends: Backends,
    canvases: Box<dyn TrackerStore>,
    connector: Arc<dyn Connector>,
}

enum Selected {
    Markdown,
    Configured(Box<Configured>),
    Unbuilt(String),
}

pub struct Trackers {
    data: PathBuf,
    markdown: Kept,
    connect: Connect,
    selected: BTreeMap<ProjectId, Selected>,
}

impl Trackers {
    #[must_use]
    pub const fn new(data: PathBuf, markdown: Kept, connect: Connect) -> Self {
        Self {
            data,
            markdown,
            connect,
            selected: BTreeMap::new(),
        }
    }

    pub fn follow(&mut self, home: &Snapshot) {
        self.selected = home
            .projects()
            .iter()
            .map(|project| (project.id, self.select(home, project)))
            .collect();
    }

    fn select(&self, home: &Snapshot, project: &Project) -> Selected {
        match tracker::of(project) {
            Tracker::Markdown => Selected::Markdown,
            Tracker::Configured(tracked) => {
                match (self.connect)(&self.data, home, tracked) {
                    Ok(connection) => {
                        Selected::Configured(Box::new(Configured {
                            backends: tracked.backends.clone(),
                            canvases: connection.canvases,
                            connector: connection.connector,
                        }))
                    }
                    Err(error) => Selected::Unbuilt(format!(
                        "the tracker of {}: {error}",
                        tracked.name
                    )),
                }
            }
        }
    }

    fn selected(&self, project: ProjectId) -> Result<&Selected, String> {
        self.selected
            .get(&project)
            .ok_or_else(|| format!("no project {project} in the home"))
    }

    pub async fn look(
        &self,
        home: &Snapshot,
        key: &IssueKey,
    ) -> Result<IssueReading, String> {
        match self.selected(key.project)? {
            Selected::Markdown => Ok(home.reading(key)),
            Selected::Configured(configured) => {
                configured.look(&self.data, home, key, false).await
            }
            Selected::Unbuilt(reason) => Err(reason.clone()),
        }
    }

    pub async fn read(
        &self,
        home: &Snapshot,
        key: &IssueKey,
    ) -> Result<IssueReading, String> {
        match self.selected(key.project)? {
            Selected::Markdown => Ok(home.reading(key)),
            Selected::Configured(configured) => {
                configured.look(&self.data, home, key, true).await
            }
            Selected::Unbuilt(reason) => Err(reason.clone()),
        }
    }

    pub async fn listing(
        &self,
        home: &Snapshot,
        project: ProjectId,
        offline: bool,
    ) -> Result<IssueListing, String> {
        match self.selected(project)? {
            Selected::Markdown => Ok(home.listed(project)),
            Selected::Configured(configured) => {
                configured.listing(&self.data, home, project, offline).await
            }
            Selected::Unbuilt(reason) => Err(reason.clone()),
        }
    }

    pub async fn refresh(
        &self,
        home: &Snapshot,
        project: ProjectId,
    ) -> Result<Result<(), ReadFailure>, String> {
        let listing = self.listing(home, project, false).await?;
        Ok(listing.status.failure.map_or(Ok(()), Err))
    }

    pub async fn write(
        &self,
        home: &ReadHome,
        root: &Path,
        change: &Change,
    ) -> Result<Option<String>, String> {
        match self.selected(change.observation.project())? {
            Selected::Markdown => {
                let file = home.encode(change)?;
                let path = root.join(file.path());
                std::fs::create_dir_all(root.join(file.folder()))
                    .and_then(|()| std::fs::write(&path, &file.text))
                    .map_err(|err| format!("{}: {err}", path.display()))?;
                Ok(Some(file.path()))
            }
            Selected::Configured(configured) => configured
                .write(&self.data, &home.snapshot, change)
                .await
                .map(|()| None),
            Selected::Unbuilt(reason) => Err(reason.clone()),
        }
    }

    pub fn register(
        &self,
        project: ProjectId,
        registry: &mut Registry,
    ) -> Result<Result<(), ReadFailure>, String> {
        match self.selected(project)? {
            Selected::Markdown => Ok(Ok(())),
            Selected::Configured(configured) => {
                Ok(configured.register(registry))
            }
            Selected::Unbuilt(reason) => Err(reason.clone()),
        }
    }

    fn store(
        &self,
        project: ProjectId,
    ) -> Result<&dyn CanvasStore, StoreError> {
        match self.selected(project).map_err(unavailable)? {
            Selected::Markdown => Ok(&*self.markdown),
            Selected::Configured(configured) => Ok(&*configured.canvases),
            Selected::Unbuilt(reason) => Err(unavailable(reason.as_str())),
        }
    }

    fn store_mut(
        &mut self,
        project: ProjectId,
    ) -> Result<&mut dyn CanvasStore, StoreError> {
        match self.selected.get_mut(&project) {
            Some(Selected::Markdown) => Ok(&mut *self.markdown),
            Some(Selected::Configured(configured)) => {
                Ok(&mut *configured.canvases)
            }
            Some(Selected::Unbuilt(reason)) => {
                Err(unavailable(reason.as_str()))
            }
            None => {
                Err(unavailable(format!("no project {project} in the home")))
            }
        }
    }

    fn markdown(&self, project: ProjectId) -> bool {
        matches!(self.selected.get(&project), Some(Selected::Markdown))
    }
}

impl Configured {
    fn register(&self, registry: &mut Registry) -> Result<(), ReadFailure> {
        if registry
            .preflight(&self.backends, Capability::ReadIssues)
            .is_ok()
        {
            return Ok(());
        }
        let reader = self.connector.connect()?;
        registry
            .register_tracker(reader.registration, reader.tracker)
            .map_err(ReadFailure::Refused)
    }

    async fn write(
        &self,
        data: &Path,
        home: &Snapshot,
        change: &Change,
    ) -> Result<(), String> {
        if change.created {
            return Err(String::from(
                "home doesn't open issues in a project's own tracker yet",
            ));
        }
        let seen = change.observation.revision.clone().ok_or_else(|| {
            String::from("the issue was read without a revision to compare")
        })?;
        let mut registry = Registry::default();
        self.register(&mut registry)
            .map_err(|failure| failure.to_string())?;
        let operations =
            Operations::open(data).map_err(|error| error.to_string())?;
        let handle = operations.handle().map_err(|error| error.to_string())?;
        let issue = Lookup {
            project: change.observation.project(),
            id: change.observation.id().clone(),
        };
        let project = registry.project(&self.backends);
        project
            .edit_issue(
                &operations,
                handle,
                IssueEdit {
                    issue: issue.clone(),
                    seen,
                    mutations: change.mutations.clone(),
                },
            )
            .await
            .map_err(|error| error.to_string())?;
        let cache = IssueCache::new(data, home.flow()?);
        project
            .issue(&cache, issue, ReadMode::Refresh)
            .await
            .map(drop)
            .map_err(|error| error.to_string())
    }

    fn mode(&self, registry: &mut Registry, fresh: bool) -> ReadMode {
        if fresh {
            self.register(registry)
                .map_or_else(ReadMode::Failed, |()| ReadMode::Refresh)
        } else {
            ReadMode::Cached
        }
    }

    async fn look(
        &self,
        data: &Path,
        home: &Snapshot,
        key: &IssueKey,
        fresh: bool,
    ) -> Result<IssueReading, String> {
        let mut registry = Registry::default();
        let mode = self.mode(&mut registry, fresh);
        let cache = IssueCache::new(data, home.flow()?);
        let lookup = Lookup {
            project: key.project,
            id: key.id.clone(),
        };
        registry
            .project(&self.backends)
            .issue(&cache, lookup, mode)
            .await
            .map_err(|error| error.to_string())
    }

    async fn listing(
        &self,
        data: &Path,
        home: &Snapshot,
        project: ProjectId,
        offline: bool,
    ) -> Result<IssueListing, String> {
        let mut registry = Registry::default();
        let mode = self.mode(&mut registry, !offline);
        let refreshing = matches!(mode, ReadMode::Refresh);
        let cache = IssueCache::new(data, home.flow()?);
        let listing = registry
            .project(&self.backends)
            .issues(&cache, project, mode)
            .await
            .map_err(|error| error.to_string())?;
        Ok(self.refreshed(listing, refreshing).await)
    }

    async fn refreshed(
        &self,
        mut listing: IssueListing,
        refreshing: bool,
    ) -> IssueListing {
        if refreshing && listing.status.failure.is_none() {
            listing.status.failure =
                self.canvases.refresh(&listing).await.err();
        }
        listing
    }
}

fn unavailable(reason: impl Into<String>) -> StoreError {
    StoreError::Unavailable {
        reason: reason.into(),
    }
}

impl CanvasStore for Trackers {
    fn all(&self) -> Result<Held<CanvasKey>, StoreError> {
        let mut held = self.markdown.all()?;
        held.canvases.retain(|key| self.markdown(key.project));
        for selected in self.selected.values() {
            let kept = match selected {
                Selected::Markdown => continue,
                Selected::Configured(configured) => {
                    configured.canvases.all()?
                }
                Selected::Unbuilt(reason) => {
                    return Err(unavailable(reason.as_str()));
                }
            };
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
        self.store(*project)?.list(project, issue)
    }

    fn get(&self, key: &CanvasKey) -> Result<Option<CanvasFile>, StoreError> {
        self.store(key.project)?.get(key)
    }

    fn put<'a>(
        &'a mut self,
        key: &'a CanvasKey,
        file: &'a CanvasFile,
    ) -> Putting<'a> {
        match self.store_mut(key.project) {
            Ok(store) => store.put(key, file),
            Err(refused) => Box::pin(async move { Err(refused) }),
        }
    }
}

#[cfg(test)]
mod tests;
