//! Where a CLI keeps its config, credentials and transcripts.

use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use agents::{Config, HomeKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Home {
    /// The user's own (`~/.claude`, `~/.codex`).
    User,
    /// A directory of the host's own, passed as `CLAUDE_CONFIG_DIR` or
    /// `CODEX_HOME`, so the user's homes stay untouched.
    Private(PathBuf),
}

impl Home {
    /// The private home of the instance `instance`, at
    /// `<data_dir>/home/<instance>` (§6.2), created with its provider's
    /// `config` on first run only, since the CLI edits its own config too.
    ///
    /// # Errors
    ///
    /// Fails if the directory or the config can't be created.
    pub fn private(
        data_dir: &Path,
        instance: &str,
        config: &Config,
    ) -> std::io::Result<Self> {
        let dir = data_dir.join("home").join(instance);
        std::fs::create_dir_all(&dir)?;
        write_once(&dir.join(&config.file), &config.contents)?;
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

#[cfg(test)]
mod tests {
    use super::Home;

    #[test]
    fn a_home_gets_its_config_once() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir()
            .join(format!("conductor-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let config = claude::profile().config;
        let home = Home::private(&dir, "work", &config)?;
        let settings = dir.join("home/work/settings.json");
        let written = std::fs::read_to_string(&settings)?;
        std::fs::write(&settings, "{}")?;
        let again = Home::private(&dir, "work", &config)?;
        let other = Home::private(&dir, "personal", &config)?;
        assert_eq!(
            (
                home,
                again,
                other,
                written,
                std::fs::read_to_string(&settings)?
            ),
            (
                Home::Private(dir.join("home/work")),
                Home::Private(dir.join("home/work")),
                Home::Private(dir.join("home/personal")),
                config.contents,
                String::from("{}")
            )
        );
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
