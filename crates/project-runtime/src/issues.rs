mod store;

use super::Project;
use home::issue::{
    IssueListing, IssueReading, ListPage, Lookup, ReadFailure, ReadMode,
    cache::Scan,
};
use home::{ProjectId, backend::Refusal, flow::Flow};
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

fn now() -> Result<u64, CacheError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| CacheError::Store(err.to_string()))?;
    elapsed
        .as_millis()
        .try_into()
        .map_err(|err: std::num::TryFromIntError| {
            CacheError::Store(err.to_string())
        })
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

    /// # Errors
    /// No tracker is configured or the host cache could not be read or updated.
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
                return Ok(scan);
            }
        }
    }

    /// # Errors
    /// No tracker is configured or the host cache could not be read or updated.
    pub async fn issue(
        &self,
        cache: &IssueCache<'_>,
        project: ProjectId,
        selector: &str,
        mode: ReadMode,
    ) -> Result<IssueReading, CacheError> {
        let scope = self.scope(cache, project)?;
        let records = scope.load()?;
        let lookup = match records.lookup(project, selector) {
            Ok(request) => request,
            Err(error) => {
                return Ok(records.reading(
                    cache.flow,
                    None,
                    false,
                    Some(error),
                ));
            }
        };
        match mode {
            ReadMode::Refresh => self.resolve(&scope, cache.flow, lookup).await,
            ReadMode::Cached => Ok(records.reading(
                cache.flow,
                lookup.expected.as_ref(),
                false,
                None,
            )),
            ReadMode::Failed(cause) => Ok(records.reading(
                cache.flow,
                lookup.expected.as_ref(),
                false,
                Some(cause),
            )),
        }
    }

    async fn resolve(
        &self,
        scope: &Scope,
        flow: &Flow,
        mut lookup: Lookup,
    ) -> Result<IssueReading, CacheError> {
        if home::IssueId::parse(&lookup.reference).is_some() {
            let listing =
                self.refresh_list(scope, lookup.project, flow).await?;
            let records = scope.load()?;
            let previous = lookup.expected.as_ref();
            lookup = match records
                .lookup(lookup.project, &lookup.reference)
                .ok()
                .filter(|resolved| {
                    home::IssueId::parse(&resolved.reference).is_none()
                }) {
                Some(resolved) => resolved,
                None => {
                    return Ok(records.reading(
                        flow,
                        previous,
                        false,
                        listing
                            .status
                            .failure
                            .or(Some(ReadFailure::Unavailable)),
                    ));
                }
            };
        }
        self.refresh_issue(scope, flow, lookup).await
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
