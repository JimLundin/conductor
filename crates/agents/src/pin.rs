//! The decisions of installing a pinned CLI: which build this platform
//! takes, and whether a download is the pinned one.

use crate::prelude::*;

/// What each vendor calls a platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    /// The platform in Claude's `manifest.json`. Linux means glibc.
    pub claude: &'static str,
    /// Codex's target triple.
    pub codex: &'static str,
}

/// The platforms both CLIs publish builds for, by the `os` and `arch` Rust
/// names.
#[must_use]
pub fn platform(os: &str, arch: &str) -> Option<Platform> {
    let (claude, codex) = match (os, arch) {
        ("linux", "x86_64") => ("linux-x64", "x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => ("linux-arm64", "aarch64-unknown-linux-musl"),
        ("macos", "x86_64") => ("darwin-x64", "x86_64-apple-darwin"),
        ("macos", "aarch64") => ("darwin-arm64", "aarch64-apple-darwin"),
        ("windows", "x86_64") => ("win32-x64", "x86_64-pc-windows-msvc"),
        ("windows", "aarch64") => ("win32-arm64", "aarch64-pc-windows-msvc"),
        _ => return None,
    };
    Some(Platform { claude, codex })
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
    use super::{Mismatch, Platform, platform, verify};

    #[test]
    fn each_supported_platform_has_both_vendors_names() {
        assert_eq!(
            [
                platform("linux", "aarch64"),
                platform("windows", "x86_64"),
                platform("linux", "riscv64"),
                platform("freebsd", "x86_64"),
            ],
            [
                Some(Platform {
                    claude: "linux-arm64",
                    codex: "aarch64-unknown-linux-musl",
                }),
                Some(Platform {
                    claude: "win32-x64",
                    codex: "x86_64-pc-windows-msvc",
                }),
                None,
                None,
            ]
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
