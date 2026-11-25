use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, ensure};
use git_workspace::features::{
    ATTRIBUTES, Attribute, Setting, accepts, configuration,
};
use git2::{
    AttrCheckFlags, AttrValue, Config, Index, IndexEntry,
    IndexEntryExtendedFlag, IndexEntryFlag, Oid, Repository,
};

#[cfg(test)]
mod tests;

pub fn check(repo: &Repository, revision: Oid) -> Result<(), vcs::Error> {
    inspect(repo, revision).map_err(|problem| {
        vcs::Error::failed("workspace preflight", &problem.to_string())
    })
}

fn inspect(repo: &Repository, revision: Oid) -> anyhow::Result<()> {
    ensure!(
        cfg!(target_os = "linux"),
        "workspace filesystem is unverified on this platform"
    );
    ensure!(
        repo.object_format() == git2::ObjectFormat::Sha1,
        "unsupported object format"
    );
    ensure!(!repo.is_shallow(), "shallow history");
    ensure!(
        repo.state() == git2::RepositoryState::Clean,
        "another repository operation is in progress"
    );
    let root = repo.workdir().context("a working copy is required")?;
    config(repo)?;
    layout(repo)?;
    let mut paths = BTreeSet::new();
    let real = repo.index()?;
    index_paths(&real, &mut paths)?;
    changes(repo, &mut paths)?;
    let commit = repo.find_commit(revision)?;
    let view = Repository::open(root)?;
    let mut candidate = Index::new()?;
    view.set_index(&mut candidate)?;
    candidate.read_tree(&commit.tree()?)?;
    index_paths(&candidate, &mut paths)?;
    for path in &paths {
        attributes(repo, path, AttrCheckFlags::FILE_THEN_INDEX)?;
        attributes(repo, path, AttrCheckFlags::INDEX_ONLY)?;
        attributes(&view, path, AttrCheckFlags::INDEX_ONLY)?;
    }
    Ok(())
}

fn layout(repo: &Repository) -> anyhow::Result<()> {
    let common = repo.commondir();
    for name in [
        "objects/info/alternates",
        "objects/info/http-alternates",
        "info/grafts",
        "shallow",
    ] {
        ensure!(
            !present(&common.join(name))?,
            "unsupported repository data at {name}"
        );
    }
    ensure!(
        repo.references_glob("refs/replace/*")?
            .next()
            .transpose()?
            .is_none(),
        "replacement history"
    );
    ensure!(!promisor_packs(common)?, "partial object database");
    Ok(())
}

fn promisor_packs(common: &Path) -> anyhow::Result<bool> {
    let path = common.join("objects/pack");
    if !present(&path)? {
        return Ok(false);
    }
    std::fs::read_dir(path)?.try_fold(false, |found, entry| {
        Ok(found
            | entry?
                .path()
                .extension()
                .is_some_and(|ext| ext == "promisor"))
    })
}

fn present(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

fn config(repo: &Repository) -> anyhow::Result<()> {
    let cfg = repo.config()?.snapshot()?;
    let mut entries = cfg.entries(None)?;
    while let Some(entry) = entries.next() {
        let name = entry?.name()?;
        ensure!(
            allowed(&cfg, name)?,
            "unsupported workspace configuration: {name}"
        );
    }
    Ok(())
}

fn allowed(cfg: &Config, name: &str) -> Result<bool, git2::Error> {
    match configuration(name) {
        Setting::Any => Ok(true),
        Setting::Disabled => cfg.get_bool(name).map(|b| !b),
        Setting::Enabled => cfg.get_bool(name),
        Setting::Exact(wanted) => cfg
            .get_bytes(name)
            .map(|actual| actual.eq_ignore_ascii_case(wanted.as_bytes())),
        Setting::Unsupported => Ok(false),
    }
}

fn index_paths(
    index: &Index,
    paths: &mut BTreeSet<PathBuf>,
) -> anyhow::Result<()> {
    ensure!(!index.has_conflicts(), "unresolved index conflicts");
    for entry in index.iter() {
        ensure!(ordinary(&entry), "unsupported index entry or submodule");
        paths.insert(path(&entry.path)?.to_owned());
    }
    Ok(())
}

fn ordinary(entry: &IndexEntry) -> bool {
    let hidden = IndexEntryExtendedFlag::INTENT_TO_ADD
        | IndexEntryExtendedFlag::SKIP_WORKTREE;
    matches!(entry.mode, 0o100_644 | 0o100_755 | 0o120_000)
        && entry.flags & IndexEntryFlag::VALID.bits() == 0
        && entry.flags_extended & hidden.bits() == 0
}

fn path(raw: &[u8]) -> anyhow::Result<&Path> {
    Ok(Path::new(
        std::str::from_utf8(raw).context("unsupported non-UTF-8 path")?,
    ))
}

fn changes(
    repo: &Repository,
    paths: &mut BTreeSet<PathBuf>,
) -> anyhow::Result<()> {
    let root = repo.workdir().context("a working copy is required")?;
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .update_index(false);
    for entry in &repo.statuses(Some(&mut options))? {
        let relative = path(entry.path_bytes())?;
        ensure!(
            !entry.path_bytes().ends_with(b"/"),
            "unsupported nested repository or untracked directory"
        );
        ensure!(!nested(root, relative)?, "nested repository");
        paths.insert(relative.to_owned());
    }
    Ok(())
}

fn nested(root: &Path, relative: &Path) -> anyhow::Result<bool> {
    let full = root.join(relative);
    if directory(&full)? {
        return Ok(present(&full.join(".git"))?);
    }
    Ok(false)
}

fn directory(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.is_dir()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

fn attributes(
    repo: &Repository,
    path: &Path,
    source: AttrCheckFlags,
) -> anyhow::Result<()> {
    for name in ATTRIBUTES {
        let attr =
            AttrValue::from_bytes(repo.get_attr_bytes(path, name, source)?);
        let value = match attr {
            AttrValue::True => Attribute::Set,
            AttrValue::False => Attribute::Unset,
            AttrValue::String(v) => Attribute::Value(v),
            AttrValue::Bytes(_) => Attribute::NonUtf8,
            AttrValue::Unspecified => Attribute::Unspecified,
        };
        ensure!(
            accepts(name, value),
            "unsupported {name} attribute on {}",
            path.display()
        );
    }
    Ok(())
}
