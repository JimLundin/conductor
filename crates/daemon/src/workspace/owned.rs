use std::path::{Path, PathBuf};
use std::sync::Arc;

use home::backend::{Backends, Capability, Refusal, Selection};
use host::log::HostSeq;
use host::session::{SessionDirectory, SessionId};
use host::workspace::{SnapshotDiff, Workspace};
use project_runtime::{Failure, Registry, VersionControl};
use vcs::command::{
    Capture, Compare, Install, OwnedWorkspace, Prepare, PreserveAndRemove,
    Revision, SnapshotAttempt, WorkspaceId,
};
use vcs::{Error, Kept, Worktree};

pub type RegisterWorkspaces = fn(
    &mut Registry,
    Arc<dyn VersionControl>,
    &Selection,
    &Path,
) -> Result<(), Refusal>;

#[derive(Clone)]
pub struct Workspaces {
    adapter: Arc<dyn VersionControl>,
    register: RegisterWorkspaces,
    root: PathBuf,
}

impl std::fmt::Debug for Workspaces {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Workspaces")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Workspaces {
    #[must_use]
    pub fn new(
        root: PathBuf,
        adapter: Arc<dyn VersionControl>,
        register: RegisterWorkspaces,
    ) -> Self {
        Self {
            root,
            adapter,
            register,
        }
    }

    pub(crate) fn preflight(
        &self,
        backends: &Backends,
        directory: &SessionDirectory,
    ) -> Result<(), Refusal> {
        let mut registered = Registry::default();
        let selection = backends
            .version_control
            .as_ref()
            .ok_or(Refusal::VersionControlNotConfigured)?;
        (self.register)(
            &mut registered,
            Arc::clone(&self.adapter),
            selection,
            Path::new(&directory.0),
        )?;
        [
            Capability::PrepareWorkspace,
            Capability::CaptureSnapshot,
            Capability::InstallSnapshot,
        ]
        .into_iter()
        .try_for_each(|capability| registered.preflight(backends, capability))
    }

    pub(crate) async fn create(
        &self,
        original: &SessionDirectory,
        id: SessionId,
        base: Option<&str>,
    ) -> Result<Workspace, Error> {
        let prepared = self
            .adapter
            .prepare(Prepare {
                directory: original.0.clone(),
                name: WorkspaceId(id.to_string()),
                base: base.map(str::to_owned),
            })
            .await
            .map_err(failure)?;
        Ok(Workspace {
            repository: prepared.repository,
            original: prepared.original,
            worktree: Worktree(prepared.root),
            directory: SessionDirectory(prepared.directory),
            branch: prepared.branch,
        })
    }

    fn owned(
        &self,
        workspace: &Workspace,
        id: SessionId,
    ) -> Result<OwnedWorkspace, Error> {
        let name = id.to_string();
        let root = self.root.canonicalize().map_err(|error| {
            Error::failed("worktree path", &error.to_string())
        })?;
        if root.join(&name) != Path::new(&workspace.worktree.0) {
            return Err(Error::failed(
                "worktree path",
                "does not match the session's owned workspace",
            ));
        }
        Ok(OwnedWorkspace {
            id: WorkspaceId(name),
            directory: workspace.directory.0.clone(),
        })
    }

    pub(crate) async fn snapshot(
        &self,
        workspace: &Workspace,
        id: SessionId,
        attempt: SnapshotAttempt,
    ) -> Result<String, Error> {
        let owned = self.owned(workspace, id)?;
        let revision = self
            .adapter
            .capture(Capture {
                workspace: owned.clone(),
                attempt: attempt.clone(),
            })
            .await
            .map_err(failure)?;
        self.adapter
            .install(Install {
                workspace: owned,
                revision: revision.clone(),
                attempt,
            })
            .await
            .map_err(failure)?;
        Ok(revision.0)
    }

    pub(crate) async fn diff(
        &self,
        workspace: &Workspace,
        id: SessionId,
        snapshot: HostSeq,
        revision: &str,
        baseline: &str,
    ) -> Result<SnapshotDiff, Error> {
        let compared = self
            .adapter
            .compare(Compare {
                workspace: self.owned(workspace, id)?,
                revision: Revision(String::from(revision)),
                baseline: Revision(String::from(baseline)),
            })
            .await
            .map_err(failure)?;
        Ok(SnapshotDiff {
            snapshot,
            files: compared.files,
            extent: compared.extent,
        })
    }

    pub(crate) async fn remove(
        &self,
        workspace: &Workspace,
        id: SessionId,
    ) -> Result<Kept, Error> {
        let kept = self
            .adapter
            .preserve_and_remove(PreserveAndRemove {
                workspace: self.owned(workspace, id)?,
            })
            .await
            .map_err(failure)?;
        super::trace_removed(&workspace.branch, &kept);
        Ok(kept)
    }
}

fn failure(failure: Failure) -> Error {
    match failure {
        Failure::Workspace(error) => error,
        Failure::Refused(reason) => Error::Failed(reason.label()),
        Failure::Unavailable => {
            Error::Failed(String::from("workspace backend is unavailable"))
        }
    }
}
