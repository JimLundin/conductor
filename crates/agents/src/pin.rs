//! The decisions of installing a pinned CLI: which build this platform
//! takes, where it goes, whether a download is the pinned one, and what an
//! install leaves behind.

use alloc::format;
use proto::Provider;

use crate::prelude::*;

pub mod pins;

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

/// The pinned download of `cli` for the platform the Rust names `os` and
/// `arch` describe, whose executables end in `exe`.
#[must_use]
pub fn pin(
    cli: Provider,
    os: &str,
    arch: &str,
    exe: &str,
    musl: bool,
) -> Option<Pin> {
    let Platform { claude, codex } = platform(os, arch, musl)?;
    Some(match cli {
        Provider::Claude => Pin {
            version: String::from(pins::CLAUDE_VERSION),
            url: format!(
                "https://downloads.claude.ai/claude-code-releases/{}/{claude}/claude{exe}",
                pins::CLAUDE_VERSION
            ),
            sha256: pins::claude_sha256(claude)?,
            package: Package::Binary,
            binary: format!("claude{exe}"),
        },
        Provider::Codex => Pin {
            version: String::from(pins::CODEX_VERSION),
            url: format!(
                "https://github.com/openai/codex/releases/download/rust-v{}/codex-package-{codex}.tar.gz",
                pins::CODEX_VERSION
            ),
            sha256: pins::codex_sha256(codex)?,
            package: Package::Archive,
            binary: format!("bin/codex{exe}"),
        },
    })
}

/// Whether the entry `name` of a CLI's directory stays when `keep` is the
/// pinned version: the lock and that version do, while older versions and
/// the downloads and unpacks a crash left behind go.
#[must_use]
pub fn kept(name: &str, keep: &str) -> bool {
    name == keep || name == "lock"
}

/// What each vendor calls a platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Platform {
    /// The platform in Claude's `manifest.json`.
    pub claude: &'static str,
    /// Codex's target triple.
    pub codex: &'static str,
}

/// The platforms both CLIs publish builds for, by the `os` and `arch` Rust
/// names.
#[must_use]
pub fn platform(os: &str, arch: &str, musl: bool) -> Option<Platform> {
    let (claude, codex) = match (os, arch, musl) {
        ("linux", "x86_64", true) => {
            ("linux-x64-musl", "x86_64-unknown-linux-musl")
        }
        ("linux", "aarch64", true) => {
            ("linux-arm64-musl", "aarch64-unknown-linux-musl")
        }
        ("linux", "x86_64", false) => {
            ("linux-x64", "x86_64-unknown-linux-musl")
        }
        ("linux", "aarch64", false) => {
            ("linux-arm64", "aarch64-unknown-linux-musl")
        }
        ("macos", "x86_64", _) => ("darwin-x64", "x86_64-apple-darwin"),
        ("macos", "aarch64", _) => ("darwin-arm64", "aarch64-apple-darwin"),
        ("windows", "x86_64", _) => ("win32-x64", "x86_64-pc-windows-msvc"),
        ("windows", "aarch64", _) => ("win32-arm64", "aarch64-pc-windows-msvc"),
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
    use super::{
        Mismatch, Package, Pin, Provider, kept, pin, pins, platform, verify,
    };

    const SUPPORTED: [(&str, &str); 6] = [
        ("linux", "x86_64"),
        ("linux", "aarch64"),
        ("macos", "x86_64"),
        ("macos", "aarch64"),
        ("windows", "x86_64"),
        ("windows", "aarch64"),
    ];

    #[test]
    fn each_supported_platform_has_both_vendors_names() {
        assert_eq!(
            SUPPORTED.map(|(os, arch)| platform(os, arch, false)
                .map(|named| (named.claude, named.codex))),
            [
                Some(("linux-x64", "x86_64-unknown-linux-musl")),
                Some(("linux-arm64", "aarch64-unknown-linux-musl")),
                Some(("darwin-x64", "x86_64-apple-darwin")),
                Some(("darwin-arm64", "aarch64-apple-darwin")),
                Some(("win32-x64", "x86_64-pc-windows-msvc")),
                Some(("win32-arm64", "aarch64-pc-windows-msvc")),
            ]
        );
        assert_eq!(
            [
                platform("linux", "riscv64", false),
                platform("freebsd", "x86_64", false)
            ],
            [None, None]
        );
    }

    fn is_digest(sha256: Option<String>) -> bool {
        sha256.is_some_and(|hex| {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
    }

    #[test]
    fn every_build_the_pins_list_has_its_own_digest() {
        let platforms = SUPPORTED.map(|(os, arch)| platform(os, arch, false));
        let claude = platforms
            .iter()
            .flatten()
            .map(|named| named.claude)
            .chain(
                ["aarch64", "x86_64"]
                    .into_iter()
                    .filter_map(|arch| platform("linux", arch, true))
                    .map(|named| named.claude),
            )
            .map(pins::claude_sha256);
        let codex = platforms
            .iter()
            .flatten()
            .map(|named| pins::codex_sha256(named.codex));
        let digests: Vec<Option<String>> = claude.chain(codex).collect();
        assert_eq!(digests.len(), 14);
        assert!(digests.iter().cloned().all(is_digest), "{digests:?}");
        let mut distinct = digests.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), digests.len(), "each build its own digest");
        assert_eq!(pins::claude_sha256("freebsd-x64"), None);
        assert_eq!(pins::codex_sha256("riscv64-unknown-linux-gnu"), None);
    }

    #[test]
    fn each_cli_downloads_its_pinned_build_for_the_platform() {
        let claude = pin(Provider::Claude, "windows", "x86_64", ".exe", false);
        let codex = pin(Provider::Codex, "linux", "aarch64", "", false);
        assert_eq!(
            [
                claude,
                codex,
                pin(Provider::Codex, "freebsd", "x86_64", "", false)
            ],
            [
                Some(Pin {
                    version: String::from(pins::CLAUDE_VERSION),
                    url: format!(
                        "https://downloads.claude.ai/claude-code-releases/{}/win32-x64/claude.exe",
                        pins::CLAUDE_VERSION
                    ),
                    sha256: pins::claude_sha256("win32-x64")
                        .unwrap_or_default(),
                    package: Package::Binary,
                    binary: String::from("claude.exe"),
                }),
                Some(Pin {
                    version: String::from(pins::CODEX_VERSION),
                    url: format!(
                        "https://github.com/openai/codex/releases/download/rust-v{}/codex-package-aarch64-unknown-linux-musl.tar.gz",
                        pins::CODEX_VERSION
                    ),
                    sha256: pins::codex_sha256("aarch64-unknown-linux-musl")
                        .unwrap_or_default(),
                    package: Package::Archive,
                    binary: String::from("bin/codex"),
                }),
                None,
            ]
        );
    }

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
