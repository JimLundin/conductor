use home::backend::{Capability, Refusal};
use home::issue::{
    Comment, CommentRequest, CommentWrite, Entry, Lookup, ReadFailure,
    SendFailure,
};
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
    #[error(transparent)]
    Operation(#[from] Error),
    #[error(
        "the outcome of operation {0} is unknown; retry it to look for its marker"
    )]
    Unknown(Handle),
    #[error("the tracker can't make the change: {0}")]
    Unsupported(String),
    #[error(
        "the tracker rejected operation {handle} after {applied} of its writes: {failure}"
    )]
    EditRejected {
        handle: Handle,
        applied: usize,
        failure: ReadFailure,
    },
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

    pub(crate) fn writer(&self) -> Result<&dyn IssueTracker, WriteError> {
        self.registry
            .preflight(self.backends, Capability::WriteIssues)
            .map_err(WriteError::Refused)?;
        selected(&self.registry.trackers, self.backends.tracker.as_ref())
            .map_err(|_| WriteError::Read(ReadFailure::Unavailable))
    }

    pub async fn append_note(
        &self,
        operations: &Operations,
        handle: Handle,
        issue: Lookup,
        entry: &Entry,
    ) -> Result<Comment, WriteError> {
        let tracker = self.writer()?;
        let write = CommentWrite::Create {
            body: tracker.logged(entry, handle),
        };
        let comments = tracker
            .comments(issue.clone())
            .await
            .map_err(WriteError::Read)?;
        let seen = write.precondition(&comments);
        self.write_comment(
            operations,
            handle,
            CommentRequest { issue, write, seen },
        )
        .await
    }

    pub async fn write_comment(
        &self,
        operations: &Operations,
        handle: Handle,
        request: CommentRequest,
    ) -> Result<Comment, WriteError> {
        let tracker = self.writer()?;
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
