//! What installing any provider's pinned CLI decides.
//!
//! Whether a download is the pinned one, and what an install leaves behind.
//! Each provider pins its own CLI and says which build a platform takes.

use crate::prelude::*;

/// How a download becomes the install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Package {
    /// The download is the binary.
    Binary,
    /// A `.tar.gz` of the whole install directory.
    Archive,
}

/// One CLI's pinned download for a platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub package: Package,
    /// The binary's path in the install directory.
    pub binary: String,
}

/// Whether the entry `name` of a CLI's directory stays when `keep` is the
/// pinned version: the lock and that version do, while older versions and
/// the downloads and unpacks a crash left behind go.
#[must_use]
pub fn kept(name: &str, keep: &str) -> bool {
    name == keep || name == "lock"
}

/// A download whose SHA-256 is not its pin's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub found: String,
    pub expected: String,
}

/// Whether a download with SHA-256 `found` is the pinned build.
///
/// # Errors
/// The download is not the pinned build.
pub fn verify(found: String, expected: &str) -> Result<(), Mismatch> {
    if found == expected {
        Ok(())
    } else {
        Err(Mismatch {
            found,
            expected: String::from(expected),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Mismatch, kept, verify};

    #[test]
    fn an_install_keeps_only_the_lock_and_its_version() {
        assert_eq!(
            ["1.0.0", "lock", "0.9.0", "1.0.0.download", "1.0.0.unpack"]
                .map(|name| kept(name, "1.0.0")),
            [true, true, false, false, false]
        );
    }

    #[test]
    fn only_the_pinned_digest_verifies() {
        assert_eq!(
            [
                verify(String::from("ab"), "ab"),
                verify(String::from("cd"), "ab")
            ],
            [
                Ok(()),
                Err(Mismatch {
                    found: String::from("cd"),
                    expected: String::from("ab"),
                })
            ]
        );
    }
}
