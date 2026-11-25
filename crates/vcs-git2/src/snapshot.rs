use std::path::Path;

use anyhow::{Context as _, ensure};
use git2::{ErrorCode, Index, IndexAddOption, Oid, Repository, Signature};

pub fn tree(repo: &Repository) -> Result<Oid, vcs::Error> {
    scratch_tree(repo).map_err(super::failure)
}

fn scratch_tree(repo: &Repository) -> anyhow::Result<Oid> {
    let session = Repository::open(
        repo.workdir().context("workspace has no directory")?,
    )?;
    let base = session.head()?.peel_to_tree()?;
    let mut index = Index::new()?;
    session.set_index(&mut index)?;
    index.read_tree(&base)?;
    index.update_all(["*"], None)?;
    index.add_all(["*"], IndexAddOption::DEFAULT, None)?;
    Ok(index.write_tree_to(&session)?)
}

pub fn candidate(repo: &Repository) -> anyhow::Result<Oid> {
    let parent = repo.head()?.peel_to_commit()?;
    crate::features::check(repo, parent.id())?;
    let files = tree(repo)?;
    if files == parent.tree_id() {
        return Ok(parent.id());
    }
    let signature = Signature::now("Conductor", "conductor@local")?;
    Ok(repo.commit(
        None,
        &signature,
        &signature,
        "Conductor: snapshot session work",
        &repo.find_tree(files)?,
        &[&parent],
    )?)
}

pub fn retained(
    repo: &Repository,
    owner: &str,
    revision: &str,
) -> anyhow::Result<Oid> {
    let oid = Oid::from_str(revision)?;
    ensure!(
        revision == oid.to_string(),
        "expected a full snapshot revision"
    );
    let reference = format!("refs/conductor/{owner}/{oid}");
    ensure!(
        repo.refname_to_id(&reference)? == oid,
        "recorded snapshot revision changed"
    );
    Ok(oid)
}

pub fn retain(repo: &Repository, owner: &str, oid: Oid) -> anyhow::Result<()> {
    let reference = format!("refs/conductor/{owner}/{oid}");
    match repo.find_reference(&reference) {
        Ok(found) => {
            ensure!(
                found.target() == Some(oid),
                "snapshot identity already reserved"
            );
        }
        Err(error) if error.code() == ErrorCode::NotFound => {
            repo.reference(
                &reference,
                oid,
                false,
                "retain workspace snapshot",
            )?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub fn tree_at(
    repo: &Repository,
    revision: Oid,
    path: &Path,
) -> anyhow::Result<()> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    let tree = repo.find_commit(revision)?.tree()?;
    let entry = tree
        .get_path(path)
        .context("selected subdirectory must exist in the starting revision")?;
    ensure!(
        entry.kind() == Some(git2::ObjectType::Tree),
        "selected path is not a directory in the starting revision"
    );
    Ok(())
}

pub fn captured(
    session: &crate::prepare::Session,
    attempt: &str,
) -> anyhow::Result<Oid> {
    let repo = &session.repo;
    let name = &session.name;
    let digest = blake3::hash(attempt.as_bytes()).to_hex();
    let reference = format!("refs/conductor/captured/{name}/{digest}");
    match repo.find_reference(&reference) {
        Ok(found) => {
            let oid = found
                .target()
                .context("capture operation reference is symbolic")?;
            retained(repo, name, &oid.to_string())
        }
        Err(e) if e.code() == git2::ErrorCode::NotFound => {
            crate::install::ensure_available(repo, Some(attempt))?;
            crate::prepare::branch(repo, &session.record.location.branch)?;
            let captured = candidate(repo)?;
            retain(repo, name, captured)?;
            repo.reference(
                &reference,
                captured,
                false,
                "record workspace capture",
            )?;
            Ok(captured)
        }
        Err(error) => Err(error.into()),
    }
}
