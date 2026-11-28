use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use github::configuration::selected;
use home::flow::command::Resolved;
use home::flow::resolve::{Missing, Mode, Resolver};
use home::issue::storage::ReadHome;
use home::issue::{IssueReading, Lookup, ReadFailure, ReadMode};
use home::project::Project;
use home::{IssueKey, ProjectId};
use project_runtime::Registry;
use project_runtime::issues::IssueCache;

pub struct Trackers<'a> {
    root: &'a Path,
    loaded: ReadHome,
    data: Option<PathBuf>,
    registry: Registry,
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
        }
    }

    pub const fn loaded(&self) -> &ReadHome {
        &self.loaded
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
            match resolver.step(&self.loaded.snapshot, key, looked) {
                Ok(reading) => return Ok(reading),
                Err(Missing::Unknown(reason)) => return Err(reason),
                Err(Missing::Refresh(project)) => {
                    let outcome = self.refresh(project).await?;
                    resolver.refreshed(project, outcome);
                }
            }
        }
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
        let Some(project) = self.tracked(key.project) else {
            return Ok(self.loaded.snapshot.reading(key));
        };
        let data = self.data()?;
        let cache = IssueCache::new(&data, self.loaded.snapshot.flow()?);
        let lookup = Lookup {
            project: key.project,
            id: key.id.clone(),
        };
        self.registry
            .project(&project.backends)
            .issue(&cache, lookup, ReadMode::Cached)
            .await
            .map_err(|error| error.to_string())
    }

    async fn refresh(
        &mut self,
        project: ProjectId,
    ) -> Result<Result<(), ReadFailure>, String> {
        let Some(name) = self.tracked(project).map(|known| known.name.clone())
        else {
            self.loaded = home_files::read(self.root);
            return Ok(Ok(()));
        };
        let data = self.data()?;
        let registered = selected(&self.loaded.snapshot, &name)?
            .tracker
            .map_or(Ok(()), |tracker| {
                super::tracking::register(&mut self.registry, &data, tracker)
            });
        if let Err(failure) = registered {
            return Ok(Err(failure));
        }
        let flow = self.loaded.snapshot.flow()?;
        let cache = IssueCache::new(&data, flow);
        let backends = self
            .tracked(project)
            .map(|known| &known.backends)
            .ok_or_else(|| format!("no project {project} keeps a tracker"))?;
        let listing = self
            .registry
            .project(backends)
            .issues(&cache, project, ReadMode::Refresh)
            .await
            .map_err(|error| error.to_string())?;
        Ok(listing.status.failure.map_or(Ok(()), Err))
    }
}
