use std::{
    fs::{self, File},
    io::Write as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, ensure};
use git2::{Oid, Reference, Repository, Worktree, WorktreeAddOptions};
use serde::{Deserialize, Serialize};
use vcs::command::{OwnedWorkspace, Prepare, Prepared};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub source: PathBuf,
    original: PathBuf,
    pub root: PathBuf,
    pub directory: PathBuf,
    pub branch: String,
    common: PathBuf,
    selected_base: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Identity {
    source: (u64, u64),
    common: (u64, u64),
    linked: (u64, u64),
    admin: (u64, u64),
}

#[derive(Serialize, Deserialize)]
pub struct Record {
    pub location: Location,
    identity: Identity,
}

pub struct Session {
    pub repo: Repository,
    pub record: Record,
    pub name: String,
    _lock: File,
}

fn root(repo: &Repository) -> anyhow::Result<PathBuf> {
    Ok(repo
        .workdir()
        .context("repository has no checkout")?
        .canonicalize()?)
}

pub fn require_name(name: &str) -> anyhow::Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 128
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
        "invalid workspace identity"
    );
    Ok(())
}

#[cfg(unix)]
fn identity(path: &Path) -> anyhow::Result<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    let data = fs::symlink_metadata(path)?;
    ensure!(data.is_dir(), "workspace ownership directory changed");
    Ok((data.dev(), data.ino()))
}

#[cfg(not(unix))]
fn identity(_path: &Path) -> anyhow::Result<(u64, u64)> {
    anyhow::bail!("workspace ownership is unverified on this filesystem");
}

impl Location {
    fn identity(&self, linked: &Repository) -> anyhow::Result<Identity> {
        Ok(Identity {
            source: identity(&self.source)?,
            common: identity(&self.common)?,
            linked: identity(&self.root)?,
            admin: identity(linked.path())?,
        })
    }

    fn prepared(&self) -> anyhow::Result<Prepared> {
        Ok(Prepared {
            repository: vcs::Repository(text(&self.source)?),
            original: vcs::WorkingCopy(text(&self.original)?),
            root: text(&self.root)?,
            directory: text(&self.directory)?,
            branch: self.branch.clone(),
        })
    }

    fn legacy_marker(&self) -> anyhow::Result<String> {
        let Prepared {
            repository,
            original,
            root,
            directory,
            branch,
        } = self.prepared()?;
        Ok(format!(
            "{}\n{}\n{root}\n{directory}\n{branch}\n",
            repository.0, original.0
        ))
    }
}

fn text(path: &Path) -> anyhow::Result<String> {
    let utf8 = path.to_str().context("non-UTF-8 workspace path")?;
    ensure!(!utf8.contains('\n'), "newline in workspace path");
    Ok(utf8.to_owned())
}

pub fn lock(common: &Path) -> anyhow::Result<File> {
    let locked = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(common.join("conductor-operations.lock"))?;
    locked
        .try_lock()
        .context("workspace operation already in progress")?;
    Ok(locked)
}

fn admin(base: &Path, name: &str) -> PathBuf {
    base.join(".conductor").join(name)
}

pub fn receipts(base: &Path, name: &str) -> PathBuf {
    admin(base, name).with_extension("removed")
}

pub fn save(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let parent = path.parent().context("workspace metadata has no parent")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn read_record(base: &Path, name: &str) -> anyhow::Result<Record> {
    Ok(serde_json::from_slice(&fs::read(admin(base, name))?)?)
}

impl Record {
    pub fn open(&self, base: &Path, name: &str) -> anyhow::Result<Repository> {
        let owner = &self.location;
        let expected = base.canonicalize()?.join(name);
        ensure!(expected == owner.root, "worktree path changed");
        ensure!(
            expected == expected.canonicalize()?,
            "worktree path changed"
        );
        let source = Repository::open(&owner.source)?;
        let linked = source.find_worktree(name)?;
        linked.validate()?;
        ensure!(linked.path() == expected, "workspace registration changed");
        ensure!(root(&source)? == owner.source, "workspace source changed");
        let repo = Repository::open(&expected)?;
        ensure!(
            repo.commondir().canonicalize()? == owner.common
                && source.commondir().canonicalize()? == owner.common,
            "workspace repository changed"
        );
        ensure!(
            self.identity == owner.identity(&repo)?,
            "workspace directory identity changed"
        );
        ensure!(
            repo.is_worktree()
                && Worktree::open_from_repository(&repo)?.name()? == Some(name),
            "workspace metadata changed"
        );
        ensure!(
            fs::read_to_string(repo.path().join("conductor-workspace"))?
                == owner.legacy_marker()?,
            "workspace ownership marker changed"
        );
        Ok(repo)
    }
}

pub fn open(
    base: &Path,
    workspace: &OwnedWorkspace,
) -> anyhow::Result<Session> {
    let name = &workspace.id.0;
    require_name(name)?;
    let record = read_record(base, name)?;
    ensure!(
        Path::new(&workspace.root) == record.location.root,
        "session worktree path changed"
    );
    ensure!(
        Path::new(&workspace.directory) == record.location.directory,
        "selected workspace path changed"
    );
    let lock = lock(&record.location.common)?;
    let repo = record.open(base, name)?;
    Ok(Session {
        repo,
        record,
        name: name.clone(),
        _lock: lock,
    })
}

fn target(source: &Repository, base: Option<&str>) -> anyhow::Result<Oid> {
    match base {
        Some(reference) => {
            Ok(source.revparse_single(reference)?.peel_to_commit()?.id())
        }
        None => Ok(source
            .head()
            .and_then(|head| head.peel_to_commit())
            .map_err(|_| vcs::Error::NotRepository)?
            .id()),
    }
}

fn new_location(
    source: &Repository,
    base: &Path,
    original: PathBuf,
    request: &Prepare,
) -> anyhow::Result<Location> {
    let repository = root(source)?;
    let relative = original.strip_prefix(&repository)?;
    let path = base.canonicalize()?.join(&request.name.0);
    let directory = path.join(relative);
    Ok(Location {
        source: repository,
        original,
        root: path,
        directory,
        branch: format!("conductor/{}", request.name.0),
        common: source.commondir().canonicalize()?,
        selected_base: request.base.clone(),
    })
}

pub fn create(base: &Path, request: &Prepare) -> anyhow::Result<Prepared> {
    let name = &request.name.0;
    require_name(name)?;
    let original = Path::new(&request.directory).canonicalize()?;
    let source = Repository::discover(&original)
        .map_err(|_| vcs::Error::NotRepository)?;
    let tip = target(&source, request.base.as_deref())?;
    crate::features::check(&source, tip)?;
    fs::create_dir_all(base)?;
    let location = new_location(&source, base, original, request)?;
    let _lock = lock(&location.common)?;
    crate::features::check(&source, tip)?;
    let record_path = admin(base, name);
    if record_path.try_exists()? {
        return existing(base, name, &location);
    }
    create_link(&source, &location, tip, name, &record_path)?;
    existing(base, name, &location)
}

fn existing(
    base: &Path,
    name: &str,
    requested: &Location,
) -> anyhow::Result<Prepared> {
    let recorded = read_record(base, name)?;
    ensure!(
        recorded.location == *requested,
        "workspace identity has a different preparation request"
    );
    branch(&recorded.open(base, name)?, &recorded.location.branch)?;
    recorded.location.prepared()
}

fn create_link(
    source: &Repository,
    location: &Location,
    tip: Oid,
    name: &str,
    record_path: &Path,
) -> anyhow::Result<()> {
    let relative = location.original.strip_prefix(&location.source)?;
    crate::snapshot::tree_at(source, tip, relative)?;
    ensure!(
        !location.root.try_exists()?,
        "workspace destination already occupied"
    );
    let full = format!("refs/heads/{}", location.branch);
    ensure!(Reference::is_valid_name(&full), "invalid workspace branch");
    fs::create_dir_all(
        record_path
            .parent()
            .context("invalid ownership directory")?,
    )?;
    let branch =
        source.reference(&full, tip, false, "prepare session workspace")?;
    source.worktree(
        name,
        &location.root,
        Some(WorktreeAddOptions::new().reference(Some(&branch))),
    )?;
    let session = Repository::open(&location.root)?;
    let expected = location.identity(&session)?;
    save(
        &session.path().join("conductor-workspace"),
        location.legacy_marker()?.as_bytes(),
    )?;
    save(
        record_path,
        &serde_json::to_vec(&Record {
            location: location.clone(),
            identity: expected,
        })?,
    )?;
    Ok(())
}

pub fn branch(repo: &Repository, name: &str) -> anyhow::Result<()> {
    ensure!(
        repo.head()?.name()? == format!("refs/heads/{name}"),
        "workspace branch changed"
    );
    Ok(())
}
