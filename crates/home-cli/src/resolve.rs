use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use github::configuration::selected;
use home::backend::{Backends, Capability};
use home::flow::command::{Resolved, named};
use home::flow::resolve::{Missing, Mode, Resolver};
use home::issue::storage::ReadHome;
use home::issue::{IssueListing, IssueReading, ReadFailure, ReadMode};
use home::project::Project;
use home::{IssueKey, ProjectId};
use home_files::FolderStore;
use home_store::Stores;
use project_runtime::Registry;
use project_runtime::issues::{IssueCache, cached};

pub struct Trackers<'a> {
    root: &'a Path,
    loaded: ReadHome,
    data: Option<PathBuf>,
    registry: Registry,
    stores: Stores,
}

impl<'a> Trackers<'a> {
    pub fn new(
        root: &'a Path,
        loaded: ReadHome,
        data: Option<PathBuf>,
    ) -> Self {
        Self {
            root,
            loaded,
            data,
            registry: Registry::default(),
            stores: Stores::new(Box::new(FolderStore::new(root.to_path_buf()))),
        }
    }

    pub const fn loaded(&self) -> &ReadHome {
        &self.loaded
    }

    pub const fn stores(&self) -> &Stores {
        &self.stores
    }

    pub const fn stores_mut(&mut self) -> &mut Stores {
        &mut self.stores
    }

    pub fn resolved(
        &mut self,
        keys: BTreeSet<IssueKey>,
        mode: Mode,
    ) -> Resolved {
        if keys.is_empty() {
            return Resolved::new();
        }
        let built = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        match built {
            Ok(runtime) => runtime.block_on(self.each(keys, mode)),
            Err(error) => keys
                .into_iter()
                .map(|key| (key, Err(error.to_string())))
                .collect(),
        }
    }

    async fn each(&mut self, keys: BTreeSet<IssueKey>, mode: Mode) -> Resolved {
        let mut steps = Resolver::new(mode);
        let mut found = Resolved::new();
        for key in keys {
            let reading = self.resolve(&mut steps, &key).await;
            found.insert(key, reading);
        }
        found
    }

    async fn resolve(
        &mut self,
        resolver: &mut Resolver,
        key: &IssueKey,
    ) -> Result<IssueReading, String> {
        loop {
            let looked = self.look(key).await?;
            let name = named(&self.loaded.snapshot, key.project);
            match resolver.step(&name, key, looked) {
                Ok(reading) => return Ok(reading),
                Err(Missing::Unknown(reason)) => return Err(reason),
                Err(Missing::Refresh(project)) => {
                    let outcome = self.refresh(project).await?;
                    resolver.refreshed(project, outcome);
                }
            }
        }
    }

    pub fn keeps(&self, project: ProjectId) -> bool {
        self.tracked(project).is_none()
    }

    fn tracked(&self, project: ProjectId) -> Option<&Project> {
        self.loaded.snapshot.projects().iter().find(|known| {
            known.id == project && known.backends.tracker.is_some()
        })
    }

    fn data(&self) -> Result<PathBuf, String> {
        project_runtime::data_dir_or_default(self.data.clone())
            .map_err(|error| error.to_string())
    }

    async fn look(&self, key: &IssueKey) -> Result<IssueReading, String> {
        cached(&self.data()?, &self.loaded.snapshot, key).await
    }

    async fn refresh(
        &mut self,
        project: ProjectId,
    ) -> Result<Result<(), ReadFailure>, String> {
        if self.keeps(project) {
            self.loaded = home_files::read(self.root);
            return Ok(Ok(()));
        }
        let listing = self.listing(project, false).await?;
        Ok(listing.status.failure.map_or(Ok(()), Err))
    }

    pub async fn listing(
        &mut self,
        project: ProjectId,
        offline: bool,
    ) -> Result<IssueListing, String> {
        let Some(backends) =
            self.tracked(project).map(|known| known.backends.clone())
        else {
            return Ok(self.loaded.snapshot.listed(project));
        };
        let data = self.data()?;
        let mode = if offline {
            ReadMode::Cached
        } else {
            self.register(project, &backends, &data)?
                .map_or_else(ReadMode::Failed, |()| ReadMode::Refresh)
        };
        let cache = IssueCache::new(&data, self.loaded.snapshot.flow()?);
        self.registry
            .project(&backends)
            .issues(&cache, project, mode)
            .await
            .map_err(|error| error.to_string())
    }

    fn register(
        &mut self,
        project: ProjectId,
        backends: &Backends,
        data: &Path,
    ) -> Result<Result<(), ReadFailure>, String> {
        if self
            .registry
            .preflight(backends, Capability::ReadIssues)
            .is_ok()
        {
            return Ok(Ok(()));
        }
        let name = named(&self.loaded.snapshot, project);
        Ok(selected(&self.loaded.snapshot, &name)?.tracker.map_or(
            Ok(()),
            |tracker| {
                super::tracking::register(&mut self.registry, data, tracker)
            },
        ))
    }
}

#[cfg(test)]
mod tests;
