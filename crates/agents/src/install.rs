//! Installs the pinned CLIs (`pins.toml`, docs/architecture.md §6.2). Each
//! one lands in `<data>/cli/<cli>/<version>/` by a download to a temporary
//! file, a SHA-256 check against the pin and a rename, so a crash leaves
//! either nothing or a verified install.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::live::{Presence, presence};
use crate::pins;

/// A download that sends nothing for this long has stalled.
const STALL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cli {
    Claude,
    Codex,
}

impl Cli {
    pub fn name(&self) -> &str {
        match self {
            Cli::Claude => "claude",
            Cli::Codex => "codex",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("no pinned build for {os} on {arch}")]
    Platform { os: String, arch: String },
    #[error("downloading the pinned CLI: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{path}: {error}")]
    Io { path: String, error: std::io::Error },
    #[error("{url} has SHA-256 {found}, but the pin is {expected}")]
    Mismatch {
        url: String,
        found: String,
        expected: String,
    },
    #[error("tar could not unpack {archive}: {status}")]
    Unpack { archive: String, status: String },
}

/// The platforms both CLIs publish builds for. Linux means glibc.
#[derive(Debug, Clone, Copy)]
enum Platform {
    LinuxX64,
    LinuxArm64,
    MacX64,
    MacArm64,
    WindowsX64,
    WindowsArm64,
}

/// What each vendor calls a platform.
struct Names {
    /// The platform in Claude's `manifest.json`.
    claude: String,
    /// Codex's target triple.
    codex: String,
    /// The suffix of an executable's file name.
    exe: String,
}

impl Platform {
    fn current() -> Result<Platform, InstallError> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => Ok(Platform::LinuxX64),
            ("linux", "aarch64") => Ok(Platform::LinuxArm64),
            ("macos", "x86_64") => Ok(Platform::MacX64),
            ("macos", "aarch64") => Ok(Platform::MacArm64),
            ("windows", "x86_64") => Ok(Platform::WindowsX64),
            ("windows", "aarch64") => Ok(Platform::WindowsArm64),
            _ => Err(unpinned()),
        }
    }

    fn names(&self) -> Names {
        match self {
            Platform::LinuxX64 => names("linux-x64", "x86_64-unknown-linux-musl", ""),
            Platform::LinuxArm64 => names("linux-arm64", "aarch64-unknown-linux-musl", ""),
            Platform::MacX64 => names("darwin-x64", "x86_64-apple-darwin", ""),
            Platform::MacArm64 => names("darwin-arm64", "aarch64-apple-darwin", ""),
            Platform::WindowsX64 => names("win32-x64", "x86_64-pc-windows-msvc", ".exe"),
            Platform::WindowsArm64 => names("win32-arm64", "aarch64-pc-windows-msvc", ".exe"),
        }
    }
}

fn names(claude: &str, codex: &str, exe: &str) -> Names {
    Names {
        claude: String::from(claude),
        codex: String::from(codex),
        exe: String::from(exe),
    }
}

fn unpinned() -> InstallError {
    InstallError::Platform {
        os: String::from(std::env::consts::OS),
        arch: String::from(std::env::consts::ARCH),
    }
}

/// How a download becomes the install.
enum Package {
    /// The download is the binary.
    Binary,
    /// A `.tar.gz` of the whole install directory.
    Archive,
}

/// One CLI's pinned download for this platform.
struct Pin {
    version: String,
    url: String,
    sha256: String,
    package: Package,
    /// The binary's path in the install directory.
    binary: String,
}

fn pin(cli: Cli, platform: Platform) -> Result<Pin, InstallError> {
    let Names { claude, codex, exe } = platform.names();
    match cli {
        Cli::Claude => Ok(Pin {
            version: String::from(pins::CLAUDE_VERSION),
            url: format!(
                "https://downloads.claude.ai/claude-code-releases/{}/{claude}/claude{exe}",
                pins::CLAUDE_VERSION
            ),
            sha256: pins::claude_sha256(&claude).ok_or_else(unpinned)?,
            package: Package::Binary,
            binary: format!("claude{exe}"),
        }),
        Cli::Codex => Ok(Pin {
            version: String::from(pins::CODEX_VERSION),
            url: format!(
                "https://github.com/openai/codex/releases/download/rust-v{}/codex-package-{codex}.tar.gz",
                pins::CODEX_VERSION
            ),
            sha256: pins::codex_sha256(&codex).ok_or_else(unpinned)?,
            package: Package::Archive,
            binary: format!("bin/codex{exe}"),
        }),
    }
}

/// Makes sure the pinned `cli` is installed under `data_dir` and returns its
/// path. An installed binary was verified when it was put there.
pub async fn install(cli: Cli, data_dir: &Path) -> Result<PathBuf, InstallError> {
    let pin = pin(cli, Platform::current()?)?;
    let parent = data_dir.join("cli").join(cli.name());
    let dir = parent.join(&pin.version);
    let binary = dir.join(&pin.binary);
    match presence(&binary) {
        Presence::Present => {
            tracing::info!(path = %binary.display(), "pinned {} {} is installed", cli.name(), pin.version);
            Ok(binary)
        }
        Presence::Missing => {
            tracing::info!(url = %pin.url, "installing pinned {} {}", cli.name(), pin.version);
            std::fs::create_dir_all(&parent).map_err(|error| io(&parent, error))?;
            let download = parent.join(format!("{}.download", pin.version));
            let found = fetch(&pin.url, &download).await?;
            match found {
                found if found == pin.sha256 => {
                    unpack(&pin.package, &download, &dir, &binary).await?;
                    tracing::info!(path = %binary.display(), sha256 = %pin.sha256, "installed pinned {} {}", cli.name(), pin.version);
                    Ok(binary)
                }
                found => {
                    let _ = std::fs::remove_file(&download);
                    Err(InstallError::Mismatch {
                        url: pin.url,
                        found,
                        expected: pin.sha256,
                    })
                }
            }
        }
    }
}

/// Moves the verified download into place as `dir`, with the binary at
/// `binary`.
async fn unpack(
    package: &Package,
    download: &Path,
    dir: &Path,
    binary: &Path,
) -> Result<(), InstallError> {
    match package {
        Package::Binary => {
            std::fs::create_dir_all(dir).map_err(|error| io(dir, error))?;
            make_executable(download).map_err(|error| io(download, error))?;
            std::fs::rename(download, binary).map_err(|error| io(binary, error))
        }
        Package::Archive => {
            let staging = download.with_extension("unpack");
            let _ = std::fs::remove_dir_all(&staging);
            std::fs::create_dir_all(&staging).map_err(|error| io(&staging, error))?;
            let status = tokio::process::Command::new("tar")
                .arg("-xzf")
                .arg(download)
                .arg("-C")
                .arg(&staging)
                .status()
                .await
                .map_err(|error| io(download, error))?;
            match status.code() {
                Some(0) => {
                    // An earlier run may have left the directory without its binary.
                    let _ = std::fs::remove_dir_all(dir);
                    std::fs::rename(&staging, dir).map_err(|error| io(dir, error))?;
                    std::fs::remove_file(download).map_err(|error| io(download, error))
                }
                _ => Err(InstallError::Unpack {
                    archive: download.display().to_string(),
                    status: status.to_string(),
                }),
            }
        }
    }
}

/// Streams `url` into the file at `to` and returns the body's SHA-256 in hex.
async fn fetch(url: &str, to: &Path) -> Result<String, InstallError> {
    // reqwest is built without a crypto provider, as iroh builds it; this
    // is the same ring provider iroh uses.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let response = reqwest::Client::builder()
        .read_timeout(STALL)
        .build()?
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    let file = tokio::fs::File::create(to)
        .await
        .map_err(|error| io(to, error))?;
    drain(response, file, Sha256::new(), to).await
}

/// Writes each chunk of the body to the file and the hash, then syncs the file.
async fn drain(
    mut response: reqwest::Response,
    mut file: tokio::fs::File,
    mut hash: Sha256,
    to: &Path,
) -> Result<String, InstallError> {
    loop {
        match response.chunk().await? {
            Some(chunk) => {
                hash.update(&chunk);
                file.write_all(&chunk)
                    .await
                    .map_err(|error| io(to, error))?;
            }
            None => {
                file.sync_all().await.map_err(|error| io(to, error))?;
                break Ok(data_encoding::HEXLOWER.encode(hash.finalize().as_slice()));
            }
        }
    }
}

fn io(path: &Path, error: std::io::Error) -> InstallError {
    InstallError::Io {
        path: path.display().to_string(),
        error,
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// The pin and the protocol code move together (docs/architecture.md §6.2):
/// each pinned version has a fixture recorded at it, and Claude's is in the
/// supported range.
#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use serde_json::Value;

    use crate::claude::check_version;
    use crate::pins;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// The version `pins.toml` records for `cli`.
    fn pinned(cli: &str) -> String {
        let text = std::fs::read_to_string(root().join("pins.toml")).unwrap();
        let doc = text.parse::<toml_edit::DocumentMut>().unwrap();
        doc.get(cli)
            .and_then(|table| table.get("version"))
            .and_then(|version| version.as_str())
            .map(String::from)
            .unwrap()
    }

    /// Every string at `pointer` in the lines recorded in `fixtures/<dir>`.
    fn recorded(dir: &str, pointer: &str) -> BTreeSet<String> {
        std::fs::read_dir(root().join("fixtures").join(dir))
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .flat_map(|text| {
                text.lines()
                    .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                    .filter_map(|line| {
                        line.pointer(pointer)
                            .and_then(Value::as_str)
                            .map(String::from)
                    })
                    .collect::<Vec<String>>()
            })
            .collect()
    }

    #[test]
    fn claude_pin_is_recorded_and_supported() {
        let pin = pinned("claude");
        let versions = recorded("claude", "/response/response/claude_code_version");
        let initialize =
            serde_json::from_str(&format!("{{\"claude_code_version\": \"{pin}\"}}")).unwrap();
        assert_eq!(
            pin,
            pins::CLAUDE_VERSION,
            "pins.rs is not pins.toml; run tools/pin"
        );
        assert_eq!(
            versions.get(&pin),
            Some(&pin),
            "no fixture in fixtures/claude was recorded at the pinned claude; run s1"
        );
        assert_eq!(
            check_version(initialize),
            Ok(()),
            "the pinned claude is outside SUPPORTED_VERSIONS"
        );
    }

    #[test]
    fn codex_pin_is_recorded() {
        let pin = pinned("codex");
        let versions = recorded("codex", "/result/userAgent")
            .iter()
            .filter_map(|agent| agent.split_whitespace().next())
            .filter_map(|product| product.split_once('/'))
            .map(|(_, version)| String::from(version))
            .collect::<BTreeSet<String>>();
        assert_eq!(
            pin,
            pins::CODEX_VERSION,
            "pins.rs is not pins.toml; run tools/pin"
        );
        assert_eq!(
            versions.get(&pin),
            Some(&pin),
            "no fixture in fixtures/codex was recorded at the pinned codex; run s2"
        );
    }
}
