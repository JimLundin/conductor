use std::path::Path;
use std::sync::Arc;

use home::backend::{Backend, Capability, CredentialState, Refusal, Selection};
use project_runtime::{Registration, Registry, VersionControl};

use crate::{ADAPTER, GitWorkspaces};

#[must_use]
pub fn local(data: &Path) -> Arc<dyn VersionControl> {
    Arc::new(GitWorkspaces::new(data.join("worktrees")))
}

fn checkout(
    selection: &Selection,
    directory: &Path,
) -> Result<git2::Repository, Refusal> {
    let invalid = || Refusal::InvalidConfiguration {
        backend: Backend::VersionControl,
    };
    let selected = Path::new(&selection.resource);
    selected.is_absolute().then_some(()).ok_or_else(invalid)?;
    selection
        .options
        .trim()
        .is_empty()
        .then_some(())
        .ok_or_else(invalid)?;
    let selected = selected.canonicalize().map_err(|_| invalid())?;
    let repo = git2::Repository::discover(directory).map_err(|_| invalid())?;
    let observed = repo.workdir().and_then(|root| root.canonicalize().ok());
    if observed.as_deref() != Some(&selected) {
        return Err(invalid());
    }
    Ok(repo)
}

/// # Errors
/// The configured backend cannot perform work in this checkout.
pub fn register(
    registry: &mut Registry,
    adapter: Arc<dyn VersionControl>,
    selection: &Selection,
    directory: &Path,
) -> Result<(), Refusal> {
    if selection.adapter != ADAPTER {
        return Ok(());
    }
    let repo = checkout(selection, directory)?;
    let usable = repo
        .head()
        .and_then(|head| head.peel_to_commit())
        .is_ok_and(|commit| crate::features::check(&repo, commit.id()).is_ok());
    registry.register_version_control(
        Registration {
            selection: selection.clone(),
            credential: selection
                .credential
                .as_ref()
                .map_or(CredentialState::NotRequired, |_| {
                    CredentialState::Missing
                }),
            capabilities: if usable {
                vec![
                    Capability::PrepareWorkspace,
                    Capability::CaptureSnapshot,
                    Capability::InstallSnapshot,
                    Capability::CompareSnapshots,
                    Capability::PreserveWorkspace,
                ]
            } else {
                Vec::new()
            },
        },
        adapter,
    )
}

#[cfg(test)]
mod tests {
    use super::{ADAPTER, local, register};
    use home::backend::{Backend, Backends, Capability, Refusal, Selection};
    use project_runtime::Registry;

    #[test]
    fn local_capabilities_apply_only_to_the_selected_checkout()
    -> anyhow::Result<()> {
        let selected = tempfile::tempdir()?;
        let other = tempfile::tempdir()?;
        let data = tempfile::tempdir()?;
        let repo = git2::Repository::init(selected.path())?;
        let signature =
            git2::Signature::now("Fixture", "fixture@example.test")?;
        let tree = repo.find_tree(repo.index()?.write_tree()?)?;
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "initial",
            &tree,
            &[],
        )?;
        git2::Repository::init(other.path())?;
        let subdirectory = selected.path().join("src");
        std::fs::create_dir(&subdirectory)?;
        let mut registry = Registry::default();
        let selection = Selection {
            adapter: String::from(ADAPTER),
            resource: selected.path().display().to_string(),
            options: String::new(),
            credential: Some(
                String::from("network")
                    .try_into()
                    .map_err(anyhow::Error::msg)?,
            ),
        };
        assert_eq!(
            register(
                &mut registry,
                local(data.path()),
                &selection,
                other.path()
            ),
            Err(Refusal::InvalidConfiguration {
                backend: Backend::VersionControl
            })
        );
        assert_eq!(
            register(
                &mut registry,
                local(data.path()),
                &selection,
                &subdirectory
            ),
            Ok(())
        );
        let project = Backends {
            version_control: Some(selection.clone()),
            ..Backends::default()
        };
        assert_eq!(
            registry.preflight(&project, Capability::PrepareWorkspace),
            Ok(())
        );
        assert_eq!(
            registry.preflight(&project, Capability::InstallSnapshot),
            Ok(())
        );
        assert_eq!(
            registry.preflight(&project, Capability::IntegrateSession),
            Err(Refusal::CapabilityUnavailable {
                capability: Capability::IntegrateSession
            })
        );
        repo.config()?.set_bool("remote.origin.promisor", true)?;
        let mut unsupported = Registry::default();
        assert_eq!(
            register(
                &mut unsupported,
                local(data.path()),
                &selection,
                &subdirectory
            ),
            Ok(())
        );
        assert_eq!(
            unsupported.preflight(&project, Capability::PrepareWorkspace),
            Err(Refusal::CapabilityUnavailable {
                capability: Capability::PrepareWorkspace
            })
        );
        Ok(())
    }
}
