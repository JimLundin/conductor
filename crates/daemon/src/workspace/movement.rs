//! Imports and exports the Git portion of a session move.

use std::path::Path;

use host::movement::{Incoming, Refs, checkpoints, head};
use proto::{HostPath, SessionId, Workspace};

use super::{GitError, attach_moved, failure, preflight, text};

pub async fn export(
    source: &Workspace,
    session: SessionId,
    bundle: &Path,
    checkpoint: &str,
    known: Option<&str>,
) -> Result<Refs, GitError> {
    preflight(source, checkpoint).await?;
    let archive = bundle_path(bundle)?;
    let root = Path::new(&source.worktree.0);
    let branch = head(session);
    let prefix = checkpoints(session);
    let output = text(
        root,
        &[
            "for-each-ref",
            "--format=%(objectname) %(refname)",
            &branch,
            &prefix,
        ],
    )
    .await?;
    let manifest = Refs::listed(session, &output)?;
    same(&manifest.tip, checkpoint)?;
    let refs = format!("--glob={prefix}*");
    let pack = ["bundle", "create", archive, &branch, &refs];
    if let Some(base) = known.filter(|object| host::movement::is_object(object))
    {
        if thin_bundle(root, pack, base, session, &manifest).await {
            return Ok(manifest);
        }
        remove_failed_bundle(Path::new(archive))?;
    }
    text(root, &pack).await?;
    let listed = list(root, session, archive).await?;
    same_manifest(&listed, &manifest)?;
    Ok(manifest)
}

async fn thin_bundle(
    root: &Path,
    pack: [&str; 5],
    base: &str,
    session: SessionId,
    manifest: &Refs,
) -> bool {
    let good_base = text(root, &["cat-file", "-t", base])
        .await
        .is_ok_and(|kind| kind == "commit");
    if !good_base {
        return false;
    }
    let bundle = pack[2];
    let mut args = Vec::from(pack);
    let excluded = format!("^{base}");
    args.push(&excluded);
    thin_pack(root, &args, bundle, session, manifest)
        .await
        .is_ok()
}

async fn thin_pack(
    root: &Path,
    args: &[&str],
    bundle: &str,
    session: SessionId,
    manifest: &Refs,
) -> Result<(), GitError> {
    text(root, args).await?;
    let listed = list(root, session, bundle).await?;
    manifest.validate_bundle(&listed)
}

fn remove_failed_bundle(bundle: &Path) -> Result<(), GitError> {
    match std::fs::remove_file(bundle) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => {
            Err(failure("clearing the incomplete bundle", &err.to_string()))
        }
    }
}

pub async fn import(
    data: &Path,
    checkout: &HostPath,
    incoming: Incoming,
    bundle: &Path,
    expected: &Refs,
    previous: Option<&str>,
) -> Result<Workspace, GitError> {
    let session = incoming.session;
    let archive = bundle_path(bundle)?;
    let root = Path::new(&checkout.0);
    text(root, &["bundle", "verify", "--quiet", archive]).await?;
    let listed = list(root, session, archive).await?;
    expected.validate_bundle(&listed)?;
    let omitted = expected.omitted(&listed, session);
    verify_omitted(root, &omitted).await?;
    prepare_intent(data, checkout, incoming, &listed.tip)?;
    fetch(root, session, archive, &listed).await?;
    install_omitted(root, &omitted).await?;
    update_branch(root, session, &listed.tip, previous).await?;
    let workspace = attach_moved(data, checkout, session, &listed.tip).await?;
    preflight(&workspace, &listed.tip).await?;
    Ok(workspace)
}

fn prepare_intent(
    data: &Path,
    original: &HostPath,
    incoming: Incoming,
    tip: &str,
) -> Result<(), GitError> {
    let dir = data.join("move-intents");
    std::fs::create_dir_all(&dir).map_err(|error| intent_error(&error))?;
    let move_name = ulid::Ulid::from(incoming.id.0);
    let session_name = ulid::Ulid::from(incoming.session.0);
    let path = dir.join(format!("{session_name}-{move_name}"));
    let bytes = postcard::to_stdvec(&(incoming.from, &original.0, tip))
        .map_err(|err| failure("recording a move intent", &err.to_string()))?;
    match std::fs::read(&path) {
        Ok(found) => same_intent(&found, &bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let worktree =
                data.join("worktrees").join(session_name.to_string());
            begin_intent(&dir, &path, &worktree, &bytes)
        }
        Err(error) => Err(intent_error(&error)),
    }
}

fn begin_intent(
    dir: &Path,
    path: &Path,
    worktree: &Path,
    bytes: &[u8],
) -> Result<(), GitError> {
    match std::fs::symlink_metadata(worktree) {
        Ok(_) => {
            return Err(failure(
                "session move",
                "an unowned directory occupies the incoming worktree",
            ));
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(intent_error(&err)),
    }
    let next = dir.join(format!("{}.next", ulid::Ulid::new()));
    crate::write_private(&next, bytes)
        .map_err(|err| failure("recording a move intent", &err.to_string()))?;
    std::fs::rename(&next, path).map_err(|error| intent_error(&error))?;
    crate::sync_dir(dir).map_err(|error| intent_error(&error))
}

fn same_intent(found: &[u8], expected: &[u8]) -> Result<(), GitError> {
    if found == expected {
        Ok(())
    } else {
        Err(failure(
            "session move",
            "the worktree transfer differs from the recorded intent",
        ))
    }
}

fn intent_error(error: &std::io::Error) -> GitError {
    failure("recording a move intent", &error.to_string())
}

async fn update_branch(
    root: &Path,
    session: SessionId,
    tip: &str,
    previous: Option<&str>,
) -> Result<(), GitError> {
    let branch = head(session);
    let actual =
        text(root, &["for-each-ref", "--format=%(objectname)", &branch])
            .await?;
    if actual == tip {
        return Ok(());
    }
    host::movement::replaceable_branch(&actual, previous)?;
    text(root, &["update-ref", "--no-deref", &branch, tip, &actual])
        .await
        .map(drop)
}

async fn verify_omitted(
    root: &Path,
    omitted: &[(String, &str)],
) -> Result<(), GitError> {
    for (_, commit) in omitted {
        let kind = text(root, &["cat-file", "-t", commit]).await?;
        if kind != "commit" {
            return Err(failure(
                "session bundle",
                "a prerequisite checkpoint is not a commit",
            ));
        }
    }
    Ok(())
}

async fn install_omitted(
    root: &Path,
    omitted: &[(String, &str)],
) -> Result<(), GitError> {
    for (name, commit) in omitted {
        install_checkpoint(root, name, commit).await?;
    }
    Ok(())
}

async fn install_checkpoint(
    root: &Path,
    reference: &str,
    commit: &str,
) -> Result<(), GitError> {
    let present =
        text(root, &["for-each-ref", "--format=%(objectname)", reference])
            .await?;
    if present.is_empty() {
        text(root, &["update-ref", "--no-deref", reference, commit, ""])
            .await
            .map(drop)
    } else {
        same(&present, commit)
    }
}

async fn list(
    repository: &Path,
    session: SessionId,
    bundle: &str,
) -> Result<Refs, GitError> {
    let output = text(repository, &["bundle", "list-heads", bundle]).await?;
    Refs::listed(session, &output)
}

async fn fetch(
    repository: &Path,
    session: SessionId,
    bundle: &str,
    refs: &Refs,
) -> Result<(), GitError> {
    let mappings = refs.imports(session);
    let mut args = Vec::from([
        "fetch",
        "--quiet",
        "--atomic",
        "--no-tags",
        "--no-write-fetch-head",
        "--",
        bundle,
    ]);
    args.extend(mappings.iter().map(|mapping| mapping.refspec.as_str()));
    text(repository, &args).await?;
    for mapping in mappings {
        let actual =
            text(repository, &["rev-parse", "--verify", &mapping.destination])
                .await?;
        same(&actual, mapping.commit)?;
    }
    Ok(())
}

fn bundle_path(path: &Path) -> Result<&str, GitError> {
    path.to_str().filter(|_| path.is_absolute()).ok_or_else(|| {
        failure("session bundle", "bundle path must be absolute and UTF-8")
    })
}

fn same_manifest(actual: &Refs, expected: &Refs) -> Result<(), GitError> {
    if actual == expected {
        Ok(())
    } else {
        Err(failure(
            "session bundle",
            "announced refs differ from bundle refs",
        ))
    }
}

fn same(actual: &str, expected: &str) -> Result<(), GitError> {
    if actual == expected {
        Ok(())
    } else {
        Err(failure(
            "session bundle",
            "a transferred ref points at another commit",
        ))
    }
}

pub async fn release(
    source: &Workspace,
    checkpoint: &str,
) -> Result<(), GitError> {
    let worktree = Path::new(&source.worktree.0);
    if !worktree.try_exists().map_err(|err| {
        failure("releasing a moved worktree", &err.to_string())
    })? {
        return Ok(());
    }
    preflight(source, checkpoint).await?;
    let tip = text(worktree, &["rev-parse", "--verify", "HEAD"]).await?;
    same(&tip, checkpoint)?;
    text(
        Path::new(&source.repository.0),
        &["worktree", "remove", "--", &source.worktree.0],
    )
    .await
    .map(drop)
}
