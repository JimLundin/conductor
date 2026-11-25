//! Claude's pinned CLI: which build a platform takes.

use agents::pin::{Package, Pin};

use crate::prelude::*;

pub mod pins;

fn platform(os: &str, arch: &str, musl: bool) -> Option<&'static str> {
    Some(match (os, arch, musl) {
        ("linux", "x86_64", true) => "linux-x64-musl",
        ("linux", "aarch64", true) => "linux-arm64-musl",
        ("linux", "x86_64", false) => "linux-x64",
        ("linux", "aarch64", false) => "linux-arm64",
        ("macos", "x86_64", _) => "darwin-x64",
        ("macos", "aarch64", _) => "darwin-arm64",
        ("windows", "x86_64", _) => "win32-x64",
        ("windows", "aarch64", _) => "win32-arm64",
        _ => return None,
    })
}

/// The pinned download of `claude` for the platform the Rust names `os`
/// and `arch` describe, whose executables end in `exe`.
#[must_use]
pub fn pin(os: &str, arch: &str, exe: &str, musl: bool) -> Option<Pin> {
    let platform = platform(os, arch, musl)?;
    Some(Pin {
        version: String::from(pins::VERSION),
        url: format!(
            "https://downloads.claude.ai/claude-code-releases/{}/{platform}/claude{exe}",
            pins::VERSION
        ),
        sha256: pins::sha256(platform)?,
        package: Package::Binary,
        binary: format!("claude{exe}"),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use agents::pin::{Package, Pin};
    use serde_json::Value;

    use super::{pin, pins, platform};
    use crate::check_version;

    const SUPPORTED: [(&str, &str, bool); 8] = [
        ("linux", "x86_64", false),
        ("linux", "aarch64", false),
        ("linux", "x86_64", true),
        ("linux", "aarch64", true),
        ("macos", "x86_64", false),
        ("macos", "aarch64", false),
        ("windows", "x86_64", false),
        ("windows", "aarch64", false),
    ];

    #[test]
    fn each_supported_platform_has_its_manifest_name() {
        assert_eq!(
            SUPPORTED.map(|(os, arch, musl)| platform(os, arch, musl)),
            [
                Some("linux-x64"),
                Some("linux-arm64"),
                Some("linux-x64-musl"),
                Some("linux-arm64-musl"),
                Some("darwin-x64"),
                Some("darwin-arm64"),
                Some("win32-x64"),
                Some("win32-arm64"),
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

    fn is_digest(sha256: Option<&String>) -> bool {
        sha256.is_some_and(|hex| {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
    }

    #[test]
    fn every_supported_build_has_its_own_digest() {
        let digests: Vec<Option<String>> = SUPPORTED
            .iter()
            .filter_map(|&(os, arch, musl)| platform(os, arch, musl))
            .map(pins::sha256)
            .collect();
        assert_eq!(digests.len(), 8);
        assert!(digests.iter().all(|d| is_digest(d.as_ref())), "{digests:?}");
        let distinct: BTreeSet<&Option<String>> = digests.iter().collect();
        assert_eq!(distinct.len(), digests.len(), "each build its own digest");
        assert_eq!(pins::sha256("freebsd-x64"), None);
    }

    #[test]
    fn claude_downloads_its_pinned_build_for_the_platform() {
        assert_eq!(
            [
                pin("windows", "x86_64", ".exe", false),
                pin("freebsd", "x86_64", "", false)
            ],
            [
                Some(Pin {
                    version: String::from(pins::VERSION),
                    url: format!(
                        "https://downloads.claude.ai/claude-code-releases/{}/win32-x64/claude.exe",
                        pins::VERSION
                    ),
                    sha256: pins::sha256("win32-x64").unwrap_or_default(),
                    package: Package::Binary,
                    binary: String::from("claude.exe"),
                }),
                None,
            ]
        );
    }

    fn recorded() -> Result<BTreeSet<String>, std::io::Error> {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/claude");
        let mut versions = BTreeSet::new();
        for entry in std::fs::read_dir(dir)? {
            let text = std::fs::read_to_string(entry?.path())?;
            versions.extend(
                text.lines()
                    .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                    .filter_map(|line| {
                        line.pointer("/response/response/claude_code_version")
                            .and_then(Value::as_str)
                            .map(String::from)
                    }),
            );
        }
        Ok(versions)
    }

    #[test]
    fn the_pin_is_recorded_and_supported()
    -> Result<(), Box<dyn std::error::Error>> {
        let version = String::from(pins::VERSION);
        assert!(
            recorded()?.contains(&version),
            "no fixture in fixtures/claude was recorded at the pinned claude"
        );
        let initialize = serde_json::from_str(&format!(
            "{{\"claude_code_version\": \"{version}\"}}"
        ))?;
        assert_eq!(
            check_version(initialize),
            Ok(()),
            "the pinned claude is outside SUPPORTED_VERSIONS"
        );
        Ok(())
    }
}
