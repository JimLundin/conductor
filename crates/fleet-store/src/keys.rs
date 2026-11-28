use std::io::ErrorKind;
use std::path::Path;

use anyhow::{Context, bail, ensure};
use host::secret::Secret;

pub fn exists(dir: &Path, name: &str) -> anyhow::Result<bool> {
    let path = dir.join(name);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file(),
                "{} must be a regular file",
                path.display()
            );
            Ok(true)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => {
            Err(error).with_context(|| format!("inspecting {}", path.display()))
        }
    }
}

pub fn read_file(dir: &Path, name: &str) -> anyhow::Result<Option<[u8; 32]>> {
    if !exists(dir, name)? {
        return Ok(None);
    }
    let path = dir.join(name);
    let text = zeroize::Zeroizing::new(
        std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?,
    );
    match host::secret::stored(Some(&text)) {
        Secret::Key(key) => Ok(Some(key)),
        Secret::Missing | Secret::Malformed => {
            bail!("{} is not 64 hex digits", path.display())
        }
    }
}
