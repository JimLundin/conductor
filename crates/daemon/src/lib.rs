//! The host daemon behind `conductord`. It serves the event log and the
//! sessions over iroh, and runs each instance through its provider's
//! [`runtime::Plugin`], knowing no provider by name.

mod credentials;
mod home;
mod log;
mod progress;
mod registry;
mod serve;
mod session;
mod sign_in;
mod start;
mod storage;
mod workspace;

pub use start::{HostOptions, Hosted, Hosting, Plugins, login, serve, start};

use std::io::{ErrorKind, Write};
use std::path::Path;

use anyhow::Context;
use host::Note;
use host::secret::Secret;
use iroh::SecretKey;
use proto::{AuthToken, Epoch};
use tokio::sync::{mpsc, oneshot};

/// The actor stopped: nobody can reach it any more.
#[derive(Debug, thiserror::Error)]
#[error("the actor has stopped")]
pub(crate) struct Stopped {}

async fn ask<Msg, T>(
    inbox: &mpsc::Sender<Msg>,
    build: impl FnOnce(oneshot::Sender<T>) -> Msg,
) -> Result<T, Stopped> {
    let (reply, answer) = oneshot::channel();
    inbox.send(build(reply)).await.map_err(|_| Stopped {})?;
    answer.await.map_err(|_| Stopped {})
}

/// Loads the host's iroh key from `<data_dir>/host.key`, creating it on first
/// run. The key's public half is the host's address, so it must persist.
///
/// # Errors
///
/// Fails if the key file can't be read, parsed or created.
fn load_or_create_key(data_dir: &Path) -> anyhow::Result<SecretKey> {
    Ok(SecretKey::from_bytes(&load_or_create_secret(
        data_dir, "host.key",
    )?))
}

/// Keeps the ticket's authentication token stable across host restarts.
/// It is stored with the same private-file and sync guarantees as the host key.
///
/// # Errors
///
/// Fails if the token file can't be read, parsed or created.
fn load_or_create_token(data_dir: &Path) -> anyhow::Result<AuthToken> {
    let secret = load_or_create_secret(data_dir, "auth.key")?;
    let (bytes, _) = secret
        .split_at_checked(16)
        .context("a token has 16 bytes")?;
    Ok(AuthToken::from_bytes(bytes.try_into()?))
}

/// Loads 32 secret bytes from `<data_dir>/<name>` as hex, creating them on
/// first run, readable by the owner only.
fn load_or_create_secret(
    data_dir: &Path,
    name: &str,
) -> anyhow::Result<[u8; 32]> {
    let path = data_dir.join(name);
    let text = read_if_exists(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    match host::secret::stored(text.as_deref()) {
        Secret::Key(key) => Ok(key),
        Secret::Malformed => Err(anyhow::Error::msg(format!(
            "{} is not 64 hex digits",
            path.display()
        ))),
        Secret::Missing => create_secret(data_dir, name, &path),
    }
}

/// A new key is written whole beside the file and then linked to its
/// name, which fails if the name exists, so of two first starts one key
/// wins and the other start reads it.
fn create_secret(
    data_dir: &Path,
    name: &str,
    path: &Path,
) -> anyhow::Result<[u8; 32]> {
    let key = random_key()?;
    std::fs::create_dir_all(data_dir)?;
    let contents = data_encoding::HEXLOWER.encode(&key);
    let linked = with_private_file(data_dir, contents.as_bytes(), |next| {
        link_new(next, path)
    });
    if linked.with_context(|| format!("creating {}", path.display()))? {
        sync_dir(data_dir)
            .with_context(|| format!("syncing {}", data_dir.display()))?;
        Ok(key)
    } else {
        load_or_create_secret(data_dir, name)
    }
}

/// The file's text, or `None` when there is no file.
fn read_if_exists(path: &Path) -> std::io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Links `link` to `original`, or says it already exists.
fn link_new(original: &Path, link: &Path) -> std::io::Result<bool> {
    match std::fs::hard_link(original, link) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(err),
    }
}

fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().ok_or(ErrorKind::InvalidInput)?;
    with_private_file(dir, contents, |next| std::fs::rename(next, path))?;
    sync_dir(dir)
}

fn with_private_file<T>(
    dir: &Path,
    contents: &[u8],
    publish: impl FnOnce(&Path) -> std::io::Result<T>,
) -> std::io::Result<T> {
    let nonce = getrandom::u64().map_err(std::io::Error::other)?;
    let next = dir.join(format!(".credential-{nonce:016x}.new"));
    let mut file = private_options().open(&next)?;
    let written = file.write_all(contents).and_then(|()| file.sync_all());
    drop(file);
    let result = written.and_then(|()| publish(&next));
    let _ = std::fs::remove_file(next);
    result
}

#[cfg(unix)]
fn private_options() -> std::fs::OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    options
}

#[cfg(not(unix))]
fn private_options() -> std::fs::OpenOptions {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    options
}

/// Makes a rename or link in `dir` durable. Windows has no directory sync.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

fn random_key() -> anyhow::Result<[u8; 32]> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).context("no random source")?;
    Ok(bytes)
}

/// # Errors
///
/// Fails if the OS has no random source.
fn random_epoch() -> anyhow::Result<Epoch> {
    Ok(Epoch(getrandom::u64().context("no random source")?))
}

/// Logs what the host's core decided to report.
fn log_note(note: &Note) {
    match note {
        Note::Ready {
            path,
            version,
            own,
            models,
        } => {
            tracing::info!(target: "host", %path, %version, ?own, models, "ready");
        }
        Note::Unavailable { reason } => {
            tracing::warn!(target: "host", %reason, "unavailable");
        }
        Note::NoModels { reason } => {
            tracing::warn!(target: "host", %reason, "the CLI listed no models");
        }
        Note::CredentialStored { instance } => {
            tracing::info!(target: "host", %instance, "stored a credential change");
        }
        Note::RefreshFailed { instance, reason } => {
            tracing::warn!(target: "host", %reason, %instance, "instance refresh failed");
        }
        Note::InstanceAdded { instance } => {
            tracing::info!(target: "host", %instance, "added an instance");
        }
        Note::InstanceRemoved { instance } => {
            tracing::info!(target: "host", %instance, "removed an instance");
        }
        Note::SignIn { instance, step } => {
            tracing::info!(target: "host", %instance, %step, "sign-in");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{load_or_create_key, load_or_create_token};

    fn scratch(label: &str) -> anyhow::Result<std::path::PathBuf> {
        let binary = std::env::current_exe()?;
        let root = binary.ancestors().nth(3).ok_or_else(|| {
            anyhow::anyhow!("the test binary is not in target/debug/deps")
        })?;
        let nonce = getrandom::u64()?;
        let dir = root
            .join("tmp")
            .join(format!("conductor-{label}-{nonce:016x}"));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    #[test]
    fn concurrent_initializers_return_the_installed_token() -> anyhow::Result<()>
    {
        let dir = scratch("token-race")?;
        let barrier = std::sync::Barrier::new(2);
        let (joined, second) = std::thread::scope(|scope| {
            let other = scope.spawn(|| {
                barrier.wait();
                load_or_create_token(&dir)
            });
            barrier.wait();
            let ours = load_or_create_token(&dir);
            (other.join(), ours)
        });
        let disk = load_or_create_token(&dir);
        std::fs::remove_dir_all(dir)?;
        let first =
            joined.map_err(|_| anyhow::anyhow!("initializer panicked"))??;
        let installed = disk?;
        assert!(first == installed && second? == installed);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn pairing_image_is_private_on_creation_and_replacement()
    -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch("qr-mode")?;
        let path = dir.join("join-qr.svg");
        super::write_private(&path, b"first")?;
        let created = std::fs::metadata(&path)?.permissions().mode() & 0o777;
        std::fs::set_permissions(
            &path,
            std::fs::Permissions::from_mode(0o644),
        )?;
        super::write_private(&path, b"second")?;
        let replaced = std::fs::metadata(&path)?.permissions().mode() & 0o777;
        let contents = std::fs::read(path)?;
        std::fs::remove_dir_all(dir)?;
        assert_eq!(
            (created, replaced, contents.as_slice()),
            (0o600, 0o600, b"second".as_slice())
        );
        Ok(())
    }

    #[test]
    fn ticket_token_is_created_once_then_reloaded()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir()
            .join(format!("conductord-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = load_or_create_token(&dir)?;
        assert_eq!(load_or_create_token(&dir)?, first);
        std::fs::write(dir.join("auth.key"), "not a key")?;
        assert_eq!(
            load_or_create_token(&dir)
                .err()
                .map(|_| String::from("refused")),
            Some(String::from("refused"))
        );
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn key_is_created_once_then_reloaded()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir()
            .join(format!("conductord-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = load_or_create_key(&dir)?;
        let second = load_or_create_key(&dir)?;
        assert_eq!(first.public(), second.public());
        std::fs::write(dir.join("host.key"), "nope")?;
        assert_eq!(
            load_or_create_key(&dir).err().map(|err| err.to_string()),
            Some(format!(
                "{} is not 64 hex digits",
                dir.join("host.key").display()
            ))
        );
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
