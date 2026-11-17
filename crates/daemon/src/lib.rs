//! `conductord`: the host daemon. It serves the event log and the sessions
//! over iroh (`conductord host`).

pub mod credentials;
pub mod log;
pub mod registry;
pub mod serve;
pub mod session;
pub mod start;
mod storage;
mod workspace;

use std::io::{ErrorKind, Write};
use std::path::Path;

use anyhow::Context;
use install::Home;
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
/// first run, readable by the owner only. A new key is written whole beside
/// the file and then linked to its name, which fails if the name exists, so
/// of two first starts one key wins and the other start reads it.
fn load_or_create_secret(
    data_dir: &Path,
    name: &str,
) -> anyhow::Result<[u8; 32]> {
    let path = data_dir.join(name);
    match std::fs::read_to_string(&path) {
        Ok(text) => data_encoding::HEXLOWER
            .decode(text.trim().as_bytes())
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .with_context(|| {
                format!("{} is not 64 hex digits", path.display())
            }),
        Err(err) => match err.kind() {
            ErrorKind::NotFound => {
                let key = random_key()?;
                std::fs::create_dir_all(data_dir)?;
                let next =
                    data_dir.join(format!("{name}.{}.new", std::process::id()));
                write_private(
                    &next,
                    data_encoding::HEXLOWER.encode(&key).as_bytes(),
                )
                .with_context(|| format!("writing {}", next.display()))?;
                let linked = std::fs::hard_link(&next, &path);
                let _ = std::fs::remove_file(&next);
                match linked {
                    Ok(()) => {
                        sync_dir(data_dir).map(|()| key).with_context(|| {
                            format!("syncing {}", data_dir.display())
                        })
                    }
                    Err(err) => match err.kind() {
                        ErrorKind::AlreadyExists => {
                            load_or_create_secret(data_dir, name)
                        }
                        _ => Err(err).with_context(|| {
                            format!("creating {}", path.display())
                        }),
                    },
                }
            }
            _ => {
                Err(err).with_context(|| format!("reading {}", path.display()))
            }
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
///
/// # Errors
///
/// Fails if a home or its config can't be created.
pub fn homes(data_dir: &Path) -> anyhow::Result<Homes> {
    let claude = data_dir.join("home").join("claude");
    let codex = data_dir.join("home").join("codex");
    std::fs::create_dir_all(&claude)
        .with_context(|| format!("creating {}", claude.display()))?;
    std::fs::create_dir_all(&codex)
        .with_context(|| format!("creating {}", codex.display()))?;
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
            _ => {
                Err(err).with_context(|| format!("creating {}", path.display()))
            }
        },
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

#[cfg(test)]
mod tests {
    use install::Home;

    use super::{homes, load_or_create_key, load_or_create_token};

    #[test]
    fn homes_get_their_config_once() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir()
            .join(format!("conductord-homes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let first = homes(&dir)?;
        let settings = dir.join("home/claude/settings.json");
        let config = dir.join("home/codex/config.toml");
        let written = (
            std::fs::read_to_string(&settings)?,
            std::fs::read_to_string(&config)?,
        );
        std::fs::write(&settings, "{}")?;
        let _ = homes(&dir)?;
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
        assert_eq!(std::fs::read_to_string(&settings)?, "{}");
        let _ = std::fs::remove_dir_all(&dir);
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
