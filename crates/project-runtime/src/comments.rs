use home::backend::{Capability, Refusal};
use home::issue::{Comment, CommentRequest, Lookup, ReadFailure, SendFailure};
use home::operations::{Handle, Intent, Next};

use crate::operations::{Attempt, Error, Operations};
use crate::{IssueTracker, Project, selected};

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("the tracker cannot take the write: {0:?}")]
    Refused(Refusal),
    #[error("could not read the issue's comments: {0}")]
    Read(ReadFailure),
    #[error("the tracker rejected the write: {0}")]
    Rejected(ReadFailure),
    #[error("the tracker found the write invalid")]
    Invalid,
    #[error("{0}")]
    Unheld(String),
    #[error(transparent)]
    Operation(#[from] Error),
    #[error(
        "the outcome of operation {0} is unknown; retry it to look for its marker"
    )]
    Unknown(Handle),
}

impl From<home::operations::Refusal> for WriteError {
    fn from(refusal: home::operations::Refusal) -> Self {
        Self::Operation(Error::Refused(refusal))
    }
}

impl Project<'_> {
    pub async fn comments(
        &self,
        issue: Lookup,
    ) -> Result<Vec<Comment>, ReadFailure> {
        self.tracker()?.comments(issue).await
    }

    pub async fn write_comment(
        &self,
        operations: &Operations,
        handle: Handle,
        request: CommentRequest,
    ) -> Result<Comment, WriteError> {
        self.registry
            .preflight(self.backends, Capability::WriteIssues)
            .map_err(WriteError::Refused)?;
        let tracker =
            selected(&self.registry.trackers, self.backends.tracker.as_ref())
                .map_err(|_| WriteError::Read(ReadFailure::Unavailable))?;
        let intent = Intent::comment(handle, &request)?;
        let mut attempt = operations.begin(handle, intent, true)?;
        let observed = tracker
            .comments(request.issue.clone())
            .await
            .map_err(WriteError::Read)?;
        match attempt.next(&observed)? {
            Next::Settled(comment) => Ok(comment),
            Next::Unknown => Err(WriteError::Unknown(handle)),
            Next::Send => deliver(tracker, attempt, request).await,
        }
    }
}

async fn deliver(
    tracker: &dyn IssueTracker,
    mut attempt: Attempt,
    request: CommentRequest,
) -> Result<Comment, WriteError> {
    let handle = attempt.record().handle;
    match tracker
        .write_comment(request.issue.clone(), request.write)
        .await
    {
        Ok(_) => verify(tracker, attempt, request.issue).await,
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
    issue: Lookup,
) -> Result<Comment, WriteError> {
    let handle = attempt.record().handle;
    let observed = tracker
        .comments(issue)
        .await
        .map_err(|_| WriteError::Unknown(handle))?;
    match attempt.next(&observed)? {
        Next::Settled(comment) => Ok(comment),
        Next::Send | Next::Unknown => Err(WriteError::Unknown(handle)),
    }
}
