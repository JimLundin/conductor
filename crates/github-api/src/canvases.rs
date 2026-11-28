use std::collections::BTreeSet;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use github::access::Repositories;
use github::canvas::{self, Found};
use github::configuration::{Configuration, selected};
use home::backend::{Backends, CredentialState, Selection};
use home::canvas::{Broken, CanvasEntry, CanvasFile, CanvasKey, Held};
use home::flow::Flow;
use home::flow::snapshot::Snapshot;
use home::issue::{
    CommentRequest, CommentWrite, IssueComment, IssueListing, Lookup,
    Observation, ReadFailure,
};
use home::operations::Handle;
use home::project::Project;
use home::tracker::Ids;
use home::{IssueId, IssueKey, ProjectId};
use home_store::{CanvasStore, Putting, StoreError, Wrote};
use project_runtime::operations::Operations;
use project_runtime::tracker::{
    Connection, Connector, Reader, Refreshing, TrackerStore,
};
use project_runtime::{Registration, Registry};

use crate::{CanvasCache, FleetResolver, HostCredentials, Tracker};

pub struct Github {
    selection: Selection,
    ids: Ids,
    repositories: Repositories,
    flow: Flow,
    mapping: Configuration,
    credentials: HostCredentials,
    endpoint: String,
}

impl Github {
    fn tracker(&self) -> Result<Tracker, ReadFailure> {
        Ok(self
            .credentials
            .connect_at(&self.selection, &self.endpoint)
            .map_err(crate::read_failure)?
            .tracker(
                self.flow.clone(),
                self.mapping.clone(),
                self.ids.clone(),
                self.repositories.clone(),
            ))
    }
}

impl Connector for Github {
    fn connect(&self) -> Result<Reader, ReadFailure> {
        let reader = self.tracker()?;
        Ok(Reader {
            registration: Registration {
                selection: self.selection.clone(),
                capabilities: reader.capabilities(),
                credential: CredentialState::Available,
            },
            tracker: Arc::new(reader),
        })
    }
}

pub struct Canvases {
    data: PathBuf,
    project: ProjectId,
    backends: Backends,
    github: Arc<Github>,
}

pub fn connect(
    data: &Path,
    home: &Snapshot,
    project: &Project,
    resolver: FleetResolver,
) -> Result<Connection, String> {
    let tracker = selected(home, &project.name)?
        .tracker
        .ok_or_else(|| String::from("its adapter isn't GitHub"))?;
    let github = Arc::new(Github {
        selection: tracker.selection.clone(),
        ids: home.ids().clone(),
        repositories: Repositories::of(home.projects()),
        flow: tracker.flow.clone(),
        mapping: tracker.mapping,
        credentials: HostCredentials::with_fleet(data, resolver),
        endpoint: String::from(github::mapping::AUTHORITY),
    });
    Ok(Connection {
        canvases: Box::new(Canvases {
            data: data.to_path_buf(),
            project: project.id,
            backends: project.backends.clone(),
            github: Arc::clone(&github),
        }),
        connector: github,
    })
}

impl Canvases {
    #[cfg(test)]
    pub(crate) fn at(
        data: &Path,
        project: ProjectId,
        backends: Backends,
        flow: Flow,
        endpoint: &str,
    ) -> Result<Self, String> {
        let selection = backends
            .tracker
            .clone()
            .ok_or_else(|| String::from("the project has no tracker"))?;
        let mapping = github::configuration::load("", &flow)?;
        Ok(Self {
            data: data.to_path_buf(),
            project,
            backends,
            github: Arc::new(Github {
                selection,
                ids: Ids::default().with(project, github::ISSUER),
                repositories: Repositories::default(),
                flow,
                mapping,
                credentials: HostCredentials::new(data),
                endpoint: String::from(endpoint),
            }),
        })
    }

    fn cache(&self) -> Result<CanvasCache, StoreError> {
        CanvasCache::open(&self.data, self.project, &self.github.selection)
            .map_err(unavailable)
    }

    fn found(
        &self,
        project: ProjectId,
        issue: Option<&IssueId>,
    ) -> Result<Held<Found>, StoreError> {
        if project != self.project {
            return Ok(Held::default());
        }
        let comments = self.cache()?.comments().map_err(unavailable)?;
        Ok(canvas::found(comments, issue))
    }

    async fn write(
        &self,
        key: &CanvasKey,
        file: &CanvasFile,
    ) -> Result<(), StoreError> {
        let operations = Operations::home(&self.data).map_err(unavailable)?;
        let fresh = operations.handle().map_err(unavailable)?;
        let body =
            canvas::comment(&key.name, &file.by, &fresh.marker(), &file.canvas)
                .map_err(|too_long| StoreError::Refused {
                    key: key.clone(),
                    reason: too_long.to_string(),
                })?;
        let cache = self.cache()?;
        let reader = self.github.connect().map_err(unavailable)?;
        let mut registry = Registry::default();
        registry
            .register_tracker(reader.registration, reader.tracker)
            .map_err(|refusal| unavailable(refusal.label()))?;
        let project = registry.project(&self.backends);
        let writer = Writer {
            operations: &operations,
            project: &project,
            cache: &cache,
        };
        writer.settle(key).await?;
        let issue = Lookup {
            project: key.project,
            id: key.issue.clone(),
        };
        let comments =
            project.comments(issue.clone()).await.map_err(unavailable)?;
        let write = canvas::write(&comments, &key.name, body);
        let seen = write.precondition(&comments);
        writer
            .put(key, fresh, CommentRequest { issue, write, seen })
            .await
    }
}

struct Writer<'a> {
    operations: &'a Operations,
    project: &'a project_runtime::Project<'a>,
    cache: &'a CanvasCache,
}

impl Writer<'_> {
    async fn settle(&self, key: &CanvasKey) -> Result<(), StoreError> {
        let Some(creating) = self.cache.creating(key).map_err(unavailable)?
        else {
            return Ok(());
        };
        let awaited = self
            .operations
            .status(creating)
            .map_err(unavailable)?
            .and_then(|record| record.awaited());
        if let Some(request) = awaited {
            self.send(creating, request).await?;
        }
        self.cache.created(key, creating).map_err(unavailable)
    }

    async fn put(
        &self,
        key: &CanvasKey,
        fresh: Handle,
        request: CommentRequest,
    ) -> Result<(), StoreError> {
        let CommentWrite::Create { .. } = request.write else {
            return self.send(fresh, request).await;
        };
        let creating =
            self.cache.create_with(key, fresh).map_err(unavailable)?;
        self.send(creating, request).await?;
        self.cache.created(key, creating).map_err(unavailable)
    }

    async fn send(
        &self,
        handle: Handle,
        request: CommentRequest,
    ) -> Result<(), StoreError> {
        let issue = request.issue.id.clone();
        let comment = self
            .project
            .write_comment(self.operations, handle, request)
            .await
            .map_err(unavailable)?;
        self.cache
            .posted(IssueComment { issue, comment })
            .map_err(unavailable)
    }
}

fn unavailable(error: impl Display) -> StoreError {
    StoreError::Unavailable {
        reason: format!("GitHub's canvases: {error}"),
    }
}

impl TrackerStore for Canvases {
    fn refresh<'a>(&'a self, listing: &'a IssueListing) -> Refreshing<'a> {
        Box::pin(async move {
            let marked = self.github.tracker()?.marked_comments().await?;
            let listed = listing
                .records
                .iter()
                .map(Observation::id)
                .collect::<BTreeSet<_>>();
            let kept = marked
                .into_iter()
                .filter(|comment| listed.contains(&comment.issue))
                .collect::<Vec<_>>();
            CanvasCache::open(&self.data, self.project, &self.github.selection)
                .and_then(|cache| cache.refreshed(&kept))
                .map_err(|_| ReadFailure::Unavailable)
        })
    }

    fn refresh_issue<'a>(&'a self, issue: &'a IssueKey) -> Refreshing<'a> {
        Box::pin(async move {
            if issue.project != self.project {
                return Ok(());
            }
            let comments = self
                .github
                .tracker()?
                .issue_comments(Lookup {
                    project: issue.project,
                    id: issue.id.clone(),
                })
                .await?
                .into_iter()
                .map(|comment| IssueComment {
                    issue: issue.id.clone(),
                    comment,
                })
                .collect();
            self.cache()
                .map_err(|_| ReadFailure::Unavailable)?
                .refreshed_issue(&issue.id, &canvas::marked(comments))
                .map_err(|_| ReadFailure::Unavailable)
        })
    }
}

impl CanvasStore for Canvases {
    fn all(&self) -> Result<Held<CanvasKey>, StoreError> {
        let found = self.found(self.project, None)?;
        Ok(Held {
            canvases: found
                .canvases
                .into_iter()
                .map(|kept| CanvasKey {
                    project: self.project,
                    issue: kept.issue,
                    name: kept.name,
                })
                .collect(),
            strays: found.strays,
        })
    }

    fn list(
        &self,
        project: &ProjectId,
        issue: &IssueId,
    ) -> Result<Held<Result<CanvasEntry, Broken>>, StoreError> {
        let found = self.found(*project, Some(issue))?;
        Ok(Held {
            canvases: found
                .canvases
                .into_iter()
                .map(|kept| match kept.canvas {
                    Ok(_) => Ok(CanvasEntry {
                        name: kept.name,
                        written: kept.updated,
                        by: kept.by,
                    }),
                    Err(error) => Err(Broken {
                        name: kept.name,
                        error,
                    }),
                })
                .collect(),
            strays: found.strays,
        })
    }

    fn get(&self, key: &CanvasKey) -> Result<Option<CanvasFile>, StoreError> {
        self.found(key.project, Some(&key.issue))?
            .canvases
            .into_iter()
            .find(|kept| kept.name == key.name)
            .map(|kept| {
                kept.canvas
                    .map(|canvas| CanvasFile {
                        written: kept.updated,
                        by: kept.by,
                        canvas,
                    })
                    .map_err(|error| StoreError::Invalid {
                        key: key.clone(),
                        error: Box::new(error),
                    })
            })
            .transpose()
    }

    fn put<'a>(
        &'a mut self,
        key: &'a CanvasKey,
        file: &'a CanvasFile,
    ) -> Putting<'a> {
        Box::pin(async move {
            self.write(key, file).await.map(|()| Wrote::default())
        })
    }
}
