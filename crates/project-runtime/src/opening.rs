use home::IssueId;
use home::flow::command::Change;
use home::issue::{IssueCreate, Lookup, SendFailure};
use home::operations::{Handle, Intent, Opening};

use crate::operations::{Attempt, Operations};
use crate::relations::relations;
use crate::{IssueTracker, Project, WriteError};

impl Project<'_> {
    pub async fn open_issue(
        &self,
        operations: &Operations,
        fresh: Handle,
        change: &Change,
    ) -> Result<IssueId, WriteError> {
        let issue = Lookup {
            project: change.observation.project(),
            id: self.opened_issue(operations, fresh, change).await?,
        };
        let changes = relations(&change.mutations);
        if !changes.is_empty() {
            let seen = self
                .writer()?
                .native(issue.clone())
                .await
                .map_err(WriteError::Read)?
                .revision();
            self.related(operations, &issue, seen, changes).await?;
        }
        Ok(issue.id)
    }

    async fn opened_issue(
        &self,
        operations: &Operations,
        fresh: Handle,
        change: &Change,
    ) -> Result<IssueId, WriteError> {
        let tracker = self.writer()?;
        let project = change.observation.project();
        let planned = tracker
            .opening(change, fresh)
            .map_err(WriteError::Unsupported)?;
        let (handle, intent) = operations
            .unsettled()?
            .into_iter()
            .find(|record| record.intent.opens(project, &planned))
            .map_or_else(
                || {
                    Intent::create(fresh, project, planned)
                        .map(|intent| (fresh, intent))
                },
                |record| Ok((record.handle, record.intent)),
            )?;
        let mut attempt = operations.begin(handle, intent, true)?;
        let found = tracker
            .opened(project, handle)
            .await
            .map_err(WriteError::Read)?;
        match attempt.opened(&found)? {
            Opening::Settled(id) => Ok(id),
            Opening::Unknown => Err(WriteError::Unknown(handle)),
            Opening::Send(create) => send(tracker, attempt, create).await,
        }
    }
}

async fn send(
    tracker: &dyn IssueTracker,
    mut attempt: Attempt,
    create: IssueCreate,
) -> Result<IssueId, WriteError> {
    let handle = attempt.record().handle;
    match tracker
        .create(attempt.record().intent.project, create)
        .await
    {
        Ok(_) => verify(tracker, attempt).await,
        Err(SendFailure::Rejected(failure)) => {
            attempt.reject()?;
            Err(WriteError::Rejected(failure))
        }
        Err(SendFailure::Invalid) => {
            attempt.reject()?;
            Err(WriteError::Invalid)
        }
        Err(SendFailure::Unknown) => Err(WriteError::Unknown(handle)),
    }
}

async fn verify(
    tracker: &dyn IssueTracker,
    mut attempt: Attempt,
) -> Result<IssueId, WriteError> {
    let handle = attempt.record().handle;
    let found = tracker
        .opened(attempt.record().intent.project, handle)
        .await
        .map_err(|_| WriteError::Unknown(handle))?;
    match attempt.opened(&found)? {
        Opening::Settled(id) => Ok(id),
        Opening::Send(_) | Opening::Unknown => Err(WriteError::Unknown(handle)),
    }
}
