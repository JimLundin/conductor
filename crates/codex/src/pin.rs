//! Codex's pinned CLI: which build a platform takes.

use agents::pin::{Package, Pin};

use crate::prelude::*;

pub mod pins;

fn platform(os: &str, arch: &str) -> Option<&'static str> {
    Some(match (os, arch) {
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        _ => return None,
    })
}

/// The pinned download of `codex` for the platform the Rust names `os`
/// and `arch` describe, whose executables end in `exe`. Its musl builds
/// run on either libc, so `musl` does not matter.
#[must_use]
pub fn pin(os: &str, arch: &str, exe: &str, _musl: bool) -> Option<Pin> {
    let platform = platform(os, arch)?;
    Some(Pin {
        version: String::from(pins::VERSION),
        url: format!(
            "https://github.com/openai/codex/releases/download/rust-v{}/codex-package-{platform}.tar.gz",
            pins::VERSION
        ),
        sha256: pins::sha256(platform)?,
        package: Package::Archive,
        binary: format!("bin/codex{exe}"),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use agents::pin::{Package, Pin};
    use serde_json::Value;

    use super::{pin, pins, platform};

    const SUPPORTED: [(&str, &str); 6] = [
        ("linux", "x86_64"),
        ("linux", "aarch64"),
        ("macos", "x86_64"),
        ("macos", "aarch64"),
        ("windows", "x86_64"),
        ("windows", "aarch64"),
    ];

    #[test]
    fn each_supported_platform_has_its_target_triple() {
        assert_eq!(
            SUPPORTED.map(|(os, arch)| platform(os, arch)),
            [
                Some("x86_64-unknown-linux-musl"),
                Some("aarch64-unknown-linux-musl"),
                Some("x86_64-apple-darwin"),
                Some("aarch64-apple-darwin"),
                Some("x86_64-pc-windows-msvc"),
                Some("aarch64-pc-windows-msvc"),
            ]
        );
        assert_eq!(
            [platform("linux", "riscv64"), platform("freebsd", "x86_64")],
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
            .filter_map(|&(os, arch)| platform(os, arch))
            .map(pins::sha256)
            .collect();
        assert_eq!(digests.len(), 6);
        assert!(digests.iter().all(|d| is_digest(d.as_ref())), "{digests:?}");
        let distinct: BTreeSet<&Option<String>> = digests.iter().collect();
        assert_eq!(distinct.len(), digests.len(), "each build its own digest");
        assert_eq!(pins::sha256("riscv64-unknown-linux-gnu"), None);
    }

    #[test]
    fn codex_downloads_its_pinned_build_for_the_platform() {
        assert_eq!(
            [
                pin("linux", "aarch64", "", false),
                pin("freebsd", "x86_64", "", false)
            ],
            [
                Some(Pin {
                    version: String::from(pins::VERSION),
                    url: format!(
                        "https://github.com/openai/codex/releases/download/rust-v{}/codex-package-aarch64-unknown-linux-musl.tar.gz",
                        pins::VERSION
                    ),
                    sha256: pins::sha256("aarch64-unknown-linux-musl")
                        .unwrap_or_default(),
                    package: Package::Archive,
                    binary: String::from("bin/codex"),
                }),
                None,
            ]
        );
    }

    fn recorded() -> Result<BTreeSet<String>, std::io::Error> {
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/codex");
        let mut versions = BTreeSet::new();
        for entry in std::fs::read_dir(dir)? {
            let text = std::fs::read_to_string(entry?.path())?;
            versions.extend(
                text.lines()
                    .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                    .filter_map(|line| {
                        let agent =
                            line.pointer("/result/userAgent")?.as_str()?;
                        let (_, version) =
                            agent.split_whitespace().next()?.split_once('/')?;
                        Some(String::from(version))
                    }),
            );
        }
        Ok(versions)
    }

    #[test]
    fn the_pin_is_recorded() -> Result<(), std::io::Error> {
        assert!(
            recorded()?.contains(pins::VERSION),
            "no fixture in fixtures/codex was recorded at the pinned codex"
        );
        Ok(())
    }
}
