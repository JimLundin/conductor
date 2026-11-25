use std::fs::{self, File};
use std::io::{self, Read as _, Seek as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, ensure};
use blake3::Hasher;
use git2::{Oid, Repository};
use serde::{Deserialize, Serialize};

use super::journal::{Intent, Journal};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Checkout {
    worktree: String,
    administrative: String,
    common: String,
    head: String,
    files: String,
    configuration: String,
}

fn field(hash: &mut Hasher, bytes: &[u8]) {
    hash.update(&(bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

fn digest(path: &Path) -> Result<blake3::Hash> {
    let mut hash = Hasher::new();
    hash.update_reader(File::open(path)?)?;
    Ok(hash.finalize())
}

#[cfg(unix)]
fn identity(path: &Path) -> Result<String> {
    use std::os::unix::fs::MetadataExt as _;
    let resolved = path.canonicalize()?;
    let metadata = resolved.metadata()?;
    let mut hash = Hasher::new();
    field(&mut hash, resolved.as_os_str().as_encoded_bytes());
    field(&mut hash, &metadata.dev().to_be_bytes());
    field(&mut hash, &metadata.ino().to_be_bytes());
    Ok(hash.finalize().to_hex().to_string())
}

#[cfg(not(unix))]
fn identity(_path: &Path) -> Result<String> {
    anyhow::bail!(
        "snapshot installation requires verified checkout identity support"
    )
}

fn entries(path: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    paths.sort();
    Ok(paths)
}

fn visit(root: &Path, path: &Path, hash: &mut Hasher) -> Result<()> {
    let md = path.symlink_metadata()?;
    field(
        hash,
        path.strip_prefix(root)?.as_os_str().as_encoded_bytes(),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        field(hash, &md.mode().to_be_bytes());
    }
    if md.is_dir() {
        field(hash, b"directory");
        return entries(path)?
            .iter()
            .try_for_each(|child| visit(root, child, hash));
    }
    if md.is_symlink() {
        field(hash, b"link");
        field(hash, fs::read_link(path)?.as_os_str().as_encoded_bytes());
        return Ok(());
    }
    ensure!(md.is_file(), "unsupported entry in the owned workspace");
    field(hash, b"file");
    field(hash, digest(path)?.as_bytes());
    Ok(())
}

fn configuration(repo: &Repository) -> Result<String> {
    let cfg = repo.config()?.snapshot()?;
    let mut hash = config_values(&cfg)?;
    for leaf in ["attributes", "exclude"] {
        config_file(&repo.commondir().join("info").join(leaf), &mut hash)?;
    }
    for key in ["core.attributesfile", "core.excludesfile"] {
        configured_file(&cfg, key, &mut hash)?;
    }
    Ok(hash.finalize().to_hex().to_string())
}

fn config_values(cfg: &git2::Config) -> Result<Hasher> {
    let mut hash = Hasher::new();
    for pattern in ["core.*", "extensions.*", "filter.*", "merge.*", "index.*"]
    {
        cfg.entries(Some(pattern))?.for_each(|entry| {
            field(&mut hash, entry.name_bytes());
            field(&mut hash, entry.value_bytes());
        })?;
    }
    Ok(hash)
}

fn configured_file(
    cfg: &git2::Config,
    key: &str,
    hash: &mut Hasher,
) -> Result<()> {
    match cfg.get_path(key) {
        Ok(path) => config_file(&path, hash),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn config_file(path: &Path, hash: &mut Hasher) -> Result<()> {
    field(hash, path.as_os_str().as_encoded_bytes());
    if path.try_exists()? {
        field(hash, digest(path)?.as_bytes());
    } else {
        field(hash, b"absent");
    }
    Ok(())
}

impl Checkout {
    pub fn observe(repo: &Repository) -> Result<Self> {
        let root = repo.workdir().context("missing owned checkout")?;
        let mut hash = Hasher::new();
        entries(root)?
            .into_iter()
            .filter(|path| path.file_name().is_some_and(|name| name != ".git"))
            .try_for_each(|path| visit(root, &path, &mut hash))?;
        Ok(Self {
            worktree: identity(root)?,
            administrative: identity(repo.path())?,
            common: identity(repo.commondir())?,
            head: digest(&repo.path().join("HEAD"))?.to_hex().to_string(),
            files: hash.finalize().to_hex().to_string(),
            configuration: configuration(repo)?,
        })
    }
}

pub fn history(repo: &Repository, branch: &str) -> Result<(String, u64)> {
    let mut history = File::open(repo.commondir().join("logs").join(branch))?;
    let mut hash = Hasher::new();
    hash.update_reader(&mut history)?;
    Ok((
        hash.finalize().to_hex().to_string(),
        history.stream_position()?,
    ))
}

pub fn published(
    repo: &Repository,
    intent: &Intent,
    journal: &Journal,
) -> Result<bool> {
    let file = File::open(repo.commondir().join("logs").join(&intent.branch))?;
    let mut prefix = file.take(intent.history_length);
    let mut hash = Hasher::new();
    hash.update_reader(&mut prefix)?;
    if hash.finalize().to_hex().as_str() != intent.history
        || prefix.limit() != 0
    {
        return Ok(false);
    }
    let mut suffix = Vec::new();
    prefix.into_inner().take(8193).read_to_end(&mut suffix)?;
    Ok(publication_line(&suffix, intent, journal))
}

fn publication_line(suffix: &[u8], intent: &Intent, journal: &Journal) -> bool {
    let ending = format!("\t{}\n", journal.message());
    let start = format!("{} {} ", intent.base, intent.candidate);
    suffix.len() <= 8192
        && suffix.starts_with(start.as_bytes())
        && suffix.ends_with(ending.as_bytes())
        && suffix.iter().filter(|byte| **byte == b'\n').count() == 1
}

pub fn oid(text: &str) -> Result<Oid> {
    let oid = Oid::from_str(text)?;
    ensure!(oid.to_string() == text, "invalid stored snapshot revision");
    Ok(oid)
}
