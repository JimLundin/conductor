mod store;

use super::Project;
use home::issue::{
    IssueComment, IssueListing, IssueReading, ListPage, Lookup, ReadFailure,
    ReadMode, cache::Scan,
};
use home::{ProjectId, UnixMillis, backend::Refusal, flow::Flow};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use store::Scope;

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("issue cache: {0}")]
    Store(String),
    #[error("{}", .0.label())]
    Refused(Refusal),
}

pub struct IssueCache<'a> {
    data: &'a Path,
    flow: &'a Flow,
}

impl<'a> IssueCache<'a> {
    #[must_use]
    pub const fn new(data: &'a Path, flow: &'a Flow) -> Self {
        Self { data, flow }
    }
}

fn now() -> Result<UnixMillis, CacheError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(UnixMillis::since_epoch)
        .map_err(|err| CacheError::Store(err.to_string()))
}

impl Project<'_> {
    fn scope(
        &self,
        cache: &IssueCache<'_>,
        project: ProjectId,
    ) -> Result<Scope, CacheError> {
        let selection = self
            .backends
            .tracker
            .as_ref()
            .ok_or(CacheError::Refused(Refusal::TrackerNotConfigured))?;
        Scope::new(cache.data, project, selection, cache.flow)
    }

    pub fn canvas_comments(
        &self,
        cache: &IssueCache<'_>,
        project: ProjectId,
    ) -> Result<Vec<IssueComment>, CacheError> {
        Ok(self.scope(cache, project)?.load()?.comments().to_vec())
    }

    pub fn posted(
        &self,
        cache: &IssueCache<'_>,
        project: ProjectId,
        comment: IssueComment,
    ) -> Result<(), CacheError> {
        self.scope(cache, project)?
            .save(|records| records.posted(comment))
            .map(drop)
    }

    pub async fn issues(
        &self,
        cache: &IssueCache<'_>,
        project: ProjectId,
        mode: ReadMode,
    ) -> Result<IssueListing, CacheError> {
        let scope = self.scope(cache, project)?;
        match mode {
            ReadMode::Refresh => {
                self.refresh_list(&scope, project, cache.flow).await
            }
            ReadMode::Cached => {
                Ok(scope.load()?.listing(cache.flow, false, None))
            }
            ReadMode::Failed(cause) => {
                Ok(scope.load()?.listing(cache.flow, false, Some(cause)))
            }
        }
    }

    async fn refresh_list(
        &self,
        scope: &Scope,
        project: ProjectId,
        flow: &Flow,
    ) -> Result<IssueListing, CacheError> {
        let generation = scope.begin()?;
        let scan = match self.scan(project).await {
            Ok(scan) => scan,
            Err(error) => {
                return Ok(scope.load()?.listing(flow, false, Some(error)));
            }
        };
        let when = now()?;
        let (records, publication) =
            scope.save(|records| records.publish(scan, generation, when))?;
        Ok(records.listing(flow, publication.is_ok(), publication.err()))
    }

    async fn scan(&self, project: ProjectId) -> Result<Scan, ReadFailure> {
        let mut scan = Scan::default();
        let tracker = self.tracker()?;
        let mut continuation = None;
        loop {
            let page = tracker
                .list(ListPage {
                    project,
                    continuation,
                })
                .await?;
            continuation = scan.append(project, page)?;
            if continuation.is_none() {
                scan.commented(tracker.canvas_comments(project).await?);
                return Ok(scan);
            }
        }
    }

    pub async fn issue(
        &self,
        cache: &IssueCache<'_>,
        lookup: Lookup,
        mode: ReadMode,
    ) -> Result<IssueReading, CacheError> {
        let scope = self.scope(cache, lookup.project)?;
        match mode {
            ReadMode::Refresh => {
                self.refresh_issue(&scope, cache.flow, lookup).await
            }
            ReadMode::Cached => Ok(scope.load()?.reading(
                cache.flow,
                Some(&lookup.id),
                false,
                None,
            )),
            ReadMode::Failed(cause) => Ok(scope.load()?.reading(
                cache.flow,
                Some(&lookup.id),
                false,
                Some(cause),
            )),
        }
    }

    async fn refresh_issue(
        &self,
        scope: &Scope,
        flow: &Flow,
        lookup: Lookup,
    ) -> Result<IssueReading, CacheError> {
        let generation = scope.begin()?;
        let record = match self.read_issue(lookup.clone()).await {
            Ok(record) => record,
            Err(error) => {
                return Ok(scope.load()?.failed_read(flow, &lookup, error));
            }
        };
        let when = now()?;
        let (records, accepted) = scope.save(|records| {
            records.accept(&lookup, record, generation, when)
        })?;
        Ok(match accepted {
            Ok(id) => records.reading(flow, Some(&id), true, None),
            Err(error) => records.failed_read(flow, &lookup, error),
        })
    }
}
