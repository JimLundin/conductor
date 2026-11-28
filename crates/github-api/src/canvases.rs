use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use github::canvas::{self, Found};
use github::configuration::{Configuration, selected};
use home::backend::{Backends, CredentialState, Selection};
use home::canvas::{Broken, CanvasEntry, CanvasFile, CanvasKey, Held};
use home::flow::Flow;
use home::flow::snapshot::Snapshot;
use home::issue::{
    CommentRequest, CommentWrite, IssueComment, Lookup, ReadFailure,
};
use home::project::Project;
use home::{IssueId, ProjectId};
use home_store::{CanvasStore, Putting, StoreError, Wrote};
use project_runtime::issues::IssueCache;
use project_runtime::operations::Operations;
use project_runtime::tracker::{Connection, Connector, Reader};
use project_runtime::{Registration, Registry};

use crate::{FleetResolver, HostCredentials};

pub struct Github {
    selection: Selection,
    flow: Flow,
    mapping: Configuration,
    credentials: HostCredentials,
    endpoint: String,
}

impl Connector for Github {
    fn connect(&self) -> Result<Reader, ReadFailure> {
        let reader = self
            .credentials
            .connect_at(&self.selection, &self.endpoint)
            .map_err(crate::read_failure)?
            .tracker(self.flow.clone(), self.mapping.clone());
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
                flow,
                mapping,
                credentials: HostCredentials::new(data),
                endpoint: String::from(endpoint),
            }),
        })
    }

    fn cache(&self) -> IssueCache<'_> {
        IssueCache::new(&self.data, &self.github.flow)
    }

    fn found(
        &self,
        project: ProjectId,
        issue: Option<&IssueId>,
    ) -> Result<Held<Found>, StoreError> {
        if project != self.project {
            return Ok(Held::default());
        }
        let comments = Registry::default()
            .project(&self.backends)
            .canvas_comments(&self.cache(), self.project)
            .map_err(unavailable)?;
        Ok(canvas::found(comments, issue))
    }

    async fn write(
        &self,
        key: &CanvasKey,
        file: &CanvasFile,
    ) -> Result<(), StoreError> {
        let operations = Operations::open(&self.data).map_err(unavailable)?;
        let handle = operations.handle().map_err(unavailable)?;
        let body = canvas::comment(
            &key.name,
            &file.by,
            &handle.marker(),
            &file.canvas,
        )
        .map_err(|too_long| StoreError::Refused {
            key: key.clone(),
            reason: too_long.to_string(),
        })?;
        let reader = self.github.connect().map_err(unavailable)?;
        let mut registry = Registry::default();
        registry
            .register_tracker(reader.registration, reader.tracker)
            .map_err(|refusal| unavailable(refusal.label()))?;
        let project = registry.project(&self.backends);
        let issue = Lookup {
            project: key.project,
            id: key.issue.clone(),
        };
        let comments =
            project.comments(issue.clone()).await.map_err(unavailable)?;
        let write = comments
            .iter()
            .find(|comment| canvas::holds(comment, &key.name))
            .map_or_else(
                || CommentWrite::Create { body: body.clone() },
                |comment| CommentWrite::Edit {
                    comment: comment.id.clone(),
                    body: body.clone(),
                },
            );
        let seen = write.precondition(&comments);
        let written = project
            .write_comment(
                &operations,
                handle,
                CommentRequest { issue, write, seen },
            )
            .await
            .map_err(unavailable)?;
        project
            .posted(
                &self.cache(),
                key.project,
                IssueComment {
                    issue: key.issue.clone(),
                    comment: written,
                },
            )
            .map_err(unavailable)
    }
}

fn unavailable(error: impl Display) -> StoreError {
    StoreError::Unavailable {
        reason: format!("GitHub's canvases: {error}"),
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
