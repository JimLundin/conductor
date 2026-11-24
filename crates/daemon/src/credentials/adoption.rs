use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, ensure};
use host::registry::{AccountAdoption, dir};
use serde::{Deserialize, Serialize};

/// The journal's format, its first byte. A journal kept before it had one
/// begins with its provider's index, 0 or 1, so the first format is 2.
const FORMAT: u8 = 2;

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Adoption {
    pub(super) account: AccountAdoption,
    staging: [Staging; 2],
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum Staging {
    Absent,
    Source,
    Staged,
}

impl Adoption {
    pub(super) fn begin(
        data: &Path,
        account: AccountAdoption,
    ) -> anyhow::Result<Self> {
        ensure!(
            dir(&account.from) != dir(&account.to),
            "the private directories coincide"
        );
        let source = dir(&account.from);
        let adoption = Self {
            account,
            staging: [
                Staging::inspect(&data.join("home").join(&source))?,
                Staging::inspect(&data.join("work").join(&source))?,
            ],
        };
        for directory in adoption.directories(data) {
            is_directory(&directory.destination)?;
        }
        adoption.save(data)?;
        Ok(adoption)
    }

    fn save(&self, data: &Path) -> anyhow::Result<()> {
        let journal = data.join("adoption");
        let next = journal.with_extension("new");
        let file = postcard::to_extend(self, Vec::from([FORMAT]))?;
        crate::write_private(&next, &file)?;
        rename(&next, &journal)?;
        Ok(crate::sync_dir(data)?)
    }

    pub(super) fn pending(data: &Path) -> anyhow::Result<Option<Self>> {
        let journal = data.join("adoption");
        match fs::read(&journal) {
            Ok(bytes) => Self::decode(&bytes)
                .map(Some)
                .with_context(|| format!("reading {}", journal.display())),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err)
                .with_context(|| format!("reading {}", journal.display())),
        }
    }

    fn decode(file: &[u8]) -> anyhow::Result<Self> {
        match file.split_first() {
            Some((&FORMAT, adoption)) => Ok(postcard::from_bytes(adoption)?),
            _ => anyhow::bail!(
                "the journal is in an older format; finish or undo the adoption it records by hand, then remove it"
            ),
        }
    }

    pub(super) fn move_directories(
        &mut self,
        data: &Path,
    ) -> anyhow::Result<()> {
        // A journal rename may have succeeded before its sync failed.
        crate::sync_dir(data)?;
        let directories = self.directories(data);
        for directory in &directories {
            is_directory(&directory.destination).context(
                "account adoption is pending; resolve the destination conflict and retry",
            )?;
        }
        for (index, directory) in directories.into_iter().enumerate() {
            self.move_directory(data, index, &directory)?;
        }
        Ok(())
    }

    fn move_directory(
        &mut self,
        data: &Path,
        index: usize,
        directory: &Directory,
    ) -> anyhow::Result<()> {
        match self.staging[index] {
            Staging::Absent => return Ok(()),
            Staging::Source => {
                directory.stage()?;
                self.staging[index] = Staging::Staged;
                self.save(data)?;
            }
            Staging::Staged => {}
        }
        directory.install()
    }

    pub(super) fn finish(&self, data: &Path) -> anyhow::Result<()> {
        for (directory, staging) in
            self.directories(data).into_iter().zip(self.staging)
        {
            if matches!(staging, Staging::Staged) {
                directory.discard_backup()?;
            }
        }
        fs::remove_file(data.join("adoption"))?;
        Ok(crate::sync_dir(data)?)
    }

    fn directories(&self, data: &Path) -> [Directory; 2] {
        ["home", "work"].map(|kind| {
            let parent = data.join(kind);
            Directory {
                source: parent.join(dir(&self.account.from)),
                destination: parent.join(dir(&self.account.to)),
                staged: parent.join(".adoption-source"),
                backup: parent.join(".adoption-backup"),
                parent,
            }
        })
    }
}

impl Staging {
    fn inspect(source: &Path) -> anyhow::Result<Self> {
        Ok(if is_directory(source)? {
            Self::Source
        } else {
            Self::Absent
        })
    }
}

struct Directory {
    parent: PathBuf,
    source: PathBuf,
    destination: PathBuf,
    staged: PathBuf,
    backup: PathBuf,
}

impl Directory {
    fn stage(&self) -> anyhow::Result<()> {
        if !is_directory(&self.staged)? {
            rename(&self.source, &self.staged)?;
        }
        Ok(crate::sync_dir(&self.parent)?)
    }

    fn install(&self) -> anyhow::Result<()> {
        if is_directory(&self.staged)? {
            self.prepare_destination()?;
            rename(&self.staged, &self.destination)?;
        } else {
            ensure!(
                is_directory(&self.destination)?,
                "the adopted directory {} is missing",
                self.destination.display()
            );
        }
        Ok(crate::sync_dir(&self.parent)?)
    }

    fn prepare_destination(&self) -> anyhow::Result<()> {
        if is_directory(&self.destination)? {
            if is_directory(&self.backup)? {
                // A launcher recreated the destination after the original was backed up.
                remove_directory(&self.destination)?;
            } else {
                rename(&self.destination, &self.backup)?;
            }
        }
        Ok(crate::sync_dir(&self.parent)?)
    }

    fn discard_backup(&self) -> anyhow::Result<()> {
        if is_directory(&self.backup)? {
            remove_directory(&self.backup)?;
        }
        // A previous removal may have succeeded before syncing the parent.
        Ok(crate::sync_dir(&self.parent)?)
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

fn remove_directory(path: &Path) -> anyhow::Result<()> {
    fs::remove_dir_all(path)
        .with_context(|| format!("removing {}", path.display()))
}
