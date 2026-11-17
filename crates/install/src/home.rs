//! Where a CLI keeps its config, credentials and transcripts.

use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use agents::HomeKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Home {
    /// The user's own (`~/.claude`, `~/.codex`).
    User,
    /// A directory of the host's own, passed as `CLAUDE_CONFIG_DIR` or
    /// `CODEX_HOME`, so the user's homes stay untouched.
    Private(PathBuf),
}

impl Home {
    /// A private home at `dir`, created with `config` written to `file`
    /// on first run only, since the CLI edits its own config too.
    ///
    /// # Errors
    ///
    /// Fails if the directory or the config can't be created.
    pub fn private(
        dir: PathBuf,
        file: &str,
        config: &str,
    ) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        write_once(&dir.join(file), config)?;
        Ok(Self::Private(dir))
    }

    #[must_use]
    pub const fn kind(&self) -> HomeKind {
        match self {
            Self::User => HomeKind::User,
            Self::Private(_) => HomeKind::Private,
        }
    }
}

fn write_once(path: &Path, contents: &str) -> std::io::Result<()> {
    let opened = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path);
    match opened {
        Ok(mut file) => file.write_all(contents.as_bytes()),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(err),
    }
}
