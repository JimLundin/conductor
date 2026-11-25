use alloc::{format, string::String};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AccessError {
    #[error("invalid credential or repository configuration")]
    InvalidConfiguration,
    #[error("host credential is missing")]
    CredentialMissing,
    #[error("host credential has expired")]
    Expired,
    #[error("host credential store is unavailable")]
    CredentialStoreUnavailable,
    #[error("authentication refused")]
    AuthenticationRefused,
    #[error("permission denied")]
    PermissionDenied,
    #[error("repository is unavailable or inaccessible")]
    RepositoryUnavailable,
    #[error("request was rate limited")]
    RateLimited,
    #[error("request failed")]
    RequestFailed,
}

impl AccessError {
    #[must_use]
    pub const fn response(
        status: u16,
        exhausted: bool,
        retry_after: bool,
    ) -> Self {
        match status {
            401 => Self::AuthenticationRefused,
            403 if exhausted || retry_after => Self::RateLimited,
            403 => Self::PermissionDenied,
            404 => Self::RepositoryUnavailable,
            429 => Self::RateLimited,
            _ => Self::RequestFailed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenKind {
    FineGrained,
    Classic,
    #[serde(rename = "oauth")]
    OAuth,
    Installation,
}

impl TokenKind {
    #[must_use]
    pub const fn permits_checks(self, private: Option<bool>) -> bool {
        !matches!(self, Self::FineGrained) || matches!(private, Some(false))
    }
}

pub fn validate_bearer(token: &str) -> Result<(), AccessError> {
    let value = token.trim_end_matches('=');
    if value.is_empty()
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'+' | b'/')
        })
    {
        return Err(AccessError::InvalidConfiguration);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    owner: String,
    name: String,
}

impl Repository {
    pub fn parse(resource: &str) -> Result<Self, AccessError> {
        let (owner, name) = resource
            .split_once('/')
            .ok_or(AccessError::InvalidConfiguration)?;
        if [owner, name].into_iter().any(|part| {
            part.is_empty()
                || matches!(part, "." | "..")
                || !part.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric()
                        || matches!(byte, b'.' | b'-' | b'_')
                })
        }) {
            return Err(AccessError::InvalidConfiguration);
        }
        Ok(Self {
            owner: String::from(owner),
            name: String::from(name),
        })
    }

    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn matches(&self, full_name: &str) -> bool {
        full_name.split_once('/').is_some_and(|(owner, name)| {
            self.owner.eq_ignore_ascii_case(owner)
                && self.name.eq_ignore_ascii_case(name)
        })
    }

    #[must_use]
    pub fn https_remote(&self) -> String {
        format!("https://github.com/{}/{}.git", self.owner, self.name)
    }
}

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Read {
    Issues,
    Statuses,
    Checks,
    Workflows,
}

impl Read {
    #[must_use]
    pub const fn for_backend(
        backend: home::backend::Backend,
    ) -> &'static [Self] {
        match backend {
            home::backend::Backend::IssueTracker => &[Self::Issues],
            home::backend::Backend::RepositoryHosting => {
                &[Self::Statuses, Self::Checks, Self::Workflows]
            }
            home::backend::Backend::VersionControl => &[],
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Issues => "issues",
            Self::Statuses => "commit statuses",
            Self::Checks => "checks",
            Self::Workflows => "workflow runs",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub read: Read,
    pub result: Result<(), AccessError>,
}

#[must_use]
pub fn report(observations: &[Observation]) -> String {
    use core::fmt::Write as _;
    if observations.is_empty() {
        return String::from("no GitHub tracker or hosting service selected\n");
    }
    let mut out = String::new();
    for observation in observations {
        let result = observation.result.map_or_else(
            |err| format!("{err}"),
            |()| String::from("accessible"),
        );
        let _written = writeln!(out, "{}: {result}", observation.read.label());
    }
    out
}

pub mod routes;

#[must_use]
pub const fn read_failure(error: AccessError) -> home::issue::ReadFailure {
    use home::issue::ReadFailure;
    match error {
        AccessError::InvalidConfiguration => ReadFailure::InvalidConfiguration,
        AccessError::CredentialMissing => ReadFailure::CredentialMissing,
        AccessError::Expired => ReadFailure::CredentialExpired,
        AccessError::CredentialStoreUnavailable => {
            ReadFailure::CredentialStoreUnavailable
        }
        AccessError::AuthenticationRefused => {
            ReadFailure::AuthenticationRefused
        }
        AccessError::PermissionDenied => ReadFailure::PermissionDenied,
        AccessError::RateLimited => ReadFailure::RateLimited,
        AccessError::RepositoryUnavailable | AccessError::RequestFailed => {
            ReadFailure::Unavailable
        }
    }
}
