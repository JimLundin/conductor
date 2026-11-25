use std::fs::{self, File};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, ensure};
use git2::{Oid, Repository};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use super::observed::Checkout;

pub(super) const INDEX_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Intent {
    pub version: u8,
    pub key: String,
    pub branch: String,
    pub base: String,
    pub candidate: String,
    pub checkout: Checkout,
    pub original_index: String,
    pub candidate_index: String,
    pub history: String,
    pub history_length: u64,
}

pub(super) struct Journal {
    pub root: PathBuf,
    pub directory: PathBuf,
    pub key: String,
}

impl Journal {
    pub fn new(repo: &Repository, attempt: &str) -> Result<Self> {
        ensure!(
            !attempt.is_empty(),
            "missing snapshot installation identity"
        );
        let root = repo.path().join("orchestrator-install");
        let key = scope(repo, attempt)?;
        Ok(Self {
            directory: root.join(&key),
            root,
            key,
        })
    }

    pub fn pin(&self) -> String {
        format!("refs/conductor/prepared/{}", self.key)
    }

    pub fn receipt(&self) -> String {
        format!("refs/conductor/installed/{}", self.key)
    }

    pub fn message(&self) -> String {
        format!("install snapshot {}", self.key)
    }

    pub fn load(&self, branch: &str) -> Result<Intent> {
        let bytes = read(&self.directory.join("intent"), 64 * 1024)?;
        let (digest, data) = bytes
            .split_at_checked(32)
            .context("truncated installation intent")?;
        ensure!(
            blake3::hash(data).as_bytes() == digest,
            "installation intent checksum differs"
        );
        let intent: Intent = serde_json::from_slice(data)?;
        ensure!(
            intent.version == 1
                && intent.key == self.key
                && intent.branch == branch,
            "installation intent belongs to another request or format"
        );
        Ok(intent)
    }

    pub fn reserve(&self, intent: &Intent, index: &[u8]) -> Result<()> {
        directory(&self.root)?;
        write(&self.root, "active", self.key.as_bytes())?;
        fs::create_dir(&self.directory)?;
        sync(&self.root)?;
        write(&self.directory, "index", index)?;
        let data = serde_json::to_vec(intent)?;
        let mut sealed = blake3::hash(&data).as_bytes().to_vec();
        sealed.extend(data);
        write(&self.directory, "intent", &sealed)
    }

    pub fn release(&self) -> Result<()> {
        let active = self.root.join("active");
        if optional(&active, 64)?.as_deref() == Some(self.key.as_bytes()) {
            fs::remove_file(active)?;
            sync(&self.root)?;
        }
        Ok(())
    }

    pub fn target(repo: &Repository, reference: &str) -> Result<Option<Oid>> {
        match repo.find_reference(reference) {
            Ok(found) => Ok(Some(
                found
                    .target()
                    .context("installation reference is symbolic")?,
            )),
            Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn marker(&self, name: &str) -> Result<bool> {
        let value = optional(&self.directory.join(name), 64)?;
        ensure!(
            value
                .as_deref()
                .is_none_or(|bytes| bytes == self.key.as_bytes()),
            "invalid installation progress record"
        );
        Ok(value.is_some())
    }

    pub fn advance(&self, name: &str) -> Result<()> {
        write(&self.directory, name, self.key.as_bytes())
    }
}

pub(super) fn scope(repo: &Repository, attempt: &str) -> Result<String> {
    let mut digest = blake3::Hasher::new();
    let admin = repo.path().canonicalize()?;
    for part in [admin.as_os_str().as_encoded_bytes(), attempt.as_bytes()] {
        digest.update(&(part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    Ok(digest.finalize().to_hex().to_string())
}

pub(super) fn directory(path: &Path) -> Result<()> {
    if !path.try_exists()? {
        fs::create_dir(path)?;
        sync(path.parent().context("missing journal parent")?)?;
    }
    ensure!(
        path.symlink_metadata()?.is_dir(),
        "invalid installation journal directory"
    );
    Ok(())
}

pub(super) fn sync(path: &Path) -> Result<()> {
    Ok(File::open(path)?.sync_all()?)
}

fn write(directory: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut file = NamedTempFile::new_in(directory)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(directory.join(name))?;
    sync(directory)
}

pub(super) fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    optional(path, limit)?
        .context("required snapshot installation file is missing")
}

pub(super) fn optional(path: &Path, limit: u64) -> Result<Option<Vec<u8>>> {
    let meta = match path.symlink_metadata() {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    limited(path, &meta, limit).map(Some)
}

fn limited(path: &Path, meta: &fs::Metadata, limit: u64) -> Result<Vec<u8>> {
    ensure!(
        meta.is_file() && meta.len() <= limit,
        "unsupported installation file"
    );
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "installation file exceeded its size limit"
    );
    Ok(bytes)
}
