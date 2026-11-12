//! `conductord`: the host daemon. It serves the event log and the sessions
//! over iroh (`conductord host`).

pub mod codex_hub;
pub mod log;
mod mapping;
pub mod registry;
pub mod serve;
pub mod session;
mod wire;

use std::io::ErrorKind;
use std::path::Path;

use anyhow::Context;
use iroh::SecretKey;
use proto::{AuthToken, Epoch};

/// The actor stopped: nobody can reach it any more.
#[derive(Debug, thiserror::Error)]
#[error("the actor has stopped")]
pub struct Stopped {}

/// Loads the host's iroh key from `<data_dir>/host.key`, creating it on first
/// run. The key's public half is the host's address, so it must persist.
pub fn load_or_create_key(data_dir: &Path) -> anyhow::Result<SecretKey> {
    let path = data_dir.join("host.key");
    match std::fs::read_to_string(&path) {
        Ok(text) => data_encoding::HEXLOWER
            .decode(text.trim().as_bytes())
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .map(|bytes| SecretKey::from_bytes(&bytes))
            .with_context(|| format!("{} is not 64 hex digits", path.display())),
        Err(err) => match err.kind() {
            ErrorKind::NotFound => {
                let key = SecretKey::from_bytes(&random_key());
                std::fs::create_dir_all(data_dir)?;
                write_private(&path, &data_encoding::HEXLOWER.encode(&key.to_bytes()))?;
                Ok(key)
            }
            _ => Err(err).with_context(|| format!("reading {}", path.display())),
        },
    }
}

#[cfg(unix)]
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?
        .write_all(contents.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    std::fs::write(path, contents)
}

fn random_key() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS has a random source");
    bytes
}

pub fn random_token() -> AuthToken {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS has a random source");
    AuthToken::from_bytes(bytes)
}

pub fn random_epoch() -> Epoch {
    Epoch(getrandom::u64().expect("the OS has a random source"))
}

#[cfg(test)]
mod tests {
    use super::load_or_create_key;

    #[test]
    fn key_is_created_once_then_reloaded() {
        let dir = std::env::temp_dir().join(format!("conductord-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = load_or_create_key(&dir).unwrap();
        let second = load_or_create_key(&dir).unwrap();
        assert_eq!(first.public(), second.public());
        std::fs::write(dir.join("host.key"), "nope").unwrap();
        assert_eq!(
            load_or_create_key(&dir).err().map(|err| err.to_string()),
            Some(format!(
                "{} is not 64 hex digits",
                dir.join("host.key").display()
            ))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
