//! The Claude CLI versions this crate supports. The CLI reports its version in
//! the `initialize` response ([`InitializeResponse`](crate::InitializeResponse)).

use std::fmt;
use std::ops::RangeInclusive;

/// A `major.minor.patch` CLI version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CliVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl CliVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.split('.').map(|p| p.parse().ok());
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(Some(major)), Some(Some(minor)), Some(Some(patch)), None) => {
                Some(Self::new(major, minor, patch))
            }
            _ => None,
        }
    }
}

impl fmt::Display for CliVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// From the version Conductor's live checklist (`s1`) last passed against to
/// the newest version these types were checked against. Widen it only after
/// running `s1` against the new CLI.
pub const SUPPORTED_CLI_VERSIONS: RangeInclusive<CliVersion> =
    CliVersion::new(2, 1, 316)..=CliVersion::new(2, 1, 322);

/// A reported CLI version outside [`SUPPORTED_CLI_VERSIONS`], or one that
/// does not parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "Claude CLI {found} is not supported (supported: {})",
    supported_cli_versions()
)]
pub struct UnsupportedCliVersion {
    pub found: String,
}

/// [`SUPPORTED_CLI_VERSIONS`] as text, e.g. `2.1.316 to 2.1.322`.
pub fn supported_cli_versions() -> String {
    format!(
        "{} to {}",
        SUPPORTED_CLI_VERSIONS.start(),
        SUPPORTED_CLI_VERSIONS.end()
    )
}

pub fn check_cli_version(version: &str) -> Result<CliVersion, UnsupportedCliVersion> {
    CliVersion::parse(version)
        .filter(|v| SUPPORTED_CLI_VERSIONS.contains(v))
        .ok_or_else(|| UnsupportedCliVersion {
            found: version.to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_range_is_inclusive_at_both_ends() {
        let check = |v| check_cli_version(v).map(|v| v.to_string());
        assert_eq!(check("2.1.316"), Ok("2.1.316".into()));
        assert_eq!(check("2.1.322"), Ok("2.1.322".into()));
        for bad in [
            "2.1.315",
            "2.1.323",
            "3.0.0",
            "2.1",
            "2.1.316.1",
            "2.1.x",
            "",
        ] {
            assert_eq!(
                check(bad),
                Err(UnsupportedCliVersion { found: bad.into() }),
                "{bad}"
            );
        }
    }

    #[test]
    fn error_names_both_versions() {
        assert_eq!(
            check_cli_version("2.2.0").unwrap_err().to_string(),
            "Claude CLI 2.2.0 is not supported (supported: 2.1.316 to 2.1.322)"
        );
    }
}
