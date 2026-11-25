use std::path::PathBuf;

use anyhow::ensure;
use git2::Repository;
use home::backend::{Capability, Refusal};
use project_runtime::{Completion, Failure, VersionControl};
use vcs::command::{
    Capture, Compare, Comparison, Install, Integrate, Prepare, Prepared,
    PreserveAndRemove, Revision,
};

mod compare;
mod features;
mod install;
mod prepare;
pub mod registration;
mod release;
pub mod remote;
mod snapshot;

pub const ADAPTER: &str = "git2";

pub struct GitWorkspaces {
    workspaces: PathBuf,
}

impl GitWorkspaces {
    #[must_use]
    pub const fn new(workspaces: PathBuf) -> Self {
        Self { workspaces }
    }

    fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(PathBuf) -> anyhow::Result<T> + Send + 'static,
    ) -> Completion<'_, T> {
        let root = self.workspaces.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || operation(root))
                .await
                .map_err(|_| Failure::Unavailable)?
                .map_err(|error| Failure::Workspace(failure(error)))
        })
    }
}

fn failure(error: anyhow::Error) -> vcs::Error {
    error
        .downcast::<vcs::Error>()
        .unwrap_or_else(|report| vcs::Error::Failed(format!("{report:#}")))
}

impl VersionControl for GitWorkspaces {
    fn prepare(&self, request: Prepare) -> Completion<'_, Prepared> {
        self.run(move |base| prepare::create(&base, &request))
    }

    fn capture(&self, request: Capture) -> Completion<'_, Revision> {
        self.run(move |base| {
            let session = prepare::open(&base, &request.workspace)?;
            let candidate = snapshot::captured(&session, &request.attempt.0)?;
            Ok(Revision(candidate.to_string()))
        })
    }

    fn install(&self, request: Install) -> Completion<'_, ()> {
        self.run(move |base| {
            let session = prepare::open(&base, &request.workspace)?;
            let oid = snapshot::retained(
                &session.repo,
                &session.name,
                &request.revision.0,
            )?;
            let branch =
                format!("refs/heads/{}", session.record.location.branch);
            install_recorded(&session.repo, &branch, &request.attempt.0, oid)
        })
    }

    fn compare(&self, request: Compare) -> Completion<'_, Comparison> {
        self.run(move |base| {
            let session = prepare::open(&base, &request.workspace)?;
            prepare::branch(&session.repo, &session.record.location.branch)?;
            let baseline = snapshot::retained(
                &session.repo,
                &session.name,
                &request.baseline.0,
            )?;
            let revision = snapshot::retained(
                &session.repo,
                &session.name,
                &request.revision.0,
            )?;
            compare::revisions(&session.repo, baseline, revision)
        })
    }

    fn integrate(&self, _request: Integrate) -> Completion<'_, u64> {
        Box::pin(std::future::ready(Err(Failure::Refused(
            Refusal::CapabilityUnavailable {
                capability: Capability::IntegrateSession,
            },
        ))))
    }

    fn preserve_and_remove(
        &self,
        request: PreserveAndRemove,
    ) -> Completion<'_, vcs::Kept> {
        self.run(move |base| release::workspace(&base, &request.workspace))
    }
}

fn install_recorded(
    repo: &Repository,
    branch: &str,
    attempt: &str,
    revision: git2::Oid,
) -> anyhow::Result<()> {
    if let Some(recorded) = install::recorded(repo, branch, attempt)? {
        ensure!(
            recorded == revision,
            "snapshot attempt identifies different work"
        );
    }
    install::run(repo, branch, attempt, revision)?;
    Ok(())
}

#[cfg(test)]
mod tests;
