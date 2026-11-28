use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use home::backend::{
    Backend, Backends, Capability, CredentialRef, CredentialState, Refusal,
    Selection,
};
use home::hosting;
use home::issue::{
    ListPage, Lookup, Observation, Page, ReadFailure, Unreadable,
};
use vcs::command::{
    Capture, Compare, Comparison, Install, Integrate, OwnedWorkspace, Prepare,
    Prepared, PreserveAndRemove, Revision, SnapshotAttempt, WorkspaceId,
};

use super::{
    Completion, Failure, IssueCompletion, IssueTracker, Registration, Registry,
    RepositoryHosting, VersionControl,
};

fn selection(name: &str) -> Selection {
    Selection {
        adapter: String::from(name),
        resource: String::from("local-project"),
        credential: None,
        options: String::new(),
    }
}

fn registration(
    selection: Selection,
    capabilities: &[Capability],
) -> Registration {
    Registration {
        selection,
        capabilities: capabilities.to_vec(),
        credential: CredentialState::NotRequired,
    }
}

fn workspace() -> Prepared {
    Prepared {
        repository: vcs::Repository(String::from("/repos/local")),
        original: vcs::WorkingCopy(String::from("/repos/local/src")),
        root: String::from("/owned/session"),
        directory: String::from("/owned/session/src"),
        branch: String::from("session/work"),
    }
}

fn issue() -> Result<Lookup, String> {
    Ok(Lookup {
        project: home::ProjectId::new(1),
        id: home::IssueId::new("42").ok_or("invalid test issue")?,
    })
}

struct Scripted {
    name: &'static str,
    calls: Sender<()>,
}

fn scripted(name: &'static str) -> (Arc<Scripted>, Receiver<()>) {
    let (calls, received) = mpsc::channel();
    (Arc::new(Scripted { name, calls }), received)
}

impl Scripted {
    fn reply<T: Send + 'static>(&self, value: T) -> Completion<'_, T> {
        let _sent = self.calls.send(());
        Box::pin(std::future::ready(Ok(value)))
    }
}

impl VersionControl for Scripted {
    fn prepare(&self, _request: Prepare) -> Completion<'_, Prepared> {
        self.reply(workspace())
    }

    fn capture(&self, _request: Capture) -> Completion<'_, Revision> {
        self.reply(Revision(String::from(self.name)))
    }

    fn install(&self, _request: Install) -> Completion<'_, ()> {
        self.reply(())
    }

    fn compare(&self, _request: Compare) -> Completion<'_, Comparison> {
        self.reply(Comparison {
            files: vec![vcs::SnapshotFile {
                path: String::from("README.md"),
                change: vcs::DiffChange::Edit,
                patch: vcs::FilePatch::Text(String::from("+new work")),
            }],
            extent: vcs::DiffExtent::Complete,
        })
    }

    fn integrate(&self, _request: Integrate) -> Completion<'_, u64> {
        self.reply(2)
    }

    fn preserve_and_remove(
        &self,
        _request: PreserveAndRemove,
    ) -> Completion<'_, vcs::Kept> {
        self.reply(vcs::Kept::Local)
    }
}

impl IssueTracker for Scripted {
    fn list(&self, _request: ListPage) -> IssueCompletion<'_, Page> {
        Box::pin(async { Err(ReadFailure::Unavailable) })
    }

    fn read(&self, request: Lookup) -> IssueCompletion<'_, Observation> {
        let _sent = self.calls.send(());
        Box::pin(async move {
            Ok(Observation::invalid(Unreadable {
                id: request.id,
                project: request.project,
                title: Some(String::from(self.name)),
                body: None,
                created: None,
                reason: String::from("invalid managed label"),
            }))
        })
    }
    fn comments(
        &self,
        _: home::issue::Lookup,
    ) -> crate::IssueCompletion<'_, Vec<home::issue::Comment>> {
        Box::pin(async { Err(home::issue::ReadFailure::Unavailable) })
    }
    fn write_comment(
        &self,
        _: home::issue::Lookup,
        _: home::issue::CommentWrite,
    ) -> crate::SendCompletion<'_, home::issue::Comment> {
        Box::pin(async { Err(home::issue::SendFailure::Unknown) })
    }
}

impl RepositoryHosting for Scripted {
    fn observe(
        &self,
        request: hosting::Request,
    ) -> Completion<'_, hosting::Report> {
        self.reply(hosting::Report {
            revision: request.revision,
            source: request.source,
            checks: vec![hosting::Check {
                name: String::from(self.name),
                state: hosting::State::Passed,
            }],
        })
    }
}

#[tokio::test]
async fn issue_work_without_a_tracker_is_refused_before_dispatch()
-> Result<(), String> {
    let registry = Registry::default();
    let backends = Backends::default();
    assert_eq!(
        registry.preflight(&backends, Capability::ReadIssues),
        Err(Refusal::TrackerNotConfigured)
    );
    assert_eq!(
        registry.project(&backends).read_issue(issue()?).await,
        Err(ReadFailure::Refused(Refusal::TrackerNotConfigured))
    );
    Ok(())
}

#[tokio::test]
async fn missing_credentials_refuse_issue_work_without_contacting_the_tracker()
-> Result<(), String> {
    let mut registry = Registry::default();
    let selected = Selection {
        credential: Some(CredentialRef::try_from(String::from("host/issues"))?),
        ..selection("remote-issues")
    };
    let mut registering =
        registration(selected.clone(), &[Capability::ReadIssues]);
    registering.credential = CredentialState::Missing;
    let (tracker, calls) = scripted("remote issue");
    registry
        .register_tracker(registering, tracker.clone())
        .map_err(|error| format!("{error:?}"))?;
    let backends = Backends {
        tracker: Some(selected.clone()),
        ..Backends::default()
    };
    assert_eq!(
        registry.project(&backends).read_issue(issue()?).await,
        Err(ReadFailure::Refused(Refusal::CredentialMissing {
            backend: Backend::IssueTracker,
            reference: selected.credential
        }))
    );
    assert_eq!(calls.try_iter().count(), 0);
    Ok(())
}

#[tokio::test]
async fn statuses_do_not_promise_access_to_required_checks()
-> Result<(), String> {
    let mut registry = Registry::default();
    let selected = selection("queue-host");
    let (adapter, calls) = scripted("queue");
    registry
        .register_hosting(
            registration(selected.clone(), &[Capability::ReadStatuses]),
            adapter.clone(),
        )
        .map_err(|error| format!("{error:?}"))?;
    let backends = Backends {
        hosting: Some(selected),
        ..Backends::default()
    };
    let project = registry.project(&backends);
    let checks = hosting::Request {
        revision: String::from("reviewed-revision"),
        source: hosting::Source::Checks,
    };
    assert_eq!(
        project.observe_hosting(checks).await,
        Err(Failure::Refused(Refusal::CapabilityUnavailable {
            capability: Capability::ReadChecks
        }))
    );
    assert_eq!(calls.try_iter().count(), 0);
    let statuses = project
        .observe_hosting(hosting::Request {
            revision: String::from("reviewed-revision"),
            source: hosting::Source::Statuses,
        })
        .await
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(statuses.revision, "reviewed-revision");
    assert_eq!(statuses.checks[0].state, hosting::State::Passed);
    assert_eq!(calls.try_iter().count(), 1);
    Ok(())
}

#[tokio::test]
async fn local_workspaces_snapshots_and_diffs_need_no_tracker_or_hosting()
-> Result<(), String> {
    let mut registry = Registry::default();
    let selected = selection("local-vcs");
    let (adapter, calls) = scripted("snapshot-revision");
    registry
        .register_version_control(
            registration(
                selected.clone(),
                &[
                    Capability::PrepareWorkspace,
                    Capability::CaptureSnapshot,
                    Capability::InstallSnapshot,
                    Capability::CompareSnapshots,
                    Capability::IntegrateSession,
                    Capability::PreserveWorkspace,
                ],
            ),
            adapter.clone(),
        )
        .map_err(|error| format!("{error:?}"))?;
    let backends = Backends {
        version_control: Some(selected),
        ..Backends::default()
    };
    let project = registry.project(&backends);
    let prepared = project
        .prepare(Prepare {
            directory: String::from("/repos/local/src"),
            name: WorkspaceId(String::from("session")),
            base: None,
        })
        .await
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(prepared, workspace());
    let owned = OwnedWorkspace {
        root: prepared.root,
        id: WorkspaceId(String::from("session")),
        directory: prepared.directory,
    };
    let attempt = SnapshotAttempt(String::from("turn-2"));
    assert_eq!(
        project
            .capture(Capture {
                workspace: owned.clone(),
                attempt: attempt.clone()
            })
            .await,
        Ok(Revision(String::from("snapshot-revision")))
    );
    assert_eq!(
        project
            .install(Install {
                workspace: owned.clone(),
                revision: Revision(String::from("snapshot-revision")),
                attempt
            })
            .await,
        Ok(())
    );
    let diff = project
        .compare(Compare {
            workspace: owned.clone(),
            revision: Revision(String::from("snapshot-revision")),
            baseline: Revision(String::from("initial-revision")),
        })
        .await
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(
        diff.files[0].patch,
        vcs::FilePatch::Text(String::from("+new work"))
    );
    assert_eq!(
        project
            .integrate(Integrate {
                workspace: owned.clone(),
                target: String::from("issue/target"),
                attempt: vcs::command::IntegrationId(String::from("attempt-1"))
            })
            .await,
        Ok(2)
    );
    assert_eq!(
        project
            .preserve_and_remove(PreserveAndRemove { workspace: owned })
            .await,
        Ok(vcs::Kept::Local)
    );
    assert_eq!(calls.try_iter().count(), 6);
    Ok(())
}

#[tokio::test]
async fn binding_one_resource_does_not_authorize_another_resource()
-> Result<(), String> {
    let mut registry = Registry::default();
    let selected = selection("issues");
    let (adapter, calls) = scripted("first issue");
    registry
        .register_tracker(
            registration(selected.clone(), &[Capability::ReadIssues]),
            adapter.clone(),
        )
        .map_err(|error| format!("{error:?}"))?;
    let other = Selection {
        resource: String::from("another-project"),
        ..selected
    };
    let backends = Backends {
        tracker: Some(other),
        ..Backends::default()
    };
    assert_eq!(
        registry.project(&backends).read_issue(issue()?).await,
        Err(ReadFailure::Refused(Refusal::BackendUnavailable {
            backend: Backend::IssueTracker,
            adapter: String::from("issues")
        }))
    );
    assert_eq!(calls.try_iter().count(), 0);
    Ok(())
}

#[tokio::test]
async fn selecting_a_tracker_does_not_require_a_repository_or_hosting_service()
-> Result<(), String> {
    let mut registry = Registry::default();
    let selected = selection("remote-issues");
    let (adapter, _calls) = scripted("remote issue");
    registry
        .register_tracker(
            registration(selected.clone(), &[Capability::ReadIssues]),
            adapter,
        )
        .map_err(|error| format!("{error:?}"))?;
    let backends = Backends {
        tracker: Some(selected),
        ..Backends::default()
    };
    let observed = registry
        .project(&backends)
        .read_issue(issue()?)
        .await
        .map_err(|error| format!("{error:?}"))?;
    assert!(
        matches!(observed.state, home::issue::IssueState::Invalid(record) if record.title.as_deref() == Some("remote issue"))
    );
    Ok(())
}

#[test]
fn a_registered_backend_cannot_claim_another_backends_capabilities() {
    let mut registry = Registry::default();
    let (adapter, _calls) = scripted("local");
    assert_eq!(
        registry.register_version_control(
            registration(selection("local-vcs"), &[Capability::ReadIssues]),
            adapter
        ),
        Err(Refusal::InvalidConfiguration {
            backend: Backend::VersionControl
        })
    );
    assert_eq!(registry.bindings(), Vec::new());
}

#[test]
fn a_binding_is_registered_once() -> Result<(), String> {
    let mut registry = Registry::default();
    let selected = selection("issues");
    let (adapter, _calls) = scripted("local");
    registry
        .register_tracker(
            registration(selected.clone(), &[Capability::ReadIssues]),
            adapter.clone(),
        )
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(
        registry.register_tracker(
            registration(selected, &[Capability::ReadIssues]),
            adapter
        ),
        Err(Refusal::DuplicateBackend {
            backend: Backend::IssueTracker,
            adapter: String::from("issues")
        })
    );
    assert_eq!(registry.bindings().len(), 1);
    Ok(())
}

#[test]
fn changed_configuration_requires_a_new_binding() -> Result<(), String> {
    let mut registry = Registry::default();
    let selected = selection("issues");
    registry
        .register_tracker(
            registration(selected.clone(), &[Capability::ReadIssues]),
            scripted("local").0,
        )
        .map_err(|error| format!("{error:?}"))?;
    let backends = Backends {
        tracker: Some(Selection {
            options: String::from("changed = true"),
            ..selected
        }),
        ..Backends::default()
    };
    assert_eq!(
        registry.preflight(&backends, Capability::ReadIssues),
        Err(Refusal::BackendUnavailable {
            backend: Backend::IssueTracker,
            adapter: String::from("issues")
        })
    );
    Ok(())
}
