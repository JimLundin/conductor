mod comments;
pub mod issues;
pub mod operations;
mod relations;
mod storage;
pub mod transport;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use home::ProjectId;
use home::backend::{
    self, Backend, Backends, Binding, Capability, CredentialState, Refusal,
    Selection,
};
use home::hosting;
use home::issue::{
    Comment, CommentWrite, IssueComment, ListPage, Lookup, Observation, Page,
    ReadFailure, Relating, RelationChange, RelationStep, SendFailure,
    Unrelatable,
};
use vcs::command::{
    Capture, Compare, Comparison, Install, Integrate, Prepare, Prepared,
    PreserveAndRemove, Revision,
};

pub type Completion<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, Failure>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Refused(Refusal),
    Workspace(vcs::Error),
    Unavailable,
}

impl From<Refusal> for Failure {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

pub trait VersionControl: Send + Sync {
    fn prepare(&self, request: Prepare) -> Completion<'_, Prepared>;
    fn capture(&self, request: Capture) -> Completion<'_, Revision>;
    fn install(&self, request: Install) -> Completion<'_, ()>;
    fn compare(&self, request: Compare) -> Completion<'_, Comparison>;
    fn integrate(&self, request: Integrate) -> Completion<'_, u64>;
    fn preserve_and_remove(
        &self,
        request: PreserveAndRemove,
    ) -> Completion<'_, vcs::Kept>;
}

pub type IssueCompletion<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ReadFailure>> + Send + 'a>>;

pub type SendCompletion<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, SendFailure>> + Send + 'a>>;

pub type RelationCompletion<'a> =
    Pin<Box<dyn Future<Output = Result<Relating, Unrelatable>> + Send + 'a>>;

pub use comments::WriteError;

pub trait IssueTracker: Send + Sync {
    fn read(&self, request: Lookup) -> IssueCompletion<'_, Observation>;
    fn list(&self, request: ListPage) -> IssueCompletion<'_, Page>;
    fn comments(&self, issue: Lookup) -> IssueCompletion<'_, Vec<Comment>>;
    fn canvas_comments(
        &self,
        project: ProjectId,
    ) -> IssueCompletion<'_, Vec<IssueComment>>;
    fn write_comment(
        &self,
        issue: Lookup,
        write: CommentWrite,
    ) -> SendCompletion<'_, Comment>;
    fn plan_relation(
        &self,
        issue: Lookup,
        change: RelationChange,
    ) -> RelationCompletion<'_>;
    fn write_relation(
        &self,
        issue: Lookup,
        step: RelationStep,
    ) -> SendCompletion<'_, ()>;
}

pub trait RepositoryHosting: Send + Sync {
    fn observe(
        &self,
        request: hosting::Request,
    ) -> Completion<'_, hosting::Report>;
}

pub struct Registration {
    pub selection: Selection,
    pub capabilities: Vec<Capability>,
    pub credential: CredentialState,
}

impl Registration {
    fn binding(self, backend: Backend) -> Binding {
        Binding {
            backend,
            selection: self.selection,
            capabilities: self.capabilities,
            credential: self.credential,
        }
    }
}

struct Entry<T: ?Sized> {
    binding: Binding,
    adapter: Arc<T>,
}

#[derive(Default)]
pub struct Registry {
    version_control: Vec<Entry<dyn VersionControl>>,
    trackers: Vec<Entry<dyn IssueTracker>>,
    hosting: Vec<Entry<dyn RepositoryHosting>>,
}

impl Registry {
    pub fn register_version_control(
        &mut self,
        registration: Registration,
        adapter: Arc<dyn VersionControl>,
    ) -> Result<(), Refusal> {
        let binding = registration.binding(Backend::VersionControl);
        backend::register(&self.bindings(), &binding)?;
        self.version_control.push(Entry { binding, adapter });
        Ok(())
    }

    pub fn register_tracker(
        &mut self,
        registration: Registration,
        adapter: Arc<dyn IssueTracker>,
    ) -> Result<(), Refusal> {
        let binding = registration.binding(Backend::IssueTracker);
        backend::register(&self.bindings(), &binding)?;
        self.trackers.push(Entry { binding, adapter });
        Ok(())
    }

    pub fn register_hosting(
        &mut self,
        registration: Registration,
        adapter: Arc<dyn RepositoryHosting>,
    ) -> Result<(), Refusal> {
        let binding = registration.binding(Backend::RepositoryHosting);
        backend::register(&self.bindings(), &binding)?;
        self.hosting.push(Entry { binding, adapter });
        Ok(())
    }

    #[must_use]
    pub fn bindings(&self) -> Vec<Binding> {
        self.version_control
            .iter()
            .map(|entry| &entry.binding)
            .chain(self.trackers.iter().map(|entry| &entry.binding))
            .chain(self.hosting.iter().map(|entry| &entry.binding))
            .cloned()
            .collect()
    }

    pub fn preflight(
        &self,
        backends: &Backends,
        capability: Capability,
    ) -> Result<(), Refusal> {
        backend::preflight(backends, &self.bindings(), capability).map(|_| ())
    }

    #[must_use]
    pub fn capabilities(
        &self,
        backends: &Backends,
    ) -> Vec<backend::Availability> {
        Capability::ALL
            .into_iter()
            .map(|capability| backend::Availability {
                capability,
                refusal: self.preflight(backends, capability).err(),
            })
            .collect()
    }

    #[must_use]
    pub const fn project<'a>(&'a self, backends: &'a Backends) -> Project<'a> {
        Project {
            registry: self,
            backends,
        }
    }
}

pub struct Project<'a> {
    registry: &'a Registry,
    backends: &'a Backends,
}

impl Project<'_> {
    pub async fn prepare(&self, request: Prepare) -> Result<Prepared, Failure> {
        self.version_control(Capability::PrepareWorkspace)?
            .prepare(request)
            .await
    }

    pub async fn capture(&self, request: Capture) -> Result<Revision, Failure> {
        self.version_control(Capability::CaptureSnapshot)?
            .capture(request)
            .await
    }

    pub async fn install(&self, request: Install) -> Result<(), Failure> {
        self.version_control(Capability::InstallSnapshot)?
            .install(request)
            .await
    }

    pub async fn compare(
        &self,
        request: Compare,
    ) -> Result<Comparison, Failure> {
        self.version_control(Capability::CompareSnapshots)?
            .compare(request)
            .await
    }

    pub async fn integrate(&self, request: Integrate) -> Result<u64, Failure> {
        self.version_control(Capability::IntegrateSession)?
            .integrate(request)
            .await
    }

    pub async fn preserve_and_remove(
        &self,
        request: PreserveAndRemove,
    ) -> Result<vcs::Kept, Failure> {
        self.version_control(Capability::PreserveWorkspace)?
            .preserve_and_remove(request)
            .await
    }

    pub async fn read_issue(
        &self,
        request: Lookup,
    ) -> Result<Observation, ReadFailure> {
        let observed = self.tracker()?.read(request.clone()).await?;
        home::issue::cache::verify(&request, &observed)?;
        Ok(observed)
    }

    fn tracker(&self) -> Result<&dyn IssueTracker, ReadFailure> {
        self.registry
            .preflight(self.backends, Capability::ReadIssues)
            .map_err(ReadFailure::Refused)?;
        selected(&self.registry.trackers, self.backends.tracker.as_ref())
            .map_err(|_| ReadFailure::Unavailable)
    }

    pub async fn observe_hosting(
        &self,
        request: hosting::Request,
    ) -> Result<hosting::Report, Failure> {
        self.registry
            .preflight(self.backends, request.source.capability())?;
        selected(&self.registry.hosting, self.backends.hosting.as_ref())?
            .observe(request)
            .await
    }

    fn version_control(
        &self,
        capability: Capability,
    ) -> Result<&dyn VersionControl, Failure> {
        self.registry.preflight(self.backends, capability)?;
        selected(
            &self.registry.version_control,
            self.backends.version_control.as_ref(),
        )
    }
}

fn selected<'a, T: ?Sized>(
    entries: &'a [Entry<T>],
    selection: Option<&Selection>,
) -> Result<&'a T, Failure> {
    entries
        .iter()
        .find(|entry| Some(&entry.binding.selection) == selection)
        .map(|entry| entry.adapter.as_ref())
        .ok_or(Failure::Unavailable)
}

#[cfg(test)]
mod tests;

pub fn data_dir_or_default(
    data: Option<std::path::PathBuf>,
) -> std::io::Result<std::path::PathBuf> {
    data.map_or_else(
        || {
            directories::ProjectDirs::from("", "", "conductor")
                .map(|dirs| dirs.data_dir().to_owned())
                .ok_or_else(|| {
                    std::io::Error::other(
                        "no home directory to put the data dir in",
                    )
                })
        },
        Ok,
    )
}
