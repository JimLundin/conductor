use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};

use home::flow::command::{CanvasWrite, Change};
use home::flow::command::{Listed, Resolved, named};
use home::flow::resolve::{Missing, Mode, Resolver};
use home::flow::snapshot::Snapshot;
use home::issue::storage::ReadHome;
use home::issue::{Freshness, IssueListing, IssueReading, ReadStatus};
use home::{IssueKey, ProjectId};
use home_files::FolderStore;
use home_store::CanvasStore;
use project_runtime::tracker::{self, Connect, Written};

pub struct Trackers {
    loaded: ReadHome,
    trackers: tracker::Trackers,
}

impl Trackers {
    pub fn new(
        root: &Path,
        loaded: ReadHome,
        data: Option<PathBuf>,
        connect: Connect,
    ) -> Result<Self, String> {
        let at = project_runtime::data_dir_or_default(data)
            .map_err(|error| error.to_string())?;
        let mut trackers = tracker::Trackers::new(
            at,
            Box::new(FolderStore::new(root.to_path_buf())),
            connect,
        );
        trackers.follow(&loaded.snapshot);
        Ok(Self { loaded, trackers })
    }

    pub fn catalog(&mut self) -> Result<(), String> {
        let home = self.loaded.snapshot.clone();
        self.loaded.snapshot = run(cataloged(&self.trackers, home))?;
        Ok(())
    }

    pub const fn loaded(&self) -> &ReadHome {
        &self.loaded
    }

    pub const fn stores(&self) -> &tracker::Trackers {
        &self.trackers
    }

    pub fn write(
        &self,
        root: &Path,
        changes: &[Change],
    ) -> Result<Vec<Written>, String> {
        run(async {
            let mut written = Vec::new();
            for change in changes {
                written.push(
                    self.trackers.write(&self.loaded, root, change).await?,
                );
            }
            Ok(written)
        })?
    }

    pub fn put(
        &mut self,
        canvases: &[CanvasWrite],
    ) -> Result<Vec<String>, String> {
        let stores = &mut self.trackers;
        run(async move {
            let mut written = Vec::new();
            for canvas in canvases {
                let wrote = stores
                    .put(&canvas.key, &canvas.file)
                    .await
                    .map_err(|error| error.to_string())?;
                written.extend(wrote.home_file);
            }
            Ok(written)
        })?
    }

    pub fn touches(
        &self,
        changes: &[Change],
        canvases: &[CanvasWrite],
    ) -> Result<Vec<String>, String> {
        let mut touched = Vec::new();
        for change in changes {
            touched.extend(self.trackers.touches_issue(&self.loaded, change)?);
        }
        touched.extend(
            canvases
                .iter()
                .flat_map(|canvas| self.trackers.touches(&canvas.key)),
        );
        Ok(touched)
    }

    pub fn resolved(&self, keys: BTreeSet<IssueKey>, mode: Mode) -> Resolved {
        if keys.is_empty() {
            return Resolved::new();
        }
        let wanted = keys.clone();
        run(self.each(keys, mode)).unwrap_or_else(|error| {
            wanted
                .into_iter()
                .map(|key| (key, Err(error.clone())))
                .collect()
        })
    }

    async fn each(&self, keys: BTreeSet<IssueKey>, mode: Mode) -> Resolved {
        let mut steps = Resolver::new(mode);
        let mut found = Resolved::new();
        for key in keys {
            let reading = self.resolve(&mut steps, &key).await;
            found.insert(key, reading);
        }
        found
    }

    async fn resolve(
        &self,
        resolver: &mut Resolver,
        key: &IssueKey,
    ) -> Result<IssueReading, String> {
        let home = &self.loaded.snapshot;
        loop {
            let looked = if resolver.fresh(key.project) {
                self.trackers.read(home, key).await?
            } else {
                self.trackers.look(home, key).await?
            };
            match resolver.step(&named(home, key.project), key, looked) {
                Ok(reading) => return Ok(reading),
                Err(Missing::Unknown(reason)) => return Err(reason),
                Err(Missing::Refresh(project)) => {
                    let outcome = self.trackers.refresh(home, project).await?;
                    resolver.refreshed(project, outcome);
                }
            }
        }
    }

    pub fn listings(&self) -> Vec<Listed> {
        let home = &self.loaded.snapshot;
        let tracked = home
            .projects()
            .iter()
            .filter(|project| {
                matches!(
                    home::tracker::of(project),
                    home::tracker::Tracker::Configured(_)
                )
            })
            .map(|project| project.id)
            .collect::<Vec<_>>();
        run(async {
            let mut listed = Vec::new();
            for project in &tracked {
                listed.push(Listed {
                    project: *project,
                    listing: self.trackers.listing(home, *project, false).await,
                });
            }
            listed
        })
        .unwrap_or_else(|error| {
            tracked
                .iter()
                .map(|project| Listed {
                    project: *project,
                    listing: Err(error.clone()),
                })
                .collect()
        })
    }

    pub async fn listing(
        &self,
        project: ProjectId,
        offline: bool,
    ) -> Result<IssueListing, String> {
        self.trackers
            .listing(&self.loaded.snapshot, project, offline)
            .await
    }
}

async fn cataloged(trackers: &tracker::Trackers, home: Snapshot) -> Snapshot {
    let mut held = home.clone();
    for project in home.projects() {
        let listing = trackers
            .listing(&home, project.id, true)
            .await
            .unwrap_or_else(|_| IssueListing {
                records: Vec::new(),
                status: ReadStatus {
                    freshness: Freshness::Unknown,
                    complete: false,
                    observed_at: None,
                    failure: None,
                },
            });
        held = held.cataloged(project.id, listing);
    }
    held
}

pub fn connect(
    data: &Path,
    home: &Snapshot,
    project: &home::project::Project,
) -> Result<tracker::Connection, String> {
    adapters::connect(data, home, project, daemon::fleet::project_credential)
}

fn run<F: Future>(future: F) -> Result<F::Output, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map(|runtime| runtime.block_on(future))
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests;
