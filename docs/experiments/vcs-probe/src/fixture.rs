use std::{fs, path::Path};

use anyhow::{Context, Result};
use git2::{Oid, Repository, Signature, Time};

pub fn signature() -> Result<Signature<'static>> {
    Ok(Signature::new(
        "VCS probe",
        "probe@example.invalid",
        &Time::new(1_700_000_000, 0),
    )?)
}

pub fn init(path: &Path) -> Result<(Repository, Oid)> {
    let repo = Repository::init(path)?;
    let mut config = repo.config()?;
    config.set_str("user.name", "VCS probe")?;
    config.set_str("user.email", "probe@example.invalid")?;
    config.set_bool("core.logAllRefUpdates", true)?;
    config.set_bool("core.autocrlf", false)?;
    let base = commit(
        &repo,
        None,
        &[
            ("tracked.txt", "base\n"),
            ("delete.txt", "delete me\n"),
            ("tracked.tmp", "tracked ignored\n"),
            ("shared.txt", "shared\n"),
            (".gitignore", "*.tmp\n"),
        ],
        "base",
    )?;
    repo.reference("refs/heads/main", base, true, "seed")?;
    repo.set_head("refs/heads/main")?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;
    Ok((repo, base))
}

pub fn commit(
    repo: &Repository,
    parent: Option<Oid>,
    files: &[(&str, &str)],
    message: &str,
) -> Result<Oid> {
    let parent_commit = parent.map(|oid| repo.find_commit(oid)).transpose()?;
    let parent_tree =
        parent_commit.as_ref().map(git2::Commit::tree).transpose()?;
    let mut builder = repo.treebuilder(parent_tree.as_ref())?;
    for (name, content) in files {
        builder.insert(name, repo.blob(content.as_bytes())?, 0o100_644)?;
    }
    let tree = repo.find_tree(builder.write()?)?;
    let parents: Vec<_> = parent_commit.iter().collect();
    Ok(repo.commit(
        None,
        &signature()?,
        &signature()?,
        message,
        &tree,
        &parents,
    )?)
}

pub fn content(repo: &Repository, tree_id: Oid, name: &str) -> Result<Vec<u8>> {
    let tree = repo.find_tree(tree_id)?;
    let entry = tree.get_path(Path::new(name))?;
    Ok(repo.find_blob(entry.id())?.content().to_vec())
}

pub fn write(repo: &Repository, name: &str, content: &str) -> Result<()> {
    fs::write(
        repo.workdir()
            .context("fixture has no worktree")?
            .join(name),
        content,
    )?;
    Ok(())
}
