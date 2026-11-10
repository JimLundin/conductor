//! `conductord`: the host daemon. For S3 it serves a simulated agent over
//! iroh (`conductord s3-host`).

pub mod host;
pub mod server;
mod wire;

use std::path::Path;

use anyhow::Context as _;
use iroh::SecretKey;

/// Loads the host's iroh key from `<data_dir>/host.key`, creating it on first
/// run. The key's public half is the host's address, so it must persist.
pub fn load_or_create_key(data_dir: &Path) -> anyhow::Result<SecretKey> {
    let path = data_dir.join("host.key");
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let bytes: [u8; 32] = data_encoding::HEXLOWER
                .decode(text.trim().as_bytes())
                .ok()
                .and_then(|b| b.try_into().ok())
                .with_context(|| format!("{} is not 64 hex digits", path.display()))?;
            Ok(SecretKey::from_bytes(&bytes))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let key = SecretKey::from_bytes(&random());
            std::fs::create_dir_all(data_dir)?;
            write_private(&path, &data_encoding::HEXLOWER.encode(&key.to_bytes()))?;
            Ok(key)
        }
        Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
    }
}

#[cfg(unix)]
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
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

pub fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("the OS has a random source");
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_created_once_then_reloaded() {
        let dir = std::env::temp_dir().join(format!("conductord-key-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let first = load_or_create_key(&dir).unwrap();
        let second = load_or_create_key(&dir).unwrap();
        assert_eq!(first.public(), second.public());
        std::fs::write(dir.join("host.key"), "nope").unwrap();
        assert!(load_or_create_key(&dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
