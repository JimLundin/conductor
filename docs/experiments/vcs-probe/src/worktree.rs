use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use git2::{Oid, Repository, WorktreeAddOptions, WorktreePruneOptions};

use crate::fixture;

struct Ownership {
    path: PathBuf,
    common: PathBuf,
    head: Oid,
    reference: String,
    marker: String,
}

fn remove(repo: &Repository, owner: &Ownership) -> Result<()> {
    let worktree = repo.find_worktree("session")?;
    worktree.validate()?;
    let linked = Repository::open(worktree.path())?;
    ensure!(
        fs::canonicalize(worktree.path())? == owner.path,
        "worktree path changed"
    );
    ensure!(
        fs::canonicalize(linked.commondir())? == owner.common,
        "worktree repository changed"
    );
    ensure!(
        linked.head()?.target() == Some(owner.head),
        "worktree head changed"
    );
    ensure!(
        linked.head()?.name()? == owner.reference,
        "worktree branch changed"
    );
    ensure!(
        fs::read_to_string(linked.path().join("conductor-owner"))?
            == owner.marker,
        "worktree owner changed"
    );
    ensure!(linked.statuses(None)?.is_empty(), "worktree is dirty");
    drop(linked);
    worktree.prune(Some(
        WorktreePruneOptions::new().valid(true).working_tree(true),
    ))?;
    Ok(())
}

pub fn run(root: &Path) -> Result<()> {
    let (repo, base) = fixture::init(&root.join("worktree"))?;
    let reference =
        repo.reference("refs/heads/session", base, false, "session")?;
    let path = root.join("session");
    let worktree = repo.worktree(
        "session",
        &path,
        Some(WorktreeAddOptions::new().reference(Some(&reference))),
    )?;
    worktree.validate()?;
    let linked = Repository::open(&path)?;
    let mut owner = Ownership {
        path: fs::canonicalize(&path)?,
        common: fs::canonicalize(repo.commondir())?,
        head: base,
        reference: "refs/heads/session".to_owned(),
        marker: "attempt-one".to_owned(),
    };
    fs::create_dir(path.join("nested"))?;
    let discovered = Repository::discover(path.join("nested"))?;
    ensure!(
        fs::canonicalize(
            discovered.workdir().context("discovery has no worktree")?
        )? == owner.path,
        "nested discovery found wrong worktree"
    );
    ensure!(
        fs::canonicalize(discovered.commondir())? == owner.common,
        "linked worktree lost common repository"
    );
    fs::write(linked.path().join("conductor-owner"), "other-attempt")?;
    ensure!(
        remove(&repo, &owner).is_err(),
        "foreign worktree was removed"
    );
    ensure!(path.exists(), "ownership refusal removed worktree");
    fs::write(linked.path().join("conductor-owner"), &owner.marker)?;
    repo.reference("refs/heads/unrelated", base, false, "unrelated")?;
    linked.set_head("refs/heads/unrelated")?;
    ensure!(
        remove(&repo, &owner).is_err(),
        "same-commit branch switch bypassed ownership"
    );
    linked.set_head("refs/heads/session")?;
    fixture::write(&linked, "tracked.txt", "unpublished change\n")?;
    ensure!(remove(&repo, &owner).is_err(), "dirty worktree was removed");
    linked.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))?;
    linked.set_head_detached(base)?;
    ensure!(linked.head_detached()?, "worktree did not detach");
    ensure!(
        remove(&repo, &owner).is_err(),
        "unexpected detached HEAD bypassed ownership"
    );
    "HEAD".clone_into(&mut owner.reference);
    drop(discovered);
    drop(linked);
    remove(&repo, &owner)?;
    ensure!(!path.exists(), "owned worktree remains");
    ensure!(
        repo.worktrees()?.is_empty(),
        "worktree registration remains"
    );
    Ok(())
}
