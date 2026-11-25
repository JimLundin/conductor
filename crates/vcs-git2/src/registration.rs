use std::path::{Path, PathBuf};
use std::sync::Arc;

use home::backend::{Backend, Capability, CredentialState, Refusal, Selection};
use project_runtime::{Registration, Registry, VersionControl};

use crate::{ADAPTER, GitWorkspaces};

#[must_use]
pub fn local(data: &Path) -> Arc<dyn VersionControl> {
    Arc::new(GitWorkspaces::new(data.join("worktrees")))
}

fn verify(selection: &Selection, directory: &Path) -> Result<(), Refusal> {
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
    let observed = git2::Repository::discover(directory)
        .ok()
        .and_then(|repo| repo.workdir().map(PathBuf::from))
        .and_then(|root| root.canonicalize().ok());
    if observed.as_deref() != Some(&selected) {
        return Err(invalid());
    }
    Ok(())
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
    verify(selection, directory)?;
    registry.register_version_control(
        Registration {
            selection: selection.clone(),
            credential: selection
                .credential
                .as_ref()
                .map_or(CredentialState::NotRequired, |_| {
                    CredentialState::Missing
                }),
            capabilities: vec![
                Capability::PrepareWorkspace,
                Capability::CaptureSnapshot,
                Capability::InstallSnapshot,
                Capability::CompareSnapshots,
                Capability::PreserveWorkspace,
            ],
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
        git2::Repository::init(selected.path())?;
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
            version_control: Some(selection),
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
        Ok(())
    }
}
