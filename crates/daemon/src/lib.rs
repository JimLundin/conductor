//! `conductord`: the host daemon. It serves the event log and the sessions
//! over iroh (`conductord host`).

pub mod codex_hub;
pub mod credentials;
pub mod log;
mod mapping;
pub mod registry;
pub mod serve;
pub mod session;

use std::io::{ErrorKind, Write};
use std::path::Path;

use agents::Home;
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
    Ok(SecretKey::from_bytes(&load_or_create_secret(
        data_dir, "host.key",
    )?))
}

/// Loads 32 secret bytes from `<data_dir>/<name>` as hex, creating them on
/// first run, readable by the owner only.
fn load_or_create_secret(data_dir: &Path, name: &str) -> anyhow::Result<[u8; 32]> {
    let path = data_dir.join(name);
    match std::fs::read_to_string(&path) {
        Ok(text) => data_encoding::HEXLOWER
            .decode(text.trim().as_bytes())
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .with_context(|| format!("{} is not 64 hex digits", path.display())),
        Err(err) => match err.kind() {
            ErrorKind::NotFound => {
                let key = random_key();
                std::fs::create_dir_all(data_dir)?;
                write_private(&path, data_encoding::HEXLOWER.encode(&key).as_bytes())?;
                Ok(key)
            }
            _ => Err(err).with_context(|| format!("reading {}", path.display())),
        },
    }
}

/// Transcripts outlive Claude Code's default 30-day sweep; Conductor deletes
/// a transcript with its session (§6.2).
const CLAUDE_SETTINGS: &str = "{\n  \"cleanupPeriodDays\": 36500\n}\n";
const CODEX_CONFIG: &str = "check_for_update_on_startup = false\n";

/// The CLIs' private homes under `<data_dir>/home` (§6.2).
pub struct Homes {
    pub claude: Home,
    pub codex: Home,
}

/// Creates the private homes, writing each CLI's config on first run only,
/// since the CLIs edit their own config too.
pub fn homes(data_dir: &Path) -> anyhow::Result<Homes> {
    let claude = data_dir.join("home").join("claude");
    let codex = data_dir.join("home").join("codex");
    std::fs::create_dir_all(&claude).with_context(|| format!("creating {}", claude.display()))?;
    std::fs::create_dir_all(&codex).with_context(|| format!("creating {}", codex.display()))?;
    write_once(&claude.join("settings.json"), CLAUDE_SETTINGS)?;
    write_once(&codex.join("config.toml"), CODEX_CONFIG)?;
    Ok(Homes {
        claude: Home::Private(claude),
        codex: Home::Private(codex),
    })
}

fn write_once(path: &Path, contents: &str) -> anyhow::Result<()> {
    let opened = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path);
    match opened {
        Ok(mut file) => file
            .write_all(contents.as_bytes())
            .with_context(|| format!("writing {}", path.display())),
        Err(err) => match err.kind() {
            ErrorKind::AlreadyExists => Ok(()),
            _ => Err(err).with_context(|| format!("creating {}", path.display())),
        },
    }
}

/// Writes a file only its owner can read: mode 0600 when it is created.
#[cfg(unix)]
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(contents)
}

#[cfg(not(unix))]
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
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
    use agents::Home;

    use super::{homes, load_or_create_key};

    #[test]
    fn homes_get_their_config_once() {
        let dir = std::env::temp_dir().join(format!("conductord-homes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = homes(&dir).unwrap();
        let settings = dir.join("home/claude/settings.json");
        let config = dir.join("home/codex/config.toml");
        let written = (
            std::fs::read_to_string(&settings).unwrap(),
            std::fs::read_to_string(&config).unwrap(),
        );
        std::fs::write(&settings, "{}").unwrap();
        let _ = homes(&dir).unwrap();
        assert_eq!(
            (first.claude, first.codex),
            (
                Home::Private(dir.join("home/claude")),
                Home::Private(dir.join("home/codex"))
            )
        );
        assert_eq!(
            written,
            (
                String::from("{\n  \"cleanupPeriodDays\": 36500\n}\n"),
                String::from("check_for_update_on_startup = false\n")
            )
        );
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), "{}");
        let _ = std::fs::remove_dir_all(&dir);
    }

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
