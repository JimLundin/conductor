use std::path::Path;

use anyhow::{Context, Result, ensure};
use git2::{ErrorCode, Oid, PushOptions, RemoteCallbacks, Repository};

use crate::fixture;

pub fn run(root: &Path) -> Result<()> {
    let (repo, base) = fixture::init(&root.join("integration"))?;
    clean_merge(&repo, base)?;
    conflict(&repo, base)?;
    conditional_ref(&repo, base)?;
    transport(root, &repo, base)?;
    Ok(())
}

fn clean_merge(repo: &Repository, base: Oid) -> Result<()> {
    let left =
        fixture::commit(repo, Some(base), &[("left.txt", "left\n")], "left")?;
    let right = fixture::commit(
        repo,
        Some(base),
        &[("right.txt", "right\n")],
        "right",
    )?;
    let left_commit = repo.find_commit(left)?;
    let right_commit = repo.find_commit(right)?;
    let mut merged = repo.merge_commits(&left_commit, &right_commit, None)?;
    ensure!(!merged.has_conflicts(), "disjoint merge conflicted");
    let tree = repo.find_tree(merged.write_tree_to(repo)?)?;
    ensure!(
        fixture::content(repo, tree.id(), "left.txt")? == b"left\n",
        "merge lost left content"
    );
    ensure!(
        fixture::content(repo, tree.id(), "right.txt")? == b"right\n",
        "merge lost right content"
    );
    let merge_id = repo.commit(
        None,
        &fixture::signature()?,
        &fixture::signature()?,
        "merge",
        &tree,
        &[&left_commit, &right_commit],
    )?;
    ensure!(
        repo.find_commit(merge_id)?.parent_count() == 2,
        "merge lost parents"
    );
    let diff = repo.diff_tree_to_tree(
        Some(&repo.find_commit(base)?.tree()?),
        Some(&tree),
        None,
    )?;
    let paths: Vec<_> = diff
        .deltas()
        .map(|delta| delta.new_file().path().map(Path::to_path_buf))
        .collect();
    ensure!(
        paths == [Some("left.txt".into()), Some("right.txt".into())],
        "tree diff has unexpected paths"
    );
    ensure!(
        repo.head()?.target() == Some(base),
        "merge changed source HEAD"
    );
    Ok(())
}

fn conflict(repo: &Repository, base: Oid) -> Result<()> {
    let left = fixture::commit(
        repo,
        Some(base),
        &[("shared.txt", "left\n")],
        "conflicting left",
    )?;
    let right = fixture::commit(
        repo,
        Some(base),
        &[("shared.txt", "right\n")],
        "conflicting right",
    )?;
    let index = repo.merge_commits(
        &repo.find_commit(left)?,
        &repo.find_commit(right)?,
        None,
    )?;
    ensure!(
        index.has_conflicts(),
        "conflicting integration was accepted"
    );
    let conflict = index
        .conflicts()?
        .next()
        .context("missing conflicting path")??;
    ensure!(
        conflict.our.context("missing ours")?.path == b"shared.txt",
        "wrong conflicting path"
    );
    ensure!(
        repo.head()?.target() == Some(base),
        "conflicting merge moved branch"
    );
    Ok(())
}

fn conditional_ref(repo: &Repository, base: Oid) -> Result<()> {
    let candidate = fixture::commit(
        repo,
        Some(base),
        &[("candidate.txt", "candidate\n")],
        "candidate",
    )?;
    let competing = fixture::commit(
        repo,
        Some(base),
        &[("competing.txt", "competing\n")],
        "competing",
    )?;
    repo.reference("refs/heads/conditional", base, false, "prepare")?;
    repo.reference_matching(
        "refs/heads/conditional",
        competing,
        true,
        base,
        "competing writer",
    )?;
    let rejected = repo.reference_matching(
        "refs/heads/conditional",
        candidate,
        true,
        base,
        "stale writer",
    );
    ensure!(
        matches!(rejected, Err(error) if error.code() == ErrorCode::Modified),
        "stale branch update was not refused"
    );
    ensure!(
        repo.refname_to_id("refs/heads/conditional")? == competing,
        "competing branch update lost"
    );
    Ok(())
}

struct PushReport {
    transport: std::result::Result<(), git2::Error>,
    rejected_refs: Vec<String>,
}

fn push(repo: &Repository, refspec: &str) -> Result<PushReport> {
    let mut rejected_refs = Vec::new();
    let mut callbacks = RemoteCallbacks::new();
    callbacks.push_update_reference(|reference, status| {
        if let Some(message) = status {
            rejected_refs.push(format!("{reference}: {message}"));
        }
        Ok(())
    });
    let mut options = PushOptions::new();
    options.remote_callbacks(callbacks);
    let transport = repo
        .find_remote("origin")?
        .push(&[refspec], Some(&mut options));
    drop(options);
    Ok(PushReport {
        transport,
        rejected_refs,
    })
}

fn transport(root: &Path, repo: &Repository, base: Oid) -> Result<()> {
    let remote_path = root.join("remote.git");
    let remote = Repository::init_bare(&remote_path)?;
    remote.set_head("refs/heads/main")?;
    let url = remote_path.to_str().context("non-UTF8 fixture path")?;
    repo.remote("origin", url)?;
    let initial = push(repo, "refs/heads/main:refs/heads/main")?;
    initial.transport?;
    ensure!(initial.rejected_refs.is_empty(), "initial push rejected");
    let follower = Repository::clone(url, root.join("follower"))?;
    ensure!(
        follower.refname_to_id("HEAD")? == base,
        "clone missed initial commit"
    );
    let next = fixture::commit(
        repo,
        Some(base),
        &[("published.txt", "published\n")],
        "publish",
    )?;
    repo.reference("refs/heads/publish", next, false, "publish")?;
    let accepted = push(repo, "refs/heads/publish:refs/heads/main")?;
    accepted.transport?;
    ensure!(accepted.rejected_refs.is_empty(), "fast-forward rejected");
    ensure!(
        remote.refname_to_id("HEAD")? == next,
        "remote head did not advance"
    );
    follower.find_remote("origin")?.fetch(
        &["refs/heads/main:refs/remotes/origin/main"],
        None,
        None,
    )?;
    ensure!(
        follower.refname_to_id("refs/remotes/origin/main")? == next,
        "fetch missed updated ref"
    );
    ensure!(
        follower
            .find_commit(next)?
            .tree()?
            .get_name("published.txt")
            .is_some(),
        "fetch missed objects"
    );
    let divergent = fixture::commit(
        repo,
        Some(base),
        &[("divergent.txt", "divergent\n")],
        "divergent",
    )?;
    repo.reference("refs/heads/divergent", divergent, false, "divergent")?;
    let rejected = push(repo, "refs/heads/divergent:refs/heads/main")?;
    ensure!(
        matches!(rejected.transport, Err(error) if error.code() == ErrorCode::NotFastForward),
        "expected local non-fast-forward transport rejection"
    );
    ensure!(
        remote.refname_to_id("HEAD")? == next,
        "rejected push overwrote remote"
    );
    Ok(())
}
