use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use host::registry::dir;
use proto::InstanceId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Adoption {
    pub(super) from: InstanceId,
    pub(super) to: InstanceId,
    had_home: bool,
    had_work: bool,
}

impl Adoption {
    pub(super) fn begin(
        data: &Path,
        from: &InstanceId,
        to: &InstanceId,
    ) -> anyhow::Result<Self> {
        ensure!(dir(from) != dir(to), "the private directories coincide");
        let source = dir(from);
        let adoption = Self {
            from: from.clone(),
            to: to.clone(),
            had_home: is_directory(&data.join("home").join(&source))?,
            had_work: is_directory(&data.join("work").join(&source))?,
        };
        let journal = data.join("adoption");
        let next = journal.with_extension("new");
        crate::write_private(&next, &postcard::to_stdvec(&adoption)?)?;
        rename(&next, &journal)?;
        crate::sync_dir(data)?;
        Ok(adoption)
    }

    pub(super) fn pending(data: &Path) -> anyhow::Result<Option<Self>> {
        let journal = data.join("adoption");
        match fs::read(&journal) {
            Ok(bytes) => postcard::from_bytes(&bytes)
                .map(Some)
                .with_context(|| format!("reading {}", journal.display())),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err)
                .with_context(|| format!("reading {}", journal.display())),
        }
    }

    pub(super) fn move_directories(&self, data: &Path) -> anyhow::Result<()> {
        self.directories(data)
            .iter()
            .try_for_each(Directory::move_to)
    }

    pub(super) fn finish(&self, data: &Path) -> anyhow::Result<()> {
        for directory in self.directories(data) {
            directory.discard_backup()?;
        }
        fs::remove_file(data.join("adoption"))?;
        Ok(crate::sync_dir(data)?)
    }

    fn directories(&self, data: &Path) -> [Directory; 2] {
        [("home", self.had_home), ("work", self.had_work)].map(
            |(kind, existed)| {
                let parent = data.join(kind);
                Directory {
                    source: parent.join(dir(&self.from)),
                    destination: parent.join(dir(&self.to)),
                    backup: parent.join(".adoption-backup"),
                    parent,
                    existed,
                }
            },
        )
    }
}

struct Directory {
    parent: PathBuf,
    source: PathBuf,
    destination: PathBuf,
    backup: PathBuf,
    existed: bool,
}

impl Directory {
    fn move_to(&self) -> anyhow::Result<()> {
        if !self.existed {
            return Ok(());
        }
        if is_directory(&self.source)? {
            self.back_up_destination()?;
            rename(&self.source, &self.destination)?;
        } else {
            ensure!(
                is_directory(&self.destination)?,
                "the adopted directory {} is missing",
                self.destination.display()
            );
        }
        Ok(crate::sync_dir(&self.parent)?)
    }

    fn back_up_destination(&self) -> anyhow::Result<()> {
        if is_directory(&self.destination)? {
            ensure!(
                !is_directory(&self.backup)?,
                "both {} and its adoption backup exist",
                self.destination.display()
            );
            rename(&self.destination, &self.backup)?;
            crate::sync_dir(&self.parent)?;
        }
        Ok(())
    }

    fn discard_backup(&self) -> anyhow::Result<()> {
        if self.existed && is_directory(&self.backup)? {
            fs::remove_dir_all(&self.backup).with_context(|| {
                format!("removing {}", self.backup.display())
            })?;
            crate::sync_dir(&self.parent)?;
        }
        Ok(())
    }
}

fn is_directory(path: &Path) -> anyhow::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(metadata.is_dir(), "{} is not a directory", path.display());
            Ok(true)
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(false),
        Err(err) => {
            Err(err).with_context(|| format!("reading {}", path.display()))
        }
    }
}

fn rename(from: &Path, to: &Path) -> anyhow::Result<()> {
    fs::rename(from, to).with_context(|| {
        format!("moving {} to {}", from.display(), to.display())
    })
}
