use home::flow::command::Mutation;
use home::issue::{Lookup, NativeIssue, SendFailure};
use home::operations::{Advance, Handle, Intent};

use crate::operations::{Attempt, Operations};
use crate::{IssueTracker, Project, WriteError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueEdit {
    pub issue: Lookup,
    pub seen: String,
    pub mutations: Vec<Mutation>,
}

impl Project<'_> {
    pub async fn edit_issue(
        &self,
        operations: &Operations,
        handle: Handle,
        edit: IssueEdit,
    ) -> Result<NativeIssue, WriteError> {
        let tracker = self.writer()?;
        let observed = tracker
            .native(edit.issue.clone())
            .await
            .map_err(WriteError::Read)?;
        let intent = operations
            .status(handle)?
            .map(|record| record.intent)
            .map_or_else(|| planned(tracker, &observed, &edit), Ok)?;
        if intent.edits(&edit.issue, &edit.seen)?.is_empty() {
            return Ok(observed);
        }
        let attempt = operations.begin(handle, intent, true)?;
        deliver(tracker, attempt, edit.issue, observed).await
    }
}

fn planned(
    tracker: &dyn IssueTracker,
    observed: &NativeIssue,
    edit: &IssueEdit,
) -> Result<Intent, WriteError> {
    Intent::unchanged(&edit.seen, observed)?;
    let writes = tracker
        .writes(&edit.issue, observed, &edit.mutations)
        .map_err(WriteError::Unsupported)?;
    Ok(Intent::edit(&edit.issue, edit.seen.clone(), writes))
}

async fn deliver(
    tracker: &dyn IssueTracker,
    mut attempt: Attempt,
    issue: Lookup,
    mut observed: NativeIssue,
) -> Result<NativeIssue, WriteError> {
    let handle = attempt.record().handle;
    let mut resuming = true;
    loop {
        let write = match attempt.advance(&observed, resuming)? {
            Advance::Done => return Ok(observed),
            Advance::Unknown => return Err(WriteError::Unknown(handle)),
            Advance::Send(write) => write,
        };
        match tracker.write_issue(issue.clone(), write).await {
            Ok(()) => {}
            Err(SendFailure::Rejected(failure)) => {
                attempt.reject()?;
                return Err(WriteError::EditRejected {
                    handle,
                    applied: attempt.record().confirmed().len(),
                    failure,
                });
            }
            Err(SendFailure::Invalid) => {
                attempt.reject()?;
                return Err(WriteError::Invalid);
            }
            Err(SendFailure::Unknown) => {
                return Err(WriteError::Unknown(handle));
            }
        }
        observed = tracker
            .native(issue.clone())
            .await
            .map_err(|_| WriteError::Unknown(handle))?;
        resuming = false;
    }
}
