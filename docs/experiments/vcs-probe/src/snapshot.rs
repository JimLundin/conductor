use std::{fs, path::Path};

use anyhow::{Context, Result, ensure};
use git2::{Index, IndexAddOption, Oid, Repository};

use crate::fixture;

fn capture(path: &Path) -> Result<Oid> {
    let repo = Repository::open(path)?;
    let parent = repo.head()?.peel_to_commit()?;
    let mut index = Index::new()?;
    repo.set_index(&mut index)?;
    index.read_tree(&parent.tree()?)?;
    index.add_all(["*"], IndexAddOption::DEFAULT, None)?;
    let tree = repo.find_tree(index.write_tree_to(&repo)?)?;
    Ok(repo.commit(
        None,
        &fixture::signature()?,
        &fixture::signature()?,
        "snapshot",
        &tree,
        &[&parent],
    )?)
}

fn stage(repo: &Repository) -> Result<()> {
    fixture::write(repo, "tracked.txt", "staged\n")?;
    let mut index = repo.index()?;
    index.add_path(Path::new("tracked.txt"))?;
    index.write()?;
    Ok(())
}

fn dirty(repo: &Repository) -> Result<()> {
    fixture::write(repo, "tracked.txt", "unstaged\n")?;
    fixture::write(repo, "tracked.tmp", "changed ignored tracked\n")?;
    fixture::write(repo, "untracked.txt", "untracked\n")?;
    fixture::write(repo, "private.tmp", "ignored\n")?;
    fs::remove_file(
        repo.workdir()
            .context("missing worktree")?
            .join("delete.txt"),
    )?;
    Ok(())
}

fn files(path: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    [
        "tracked.txt",
        "tracked.tmp",
        "untracked.txt",
        "private.tmp",
        "shared.txt",
        ".gitignore",
    ]
    .into_iter()
    .map(|name| Ok((name.to_owned(), fs::read(path.join(name))?)))
    .collect()
}

pub fn run(root: &Path) -> Result<()> {
    let path = root.join("snapshot");
    let (repo, base) = fixture::init(&path)?;
    stage(&repo)?;
    dirty(&repo)?;
    let before_head = fs::read(repo.path().join("HEAD"))?;
    let before_index = fs::read(repo.path().join("index"))?;
    let before_files = files(&path)?;
    let snapshot = capture(&path)?;
    ensure!(
        repo.head()?.target() == Some(base),
        "snapshot moved source branch"
    );
    ensure!(
        fs::read(repo.path().join("HEAD"))? == before_head,
        "snapshot changed HEAD bytes"
    );
    ensure!(
        fs::read(repo.path().join("index"))? == before_index,
        "snapshot changed real index"
    );
    ensure!(
        files(&path)? == before_files,
        "snapshot changed source files"
    );
    ensure!(
        !path.join("delete.txt").exists(),
        "snapshot restored source deletion"
    );
    verify_candidate(&repo, snapshot)?;
    verify_staging(&repo, base, snapshot)?;
    Ok(())
}

fn verify_candidate(repo: &Repository, snapshot: Oid) -> Result<()> {
    let tree = repo.find_commit(snapshot)?.tree()?;
    ensure!(
        fixture::content(repo, tree.id(), "tracked.txt")? == b"unstaged\n",
        "snapshot missed worktree content"
    );
    ensure!(
        fixture::content(repo, tree.id(), "tracked.tmp")?
            == b"changed ignored tracked\n",
        "snapshot missed ignored tracked change"
    );
    ensure!(
        fixture::content(repo, tree.id(), "untracked.txt")? == b"untracked\n",
        "snapshot missed untracked file"
    );
    ensure!(
        tree.get_name("private.tmp").is_none(),
        "snapshot included ignored untracked file"
    );
    ensure!(
        tree.get_name("delete.txt").is_none(),
        "snapshot missed deletion"
    );
    Ok(())
}

fn verify_staging(repo: &Repository, base: Oid, snapshot: Oid) -> Result<()> {
    let head_tree = repo.find_commit(base)?.tree_id();
    let candidate_tree = repo.find_commit(snapshot)?.tree_id();
    let staged_tree = repo.index()?.write_tree()?;
    ensure!(
        !vcs_probe_core::preserves_staging(
            staged_tree == head_tree,
            staged_tree == candidate_tree
        ),
        "partial staging was accepted"
    );
    ensure!(
        vcs_probe_core::preserves_staging(true, false),
        "clean index was refused"
    );
    ensure!(
        vcs_probe_core::preserves_staging(false, true),
        "fully staged snapshot was refused"
    );
    ensure!(
        fixture::content(repo, staged_tree, "tracked.txt")? == b"staged\n",
        "staged content changed"
    );
    Ok(())
}
