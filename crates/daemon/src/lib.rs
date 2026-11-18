//! The host daemon behind `conductord`. It serves the event log and the
//! sessions over iroh, and runs each instance through its provider's
//! [`runtime::Plugin`], knowing no provider by name.

pub mod credentials;
pub mod log;
pub mod registry;
pub mod serve;
pub mod session;
pub mod sign_in;
pub mod start;
mod storage;
mod workspace;

use std::io::{ErrorKind, Write};
use std::path::Path;

use anyhow::Context;
use host::Note;
use host::secret::Secret;
use iroh::SecretKey;
use proto::{AuthToken, Epoch};

/// The actor stopped: nobody can reach it any more.
#[derive(Debug, thiserror::Error)]
#[error("the actor has stopped")]
pub struct Stopped {}

/// Loads the host's iroh key from `<data_dir>/host.key`, creating it on first
/// run. The key's public half is the host's address, so it must persist.
///
/// # Errors
///
/// Fails if the key file can't be read, parsed or created.
pub fn load_or_create_key(data_dir: &Path) -> anyhow::Result<SecretKey> {
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
pub fn load_or_create_token(data_dir: &Path) -> anyhow::Result<AuthToken> {
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
    let next = data_dir.join(format!("{name}.{}.new", std::process::id()));
    write_private(&next, data_encoding::HEXLOWER.encode(&key).as_bytes())
        .with_context(|| format!("writing {}", next.display()))?;
    let linked = link_new(&next, path);
    let _ = std::fs::remove_file(&next);
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

/// Writes a new file and syncs it to disk. A stale file at `path` is
/// removed first, so the new one is always created, and on Unix created
/// with mode 0600. On Windows it inherits the data dir's ACL.
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let _ = std::fs::remove_file(path);
    let mut file = private_options().open(path)?;
    file.write_all(contents)?;
    file.sync_all()
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
pub fn random_token() -> anyhow::Result<AuthToken> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).context("no random source")?;
    Ok(AuthToken::from_bytes(bytes))
}

/// # Errors
///
/// Fails if the OS has no random source.
pub fn random_epoch() -> anyhow::Result<Epoch> {
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
            tracing::info!(target: "host", instance = instance.0, "stored a credential change");
        }
        Note::RefreshFailed { instance, reason } => {
            tracing::warn!(target: "host", %reason, instance = instance.0, "instance refresh failed");
        }
        Note::InstanceAdded { instance, provider } => {
            tracing::info!(target: "host", instance = instance.0, ?provider, "added an instance");
        }
        Note::InstanceRenamed { instance } => {
            tracing::info!(target: "host", instance = instance.0, "renamed an instance");
        }
        Note::InstanceRemoved { instance } => {
            tracing::info!(target: "host", instance = instance.0, "removed an instance");
        }
        Note::SignIn { instance, step } => {
            tracing::info!(target: "host", instance = instance.0, %step, "sign-in");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{load_or_create_key, load_or_create_token};

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
