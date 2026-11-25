use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub struct CredentialRef(String);

impl CredentialRef {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CredentialRef {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        valid_name(&value)
            .then_some(Self(value))
            .ok_or_else(|| String::from("invalid host credential reference"))
    }
}

impl From<CredentialRef> for String {
    fn from(reference: CredentialRef) -> Self {
        reference.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub adapter: String,
    pub resource: String,
    pub credential: Option<CredentialRef>,
    #[serde(default)]
    pub options: String,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct Backends {
    pub version_control: Option<Selection>,
    pub tracker: Option<Selection>,
    pub hosting: Option<Selection>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum Backend {
    VersionControl,
    IssueTracker,
    RepositoryHosting,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum Capability {
    PrepareWorkspace,
    CaptureSnapshot,
    InstallSnapshot,
    CompareSnapshots,
    IntegrateSession,
    PreserveWorkspace,
    ReadIssues,
    ReadStatuses,
    ReadChecks,
    ReadWorkflows,
}

impl Capability {
    pub const ALL: [Self; 10] = [
        Self::PrepareWorkspace,
        Self::CaptureSnapshot,
        Self::InstallSnapshot,
        Self::CompareSnapshots,
        Self::IntegrateSession,
        Self::PreserveWorkspace,
        Self::ReadIssues,
        Self::ReadStatuses,
        Self::ReadChecks,
        Self::ReadWorkflows,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::PrepareWorkspace => "prepare workspace",
            Self::CaptureSnapshot => "capture snapshot",
            Self::InstallSnapshot => "install snapshot",
            Self::CompareSnapshots => "compare snapshots",
            Self::IntegrateSession => "integrate session",
            Self::PreserveWorkspace => "preserve workspace",
            Self::ReadIssues => "read issues",
            Self::ReadStatuses => "read statuses",
            Self::ReadChecks => "read checks",
            Self::ReadWorkflows => "read workflows",
        }
    }

    #[must_use]
    pub const fn backend(self) -> Backend {
        match self {
            Self::PrepareWorkspace
            | Self::InstallSnapshot
            | Self::CaptureSnapshot
            | Self::CompareSnapshots
            | Self::IntegrateSession
            | Self::PreserveWorkspace => Backend::VersionControl,
            Self::ReadIssues => Backend::IssueTracker,
            Self::ReadStatuses | Self::ReadChecks | Self::ReadWorkflows => {
                Backend::RepositoryHosting
            }
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum CredentialState {
    NotRequired,
    Available,
    Missing,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub backend: Backend,
    pub selection: Selection,
    pub capabilities: Vec<Capability>,
    pub credential: CredentialState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Refusal {
    VersionControlNotConfigured,
    TrackerNotConfigured,
    HostingNotConfigured,
    InvalidConfiguration {
        backend: Backend,
    },
    BackendUnavailable {
        backend: Backend,
        adapter: String,
    },
    DuplicateBackend {
        backend: Backend,
        adapter: String,
    },
    CredentialMissing {
        backend: Backend,
        reference: Option<CredentialRef>,
    },
    CredentialExpired {
        backend: Backend,
        reference: Option<CredentialRef>,
    },
    CapabilityUnavailable {
        capability: Capability,
    },
}

impl Backends {
    #[must_use]
    pub const fn selected(&self, backend: Backend) -> Option<&Selection> {
        match backend {
            Backend::VersionControl => self.version_control.as_ref(),
            Backend::IssueTracker => self.tracker.as_ref(),
            Backend::RepositoryHosting => self.hosting.as_ref(),
        }
    }

    pub fn validate(&self) -> Result<(), Refusal> {
        [
            Backend::VersionControl,
            Backend::IssueTracker,
            Backend::RepositoryHosting,
        ]
        .into_iter()
        .try_for_each(|backend| {
            self.selected(backend)
                .map_or(Ok(()), |selection| selection.validate(backend))
        })
    }
}

impl Selection {
    pub fn validate(&self, backend: Backend) -> Result<(), Refusal> {
        if !valid_name(&self.adapter)
            || self.resource.trim().is_empty()
            || embedded_credentials(&self.resource)
        {
            return Err(Refusal::InvalidConfiguration { backend });
        }
        Ok(())
    }
}

fn embedded_credentials(resource: &str) -> bool {
    resource.split_once("://").is_some_and(|(_, address)| {
        address
            .split('/')
            .next()
            .is_some_and(|authority| authority.contains('@'))
            || address.contains(['?', '#'])
    })
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .split('/')
            .all(|part| !part.is_empty() && !matches!(part, "." | ".."))
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'_' | b'.' | b'/')
        })
}

pub fn preflight<'a>(
    backends: &Backends,
    bindings: &'a [Binding],
    capability: Capability,
) -> Result<&'a Binding, Refusal> {
    backends.validate()?;
    let backend = capability.backend();
    let selection = backends
        .selected(backend)
        .ok_or_else(|| not_configured(backend))?;
    let binding = bindings
        .iter()
        .find(|binding| {
            binding.backend == backend && binding.selection == *selection
        })
        .ok_or_else(|| Refusal::BackendUnavailable {
            backend,
            adapter: selection.adapter.clone(),
        })?;
    if !matches!(
        capability,
        Capability::PrepareWorkspace
            | Capability::CaptureSnapshot
            | Capability::InstallSnapshot
            | Capability::CompareSnapshots
            | Capability::PreserveWorkspace
            | Capability::IntegrateSession
    ) {
        credential(binding)?;
    }
    if !binding.capabilities.contains(&capability) {
        return Err(Refusal::CapabilityUnavailable { capability });
    }
    Ok(binding)
}

pub fn register(
    bindings: &[Binding],
    binding: &Binding,
) -> Result<(), Refusal> {
    binding.selection.validate(binding.backend)?;
    if binding
        .capabilities
        .iter()
        .any(|capability| capability.backend() != binding.backend)
    {
        return Err(Refusal::InvalidConfiguration {
            backend: binding.backend,
        });
    }
    if bindings.iter().any(|other| {
        other.backend == binding.backend && other.selection == binding.selection
    }) {
        return Err(Refusal::DuplicateBackend {
            backend: binding.backend,
            adapter: binding.selection.adapter.clone(),
        });
    }
    Ok(())
}

fn credential(binding: &Binding) -> Result<(), Refusal> {
    let backend = binding.backend;
    let reference = binding.selection.credential.clone();
    match binding.credential {
        CredentialState::NotRequired if reference.is_none() => Ok(()),
        CredentialState::Available if reference.is_some() => Ok(()),
        CredentialState::NotRequired
        | CredentialState::Available
        | CredentialState::Missing => {
            Err(Refusal::CredentialMissing { backend, reference })
        }
        CredentialState::Expired => {
            Err(Refusal::CredentialExpired { backend, reference })
        }
    }
}

const fn not_configured(backend: Backend) -> Refusal {
    match backend {
        Backend::VersionControl => Refusal::VersionControlNotConfigured,
        Backend::IssueTracker => Refusal::TrackerNotConfigured,
        Backend::RepositoryHosting => Refusal::HostingNotConfigured,
    }
}

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Availability {
    pub capability: Capability,
    pub refusal: Option<Refusal>,
}

#[must_use]
pub fn requested(args: &[String]) -> Option<&str> {
    match args {
        [group, command, project]
            if group == "project" && command == "capabilities" =>
        {
            Some(project)
        }
        _ => None,
    }
}

#[must_use]
pub fn report(
    name: &str,
    capabilities: &[Availability],
    tracker_error: Option<&str>,
) -> String {
    let mut text = format!("{name}\n");
    for available in capabilities {
        let failure = tracker_error
            .filter(|_| available.capability == Capability::ReadIssues);
        let outcome = failure.map_or_else(
            || {
                available
                    .refusal
                    .as_ref()
                    .map_or_else(|| String::from("available"), Refusal::label)
            },
            String::from,
        );
        let _written =
            writeln!(text, "  {}: {outcome}", available.capability.label());
    }
    text
}

impl Refusal {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::VersionControlNotConfigured => {
                String::from("version control not configured")
            }
            Self::TrackerNotConfigured => {
                String::from("tracker not configured")
            }
            Self::HostingNotConfigured => {
                String::from("hosting not configured")
            }
            Self::InvalidConfiguration { .. } => {
                String::from("invalid backend configuration")
            }
            Self::BackendUnavailable { adapter, .. } => {
                format!("adapter {adapter:?} is unavailable")
            }
            Self::DuplicateBackend { adapter, .. } => {
                format!("adapter {adapter:?} is already registered")
            }
            Self::CredentialMissing { .. } => {
                String::from("host credential is missing")
            }
            Self::CredentialExpired { .. } => {
                String::from("host credential has expired")
            }
            Self::CapabilityUnavailable { capability } => {
                format!("{} is unavailable", capability.label())
            }
        }
    }
}

#[must_use]
pub fn access_requested(args: &[String]) -> Option<(&str, Option<&str>)> {
    let [group, command, project, extra @ ..] = args else {
        return None;
    };
    if group != "project" || command != "access" {
        return None;
    }
    match extra {
        [] => Some((project, None)),
        [dir] => Some((project, Some(dir))),
        _ => None,
    }
}

#[must_use]
pub fn configuration_query(args: &[String]) -> bool {
    requested(args).is_some()
        || access_requested(args).is_some()
        || issue_query(args).is_some()
}

mod query;
pub use query::{IssueQuery, issue_query};
