//! Installs the pinned CLIs (`pins.toml`, docs/architecture.md §6.2). Each
//! one lands in `<data>/cli/<cli>/<version>/` by a download to a temporary
//! file, a SHA-256 check of that file against the pin and a rename, so a
//! crash leaves either nothing or a verified install. `<data>/cli/<cli>/lock`
//! keeps two processes from installing into one data dir at once.

use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::live::{Presence, presence};
use crate::pins;

/// A connection that takes longer than this is to a dead network.
const CONNECT: Duration = Duration::from_secs(15);
/// A download that sends nothing for this long has stalled.
const STALL: Duration = Duration::from_secs(30);
/// How often a process waiting for another's install checks the lock.
const LOCK_POLL: Duration = Duration::from_millis(200);
const READ_BUFFER: usize = 1048576;

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

/// What each vendor calls this platform.
struct Names {
    /// The platform in Claude's `manifest.json`. Linux means glibc.
    claude: String,
    /// Codex's target triple.
    codex: String,
}

/// The platforms both CLIs publish builds for.
fn platform() -> Result<Names, InstallError> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok(names("linux-x64", "x86_64-unknown-linux-musl")),
        ("linux", "aarch64") => Ok(names("linux-arm64", "aarch64-unknown-linux-musl")),
        ("macos", "x86_64") => Ok(names("darwin-x64", "x86_64-apple-darwin")),
        ("macos", "aarch64") => Ok(names("darwin-arm64", "aarch64-apple-darwin")),
        ("windows", "x86_64") => Ok(names("win32-x64", "x86_64-pc-windows-msvc")),
        ("windows", "aarch64") => Ok(names("win32-arm64", "aarch64-pc-windows-msvc")),
        _ => Err(unpinned()),
    }
}

fn names(claude: &str, codex: &str) -> Names {
    Names {
        claude: String::from(claude),
        codex: String::from(codex),
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

fn pin(cli: Cli) -> Result<Pin, InstallError> {
    let Names { claude, codex } = platform()?;
    let exe = std::env::consts::EXE_SUFFIX;
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
/// path, after removing every other version. It runs before any session, so
/// no session can be using one.
pub async fn install(cli: Cli, data_dir: &Path) -> Result<PathBuf, InstallError> {
    put(
        cli.name(),
        pin(cli)?,
        &data_dir.join("cli").join(cli.name()),
    )
    .await
}

/// Installs `pin` in `parent`, holding `parent`'s lock throughout. An
/// installed binary was verified when it was put there.
async fn put(name: &str, pin: Pin, parent: &Path) -> Result<PathBuf, InstallError> {
    std::fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
    let lock_path = parent.join("lock");
    let lock = File::create(&lock_path).map_err(|error| io(&lock_path, error))?;
    hold(&lock, &lock_path).await?;
    remove_others(parent, &pin.version)?;
    let dir = parent.join(&pin.version);
    let binary = dir.join(&pin.binary);
    match presence(&binary) {
        Presence::Present => {
            tracing::info!(path = %binary.display(), "pinned {name} {} is installed", pin.version);
            Ok(binary)
        }
        Presence::Missing => {
            tracing::info!(url = %pin.url, "installing pinned {name} {}", pin.version);
            let download = parent.join(format!("{}.download", pin.version));
            fetch(&pin.url, &download).await?;
            match sha256(&download).await? {
                found if found == pin.sha256 => {
                    unpack(&pin.package, &download, &dir, &binary).await?;
                    tracing::info!(path = %binary.display(), sha256 = %pin.sha256, "installed pinned {name} {}", pin.version);
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

/// Waits until this process holds the lock on `lock`. Dropping the file
/// releases it.
async fn hold(lock: &File, path: &Path) -> Result<(), InstallError> {
    loop {
        match attempt(lock).await {
            Ok(_) => break Ok(()),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => break Err(io(path, error)),
        }
    }
}

/// Tries the lock once, and waits a little before reporting it busy.
async fn attempt(lock: &File) -> Result<(), TryLockError> {
    match lock.try_lock() {
        Err(TryLockError::WouldBlock) => {
            tokio::time::sleep(LOCK_POLL).await;
            Err(TryLockError::WouldBlock)
        }
        tried => tried,
    }
}

/// Removes everything in `parent` but the lock and the `keep` version: older
/// versions, and downloads and unpacks a crash left behind.
fn remove_others(parent: &Path, keep: &str) -> Result<(), InstallError> {
    let others = std::fs::read_dir(parent)
        .map_err(|error| io(parent, error))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name() != keep && entry.file_name() != "lock")
        .map(|entry| entry.path())
        .collect::<Vec<PathBuf>>();
    for other in others {
        remove(&other)?;
    }
    Ok(())
}

fn remove(path: &Path) -> Result<(), InstallError> {
    tracing::info!(path = %path.display(), "removing an older pinned CLI");
    std::fs::remove_dir_all(path)
        .or_else(|_| std::fs::remove_file(path))
        .map_err(|error| io(path, error))
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

/// Streams `url` into the file at `to`.
async fn fetch(url: &str, to: &Path) -> Result<(), InstallError> {
    // reqwest is built without a crypto provider, as iroh builds it; this
    // is the same ring provider iroh uses.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let response = reqwest::Client::builder()
        .connect_timeout(CONNECT)
        .read_timeout(STALL)
        .build()?
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    let file = tokio::fs::File::create(to)
        .await
        .map_err(|error| io(to, error))?;
    drain(response, file, to).await
}

/// Writes each chunk of the body to the file, then syncs it.
async fn drain(
    mut response: reqwest::Response,
    mut file: tokio::fs::File,
    to: &Path,
) -> Result<(), InstallError> {
    loop {
        match response.chunk().await? {
            Some(chunk) => file
                .write_all(&chunk)
                .await
                .map_err(|error| io(to, error))?,
            None => break file.sync_all().await.map_err(|error| io(to, error)),
        }
    }
}

/// The SHA-256 of the file at `path` as it is on disk, in hex.
async fn sha256(path: &Path) -> Result<String, InstallError> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| io(path, error))?;
    digest(BufReader::with_capacity(READ_BUFFER, file), Sha256::new())
        .await
        .map_err(|error| io(path, error))
}

async fn digest(
    mut reader: BufReader<tokio::fs::File>,
    mut hash: Sha256,
) -> std::io::Result<String> {
    loop {
        match reader
            .fill_buf()
            .await
            .map(|chunk| absorb(&mut hash, chunk))?
        {
            0 => break Ok(data_encoding::HEXLOWER.encode(hash.finalize().as_slice())),
            length => reader.consume(length),
        }
    }
}

/// Adds `chunk` to the hash and returns its length.
fn absorb(hash: &mut Sha256, chunk: &[u8]) -> usize {
    hash.update(chunk);
    chunk.len()
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
/// `pins.rs` is `pins.toml`, each pinned version has a fixture recorded at
/// it, and Claude's is in the supported range.
#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    use serde_json::Value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::{Package, Pin, put, sha256};
    use crate::claude::check_version;
    use crate::pins;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// One provider's section of `pins.toml`.
    struct Recorded {
        version: String,
        sha256: BTreeMap<String, String>,
    }

    fn pinned(cli: &str) -> Recorded {
        let text = std::fs::read_to_string(root().join("pins.toml")).unwrap();
        let doc = text.parse::<toml_edit::DocumentMut>().unwrap();
        let table = doc.get(cli).unwrap();
        Recorded {
            version: table
                .get("version")
                .and_then(|version| version.as_str())
                .map(String::from)
                .unwrap(),
            sha256: table
                .get("sha256")
                .and_then(|hashes| hashes.as_table_like())
                .unwrap()
                .iter()
                .map(|(key, value)| (String::from(key), String::from(value.as_str().unwrap())))
                .collect(),
        }
    }

    /// Each hash in `pins.toml` by its key, beside the one `pins.rs` has.
    fn hashes(recorded: &Recorded) -> BTreeMap<String, Option<String>> {
        recorded
            .sha256
            .iter()
            .map(|(key, value)| (key.clone(), Some(value.clone())))
            .collect()
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
    fn claude_pin_is_compiled_recorded_and_supported() {
        let pin = pinned("claude");
        let in_rs = pin
            .sha256
            .keys()
            .map(|key| (key.clone(), pins::claude_sha256(key)))
            .collect::<BTreeMap<String, Option<String>>>();
        let versions = recorded("claude", "/response/response/claude_code_version");
        let initialize =
            serde_json::from_str(&format!("{{\"claude_code_version\": \"{}\"}}", pin.version))
                .unwrap();
        assert_eq!(
            (pin.version.as_str(), in_rs),
            (pins::CLAUDE_VERSION, hashes(&pin)),
            "pins.rs is not pins.toml; run tools/pin"
        );
        assert_eq!(
            versions.get(&pin.version),
            Some(&pin.version),
            "no fixture in fixtures/claude was recorded at the pinned claude; run s1"
        );
        assert_eq!(
            check_version(initialize),
            Ok(()),
            "the pinned claude is outside SUPPORTED_VERSIONS"
        );
    }

    #[test]
    fn codex_pin_is_compiled_and_recorded() {
        let pin = pinned("codex");
        let in_rs = pin
            .sha256
            .keys()
            .map(|key| (key.clone(), pins::codex_sha256(key)))
            .collect::<BTreeMap<String, Option<String>>>();
        let versions = recorded("codex", "/result/userAgent")
            .iter()
            .filter_map(|agent| agent.split_whitespace().next())
            .filter_map(|product| product.split_once('/'))
            .map(|(_, version)| String::from(version))
            .collect::<BTreeSet<String>>();
        assert_eq!(
            (pin.version.as_str(), in_rs),
            (pins::CODEX_VERSION, hashes(&pin)),
            "pins.rs is not pins.toml; run tools/pin"
        );
        assert_eq!(
            versions.get(&pin.version),
            Some(&pin.version),
            "no fixture in fixtures/codex was recorded at the pinned codex; run s2"
        );
    }

    /// The test binary: 8 MiB of sevens, sent in 64 KiB pieces with a pause
    /// between them, so two downloads would overlap.
    const PIECES: usize = 128;
    const PIECE: usize = 65536;
    const SEVENS: &str = "15d3670274ee9cc36af50fee9e6d49164c9eec2228d48fe29e4f313aa3784a91";

    /// Answers every connection with the test binary.
    async fn serve(listener: TcpListener) -> std::io::Result<()> {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(answer(stream));
                }
                Err(error) => break Err(error),
            }
        }
    }

    async fn answer(mut stream: TcpStream) -> std::io::Result<()> {
        let mut request = [0u8; 1024];
        let _ = stream.read(&mut request).await?;
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            PIECES.saturating_mul(PIECE)
        );
        stream.write_all(head.as_bytes()).await?;
        for piece in std::iter::repeat_n([7u8; PIECE], PIECES) {
            send(&mut stream, &piece).await?;
        }
        stream.shutdown().await
    }

    async fn send(stream: &mut TcpStream, piece: &[u8]) -> std::io::Result<()> {
        stream.write_all(piece).await?;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        Ok(())
    }

    fn test_pin(url: &str) -> Pin {
        Pin {
            version: String::from("1.0.0"),
            url: String::from(url),
            sha256: String::from(SEVENS),
            package: Package::Binary,
            binary: String::from("tool"),
        }
    }

    /// Two processes starting on one data dir: the second waits for the
    /// first's install and finds it, and an older version is removed.
    #[tokio::test]
    async fn two_installs_at_once_both_end_with_the_verified_binary() {
        let parent = std::env::temp_dir().join(format!("conductor-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        std::fs::create_dir_all(parent.join("0.9.0")).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/tool", listener.local_addr().unwrap());
        tokio::spawn(serve(listener));
        let (first, second) = n0_future::future::zip(
            put("tool", test_pin(&url), &parent),
            put("tool", test_pin(&url), &parent),
        )
        .await;
        let binary = parent.join("1.0.0/tool");
        let left = std::fs::read_dir(&parent)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<BTreeSet<String>>();
        assert_eq!(
            (
                first.map_err(|error| error.to_string()),
                second.map_err(|error| error.to_string())
            ),
            (Ok(binary.clone()), Ok(binary.clone()))
        );
        assert_eq!(
            sha256(&binary).await.map_err(|error| error.to_string()),
            Ok(String::from(SEVENS))
        );
        assert_eq!(
            left,
            BTreeSet::from([String::from("1.0.0"), String::from("lock")])
        );
        let _ = std::fs::remove_dir_all(&parent);
    }
}
