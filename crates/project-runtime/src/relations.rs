use home::flow::command::Mutation;
use home::issue::{
    Lookup, NativeIssue, Relating, RelationChange, RelationRequest,
    RelationStep, SendFailure, Unrelatable,
};
use home::operations::{Handle, Intent, Relate, RelationWork, relating};

use crate::operations::{Attempt, Operations};
use crate::{IssueTracker, Project, WriteError};

impl Project<'_> {
    pub async fn write_relation(
        &self,
        operations: &Operations,
        handle: Handle,
        request: RelationRequest,
    ) -> Result<String, WriteError> {
        let tracker = self.writer()?;
        let mut attempt =
            operations.begin(handle, Intent::relate(&request), true)?;
        let mut plan = observe(tracker, &request, None).await?;
        let mut resuming = true;
        loop {
            match attempt.relate(&plan, resuming)? {
                Relate::Settled(revision) => return Ok(revision),
                Relate::Unknown => return Err(WriteError::Unsettled(handle)),
                Relate::Send(step) => {
                    send(tracker, &mut attempt, &request, step).await?;
                }
            }
            plan = observe(tracker, &request, Some(handle)).await?;
            resuming = false;
        }
    }

    pub(crate) async fn related(
        &self,
        operations: &Operations,
        issue: &Lookup,
        mut seen: String,
        changes: Vec<RelationChange>,
    ) -> Result<NativeIssue, WriteError> {
        let unsettled = operations.unsettled()?;
        for work in relating(&unsettled, issue, changes) {
            seen = match work {
                RelationWork::Resume { handle, request } => {
                    self.write_relation(operations, handle, request).await?
                }
                RelationWork::Write(change) => {
                    let request = RelationRequest {
                        issue: issue.clone(),
                        change,
                        seen,
                    };
                    self.write_relation(
                        operations,
                        operations.handle()?,
                        request,
                    )
                    .await?
                }
            };
        }
        self.writer()?
            .native(issue.clone())
            .await
            .map_err(WriteError::Read)
    }
}

pub fn relations(mutations: &[Mutation]) -> Vec<RelationChange> {
    mutations
        .iter()
        .filter_map(|mutation| match mutation {
            Mutation::Relate {
                relation, targets, ..
            } => Some(RelationChange {
                relation: relation.clone(),
                targets: targets.clone(),
            }),
            _ => None,
        })
        .collect()
}

async fn send(
    tracker: &dyn IssueTracker,
    attempt: &mut Attempt,
    request: &RelationRequest,
    step: RelationStep,
) -> Result<(), WriteError> {
    let failure = match tracker
        .write_relation(request.issue.clone(), step)
        .await
    {
        Ok(()) => return Ok(()),
        Err(SendFailure::Unknown) => {
            return Err(WriteError::Unsettled(attempt.record().handle));
        }
        Err(SendFailure::Rejected(failure)) => WriteError::Rejected(failure),
        Err(SendFailure::Invalid) => WriteError::Invalid,
    };
    attempt.reject()?;
    Err(failure)
}

async fn observe(
    tracker: &dyn IssueTracker,
    request: &RelationRequest,
    sent: Option<Handle>,
) -> Result<Relating, WriteError> {
    tracker
        .plan_relation(request.issue.clone(), request.change.clone())
        .await
        .map_err(|failure| match (sent, failure) {
            (Some(handle), _) => WriteError::Unsettled(handle),
            (None, Unrelatable::Read(read)) => WriteError::Read(read),
            (None, Unrelatable::Unheld(reason)) => WriteError::Unheld(reason),
        })
}
