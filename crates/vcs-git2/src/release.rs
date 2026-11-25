use std::{fs, path::Path};

use anyhow::{Context as _, ensure};
use git2::{
    ErrorCode, Oid, PushOptions, RemoteCallbacks, Repository, StatusOptions,
    WorktreePruneOptions,
};
use serde::{Deserialize, Serialize};
use vcs::{Kept, command::OwnedWorkspace};

use crate::prepare::{self, Session};

#[derive(Serialize, Deserialize)]
struct Removal {
    root: String,
    directory: String,
    revision: String,
    kept: Kept,
}

pub(super) fn workspace(
    base: &Path,
    owned: &OwnedWorkspace,
) -> anyhow::Result<Kept> {
    let name = &owned.id.0;
    prepare::require_name(name)?;
    let receipt = prepare::receipts(base, name);
    if receipt.try_exists()? {
        return finish(
            base,
            owned,
            &serde_json::from_slice(&fs::read(receipt)?)?,
        );
    }
    let session = prepare::open(base, owned)?;
    crate::install::ensure_available(
        &session.repo,
        Some("preserve-and-remove"),
    )?;
    let candidate = preserve(&session)?;
    let result = Removal {
        root: owned.root.clone(),
        directory: owned.directory.clone(),
        revision: candidate.to_string(),
        kept: publish(
            &session.repo,
            &session.record.location.branch,
            candidate,
        )?,
    };
    prepare::save(&receipt, &serde_json::to_vec(&result)?)?;
    prune(&session, candidate)?;
    Ok(result.kept)
}

fn preserve(session: &Session) -> anyhow::Result<Oid> {
    let repo = &session.repo;
    prepare::branch(repo, &session.record.location.branch)?;
    let branch = format!("refs/heads/{}", session.record.location.branch);
    crate::install::ensure_available(repo, Some("preserve-and-remove"))?;
    let candidate =
        match crate::install::recorded(repo, &branch, "preserve-and-remove")? {
            Some(oid) => oid,
            None => crate::snapshot::candidate(repo)?,
        };
    crate::features::check(repo, candidate)?;
    crate::install::run(repo, &branch, "preserve-and-remove", candidate)?;
    clean(repo, candidate)?;
    Ok(candidate)
}

fn clean(repo: &Repository, candidate: Oid) -> anyhow::Result<()> {
    ensure!(
        repo.head()?.peel_to_commit()?.id() == candidate,
        "workspace branch changed before removal"
    );
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .include_ignored(true)
        .recurse_untracked_dirs(true)
        .update_index(false);
    ensure!(
        repo.statuses(Some(&mut options))?.is_empty(),
        "workspace still contains unpreserved changes or ignored files"
    );
    Ok(())
}

fn finish(
    base: &Path,
    owned: &OwnedWorkspace,
    receipt: &Removal,
) -> anyhow::Result<Kept> {
    ensure!(owned.root == receipt.root, "session worktree path changed");
    ensure!(
        owned.directory == receipt.directory,
        "selected worktree path changed"
    );
    let path = base.join(&owned.id.0);
    if path.try_exists()? {
        let session = prepare::open(base, owned)?;
        prune(&session, Oid::from_str(&receipt.revision)?)?;
    }
    Ok(receipt.kept.clone())
}

fn prune(session: &Session, candidate: Oid) -> anyhow::Result<()> {
    let record = &session.record;
    let repository = Repository::open(&record.location.source)?;
    let worktree = repository.find_worktree(&session.name)?;
    let active = record.open(
        session
            .record
            .location
            .root
            .parent()
            .context("missing workspace parent")?,
        &session.name,
    )?;
    crate::features::check(&active, candidate)?;
    prepare::branch(&active, &record.location.branch)?;
    clean(&active, candidate)?;
    worktree.prune(Some(
        WorktreePruneOptions::new().valid(true).working_tree(true),
    ))?;
    Ok(())
}

fn publish(
    repo: &Repository,
    branch: &str,
    candidate: Oid,
) -> anyhow::Result<Kept> {
    let remote = match repo.find_remote("origin") {
        Ok(remote) => remote,
        Err(error) if error.code() == ErrorCode::NotFound => {
            return Ok(Kept::Local);
        }
        Err(error) => return Err(error.into()),
    };
    let url = remote.pushurl()?.unwrap_or(remote.url()?);
    let result = local_push(repo, url, branch, candidate);
    Ok(vcs::plan::kept(Some((
        vcs::remote::display(url),
        result.map_err(|_| {
            "Git could not publish to origin; the branch remains local"
                .to_owned()
        }),
    ))))
}

fn local_push(
    repo: &Repository,
    url: &str,
    branch: &str,
    candidate: Oid,
) -> anyhow::Result<()> {
    let path = Path::new(url);
    ensure!(
        path.is_absolute(),
        "host credentials for this remote are unavailable"
    );
    let destination = Repository::open_bare(path)?;
    let reference = format!("refs/heads/{branch}");
    let expected = destination_revision(&destination, &reference)?;
    if expected == candidate {
        return Ok(());
    }
    send(repo, path, &reference, expected, candidate)?;
    ensure!(
        destination.refname_to_id(&reference)? == candidate,
        "published revision differs"
    );
    Ok(())
}

fn destination_revision(
    repo: &Repository,
    branch: &str,
) -> anyhow::Result<Oid> {
    match repo.refname_to_id(branch) {
        Ok(oid) => Ok(oid),
        Err(error) if error.code() == ErrorCode::NotFound => Ok(Oid::ZERO_SHA1),
        Err(error) => Err(error.into()),
    }
}

fn send(
    repo: &Repository,
    path: &Path,
    reference: &str,
    expected: Oid,
    candidate: Oid,
) -> anyhow::Result<()> {
    let mut accepted = false;
    {
        let mut callbacks = RemoteCallbacks::new();
        callbacks.push_negotiation(|updates| {
            if matches!(updates, [update] if update.src() == expected && update.dst() == candidate && update.dst_refname_bytes() == reference.as_bytes()) {
                Ok(())
            } else {
                Err(git2::Error::from_str("remote revision changed"))
            }
        });
        callbacks.push_update_reference(|name, status| {
            accepted = name == reference && status.is_none();
            Ok(())
        });
        let url = path.to_str().context("non-UTF-8 remote path")?;
        repo.remote_anonymous(url)?.push(
            &[format!("{candidate}:{reference}")],
            Some(PushOptions::new().remote_callbacks(callbacks)),
        )?;
    }
    ensure!(accepted, "server refused branch publication");
    Ok(())
}
