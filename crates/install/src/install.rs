//! Installs the pinned CLIs (docs/architecture.md §6.2).
//!
//! Each one lands in `<data>/cli/<cli>/<version>/` by a download to a temporary
//! file, a SHA-256 check of that file against the pin and a rename, so a
//! crash leaves either nothing or a verified install. `<data>/cli/<cli>/lock`
//! keeps two processes from installing into one data dir at once. Which
//! build a platform takes is each provider's pin; whether a download is it
//! is decided in `agents::pin`.

use std::fs::{File, TryLockError};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use agents::pin::{Mismatch, Package, Pin};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const READ_STALL_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(200);
const READ_BUFFER: usize = 1_048_576;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("no pinned build for {os} on {arch}")]
    Platform { os: String, arch: String },
    /// The CLI pins no build for this platform, for the reason given.
    #[error("{0}")]
    Unsupported(String),
    #[error("starting or joining the archive extraction worker: {0}")]
    Worker(#[from] tokio::task::JoinError),
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

/// The build `pin` pins for the platform this host runs on: its OS and
/// architecture as Rust names them, its executables' suffix, and whether its
/// libc is musl.
///
/// # Errors
/// `pin` pins no build for this platform.
pub fn here(
    pin: fn(&str, &str, &str, bool) -> Option<Pin>,
) -> Result<Pin, InstallError> {
    pin(
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::consts::EXE_SUFFIX,
        cfg!(target_env = "musl"),
    )
    .ok_or_else(|| InstallError::Platform {
        os: String::from(std::env::consts::OS),
        arch: String::from(std::env::consts::ARCH),
    })
}

/// Makes sure `pin`, the CLI called `cli`, is installed under `data_dir`.
///
/// It returns the binary's path, after removing every other version. It
/// runs before any session, so no session can be using one.
///
/// # Errors
/// Fails when the download, verification or install fails.
pub async fn install(
    cli: &str,
    pin: Pin,
    data_dir: &Path,
) -> Result<PathBuf, InstallError> {
    put(cli, pin, &data_dir.join("cli").join(cli)).await
}

async fn put(
    name: &str,
    pin: Pin,
    parent: &Path,
) -> Result<PathBuf, InstallError> {
    let lock = lock(parent).await?;
    remove_others(parent, &pin.version)?;
    ensure(name, pin, parent, lock).await
}

async fn lock(parent: &Path) -> Result<File, InstallError> {
    std::fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
    let lock_path = parent.join("lock");
    let lock =
        File::create(&lock_path).map_err(|error| io(&lock_path, error))?;
    hold(&lock, &lock_path).await?;
    Ok(lock)
}

async fn ensure(
    name: &str,
    pin: Pin,
    parent: &Path,
    lock: File,
) -> Result<PathBuf, InstallError> {
    let binary = parent.join(&pin.version).join(&pin.binary);
    match presence(&binary) {
        Presence::Present => {
            tracing::info!(path = %binary.display(), "pinned {name} {} is installed", pin.version);
            Ok(binary)
        }
        Presence::Missing => download(name, pin, parent, binary, lock).await,
    }
}

async fn download(
    name: &str,
    pin: Pin,
    parent: &Path,
    binary: PathBuf,
    lock: File,
) -> Result<PathBuf, InstallError> {
    tracing::info!(url = %pin.url, "installing pinned {name} {}", pin.version);
    let download = parent.join(format!("{}.download", pin.version));
    fetch_verified(&pin, &download).await?;
    unpack(
        &pin.package,
        download,
        parent.join(&pin.version),
        &binary,
        lock,
    )
    .await?;
    tracing::info!(
        path = %binary.display(),
        sha256 = %pin.sha256,
        "installed pinned {name} {}",
        pin.version
    );
    Ok(binary)
}

async fn fetch_verified(
    pin: &Pin,
    download: &Path,
) -> Result<(), InstallError> {
    fetch(&pin.url, download).await?;
    agents::pin::verify(sha256(download).await?, &pin.sha256)
        .map_err(|Mismatch { found, expected }| InstallError::Mismatch {
            url: pin.url.clone(),
            found,
            expected,
        })
        .inspect_err(|_| {
            let _ = std::fs::remove_file(download);
        })
}

#[derive(Debug, Clone, Copy)]
enum Presence {
    Present,
    Missing,
}

fn presence(path: &Path) -> Presence {
    match std::fs::metadata(path) {
        Ok(_) => Presence::Present,
        Err(_) => Presence::Missing,
    }
}

async fn hold(lock: &File, path: &Path) -> Result<(), InstallError> {
    loop {
        match attempt(lock).await {
            Ok(()) => break Ok(()),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(error)) => break Err(io(path, error)),
        }
    }
}

async fn attempt(lock: &File) -> Result<(), TryLockError> {
    match lock.try_lock() {
        Err(TryLockError::WouldBlock) => {
            tokio::time::sleep(LOCK_POLL_INTERVAL).await;
            Err(TryLockError::WouldBlock)
        }
        tried => tried,
    }
}

fn remove_others(parent: &Path, keep: &str) -> Result<(), InstallError> {
    let others = std::fs::read_dir(parent)
        .map_err(|error| io(parent, error))?
        .filter_map(Result::ok)
        .filter(|entry| {
            !agents::pin::kept(&entry.file_name().to_string_lossy(), keep)
        })
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

async fn unpack(
    package: &Package,
    download: PathBuf,
    dir: PathBuf,
    binary: &Path,
    lock: File,
) -> Result<(), InstallError> {
    match package {
        Package::Binary => {
            std::fs::create_dir_all(&dir).map_err(|error| io(&dir, error))?;
            make_executable(&download).map_err(|error| io(&download, error))?;
            let result = std::fs::rename(download, binary)
                .map_err(|error| io(binary, error));
            drop(lock);
            result
        }
        Package::Archive => {
            let mut extractor = tar().map_err(|error| io(&download, error))?;
            extractor.args(["-xzf", "-"]);
            extract_archive(download, dir, lock, extractor).await
        }
    }
}

async fn extract_archive(
    download: PathBuf,
    dir: PathBuf,
    lock: File,
    extractor: Command,
) -> Result<(), InstallError> {
    let (cancel_on_drop, cancelled) = mpsc::channel();
    let result = tokio::task::spawn_blocking(move || {
        let _install_lock_until_extractor_exits = lock;
        unpack_archive(&download, &dir, extractor, &cancelled)
    })
    .await?;
    drop(cancel_on_drop);
    result
}

fn unpack_archive(
    download: &Path,
    dir: &Path,
    mut extractor: Command,
    cancelled: &Receiver<()>,
) -> Result<(), InstallError> {
    let staging = download.with_extension("unpack");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|error| io(&staging, error))?;
    let archive = File::open(download).map_err(|error| io(download, error))?;
    let mut child = extractor
        .stdin(archive)
        .current_dir(&staging)
        .spawn()
        .map_err(|error| io(download, error))?;
    let status = extractor_status(&mut child, cancelled)
        .map_err(|error| io(download, error))?;
    match status.code() {
        Some(0) => replace_incomplete_install(&staging, download, dir),
        _ => Err(InstallError::Unpack {
            archive: download.display().to_string(),
            status: status.to_string(),
        }),
    }
}

fn extractor_status(
    child: &mut Child,
    cancelled: &Receiver<()>,
) -> std::io::Result<ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
        if cancelled.recv_timeout(LOCK_POLL_INTERVAL)
            == Err(RecvTimeoutError::Disconnected)
        {
            let _ = child.kill();
            return child.wait();
        }
    }
}

fn tar() -> std::io::Result<Command> {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "SystemRoot is not set",
            )
        })?;
        Ok(Command::new(
            PathBuf::from(root).join("System32").join("tar.exe"),
        ))
    } else {
        Ok(Command::new("tar"))
    }
}

fn replace_incomplete_install(
    staging: &Path,
    download: &Path,
    dir: &Path,
) -> Result<(), InstallError> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::rename(staging, dir).map_err(|error| io(dir, error))?;
    std::fs::remove_file(download).map_err(|error| io(download, error))
}

async fn fetch(url: &str, to: &Path) -> Result<(), InstallError> {
    let (response, file) = open(url, to).await?;
    drain(response, file, to).await
}

async fn open(
    url: &str,
    to: &Path,
) -> Result<(reqwest::Response, tokio::fs::File), InstallError> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let response = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_STALL_TIMEOUT)
        .build()?
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    let file = tokio::fs::File::create(to)
        .await
        .map_err(|error| io(to, error))?;
    Ok((response, file))
}

async fn drain(
    mut response: reqwest::Response,
    mut file: tokio::fs::File,
    to: &Path,
) -> Result<(), InstallError> {
    write_body(&mut response, &mut file, to).await?;
    file.sync_all().await.map_err(|error| io(to, error))
}

async fn write_body(
    response: &mut reqwest::Response,
    file: &mut tokio::fs::File,
    to: &Path,
) -> Result<(), InstallError> {
    while copy_chunk(response, file, to).await? {}
    Ok(())
}

async fn copy_chunk(
    response: &mut reqwest::Response,
    file: &mut tokio::fs::File,
    to: &Path,
) -> Result<bool, InstallError> {
    let chunk = response.chunk().await?;
    file.write_all(chunk.as_deref().unwrap_or_default())
        .await
        .map_err(|error| io(to, error))?;
    Ok(chunk.is_some())
}

async fn sha256(path: &Path) -> Result<String, InstallError> {
    let path_error = |error| io(path, error);
    let file = tokio::fs::File::open(path).await.map_err(path_error)?;
    digest(BufReader::with_capacity(READ_BUFFER, file), Sha256::new())
        .await
        .map_err(path_error)
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
            0 => {
                break Ok(
                    data_encoding::HEXLOWER.encode(hash.finalize().as_slice())
                );
            }
            length => reader.consume(length),
        }
    }
}

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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::{Package, Pin, put, sha256};

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    const DOWNLOAD_PIECES: usize = 128;
    const PIECE_BYTES: usize = 65536;
    const SEVENS_SHA256: &str =
        "15d3670274ee9cc36af50fee9e6d49164c9eec2228d48fe29e4f313aa3784a91";

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
            DOWNLOAD_PIECES.saturating_mul(PIECE_BYTES)
        );
        stream.write_all(head.as_bytes()).await?;
        let piece = std::iter::repeat_n(7u8, PIECE_BYTES).collect::<Vec<u8>>();
        for piece in std::iter::repeat_n(&piece, DOWNLOAD_PIECES) {
            send_throttled(&mut stream, piece).await?;
        }
        stream.shutdown().await
    }

    async fn send_throttled(
        stream: &mut TcpStream,
        piece: &[u8],
    ) -> std::io::Result<()> {
        stream.write_all(piece).await?;
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        Ok(())
    }

    fn test_pin(url: &str) -> Pin {
        Pin {
            version: String::from("1.0.0"),
            url: String::from(url),
            sha256: String::from(SEVENS_SHA256),
            package: Package::Binary,
            binary: String::from("tool"),
        }
    }

    #[tokio::test]
    async fn two_installs_at_once_both_end_with_the_verified_binary()
    -> Result<(), Box<dyn std::error::Error>> {
        let parent = std::env::temp_dir()
            .join(format!("conductor-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        std::fs::create_dir_all(parent.join("0.9.0"))?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}/tool", listener.local_addr()?);
        tokio::spawn(serve(listener));
        let (first, second) = n0_future::future::zip(
            put("tool", test_pin(&url), &parent),
            put("tool", test_pin(&url), &parent),
        )
        .await;
        let binary = parent.join("1.0.0/tool");
        let left = std::fs::read_dir(&parent)?
            .map(|entry| {
                Ok(entry?
                    .file_name()
                    .into_string()
                    .map_err(|name| format!("{name:?}"))?)
            })
            .collect::<Result<BTreeSet<String>, Box<dyn std::error::Error>>>(
            )?;
        assert_eq!(
            (
                first.map_err(|error| error.to_string()),
                second.map_err(|error| error.to_string())
            ),
            (Ok(binary.clone()), Ok(binary.clone()))
        );
        assert_eq!(
            sha256(&binary).await.map_err(|error| error.to_string()),
            Ok(String::from(SEVENS_SHA256))
        );
        assert_eq!(
            left,
            BTreeSet::from([String::from("1.0.0"), String::from("lock")])
        );
        let _ = std::fs::remove_dir_all(&parent);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cancelling_archive_extraction_reaps_before_unlocking()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::process::Command;
        use std::time::Duration;

        let parent = root()
            .join("target")
            .join(format!("cancel-install-{}", std::process::id()));
        std::fs::create_dir_all(&parent)?;
        let download = parent.join("1.0.0.download");
        let staging = download.with_extension("unpack");
        let marker = staging.join("pid");
        std::fs::write(&download, b"")?;
        let held = super::lock(&parent).await?;
        let mut extractor = Command::new("sh");
        extractor.args(["-c", "echo $$ > pid; exec sleep 60"]);
        let install = tokio::spawn(super::extract_archive(
            download,
            parent.join("1.0.0"),
            held,
            extractor,
        ));
        let pid = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&marker)
                    && let Ok(pid) = text.trim().parse::<u32>()
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        install.abort();
        let _ = install.await;
        let next =
            tokio::time::timeout(Duration::from_secs(5), super::lock(&parent))
                .await??;
        let process = PathBuf::from(format!("/proc/{}", pid?));
        assert!(
            !process.exists(),
            "extractor still exists after lock was acquired"
        );
        super::remove_others(&parent, "1.0.0")?;
        assert!(!staging.exists());
        drop(next);
        std::fs::remove_dir_all(&parent)?;
        Ok(())
    }
}
