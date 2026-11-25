use std::fs;
use std::path::Path;

use git2::{IndexAddOption, Repository, Signature};

fn repository(path: &Path) -> anyhow::Result<Repository> {
    let repo = Repository::init(path)?;
    fs::write(path.join(".gitignore"), "*.tmp\n")?;
    fs::write(path.join("delete"), "committed\n")?;
    fs::write(path.join("shared"), "original\n")?;
    fs::write(path.join("tracked.tmp"), "tracked\n")?;
    let mut index = repo.index()?;
    index.add_all(["*"], IndexAddOption::DEFAULT, None)?;
    index.add_path(Path::new("tracked.tmp"))?;
    index.write()?;
    let tree = index.write_tree()?;
    let signature = Signature::now("Conductor", "workspace@example.test")?;
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        "initial",
        &repo.find_tree(tree)?,
        &[],
    )?;
    Ok(repo)
}

#[test]
fn capture_preserves_staging_files_and_branch_while_recording_work()
-> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let repo = repository(dir.path())?;
    fs::write(dir.path().join("shared"), "staged\n")?;
    let mut index = repo.index()?;
    index.add_path(Path::new("shared"))?;
    index.write()?;
    let staged = fs::read(repo.path().join("index"))?;
    let original = repo.head()?.peel_to_commit()?.id();
    fs::write(dir.path().join("shared"), "unstaged\n")?;
    fs::write(dir.path().join("tracked.tmp"), "tracked ignored edit\n")?;
    fs::write(dir.path().join("private.tmp"), "ignored\n")?;
    fs::write(dir.path().join("new"), "new file\n")?;
    fs::remove_file(dir.path().join("delete"))?;
    let captured = repo.find_tree(super::snapshot::tree(&repo)?)?;
    assert_eq!(repo.head()?.peel_to_commit()?.id(), original);
    assert_eq!(fs::read(repo.path().join("index"))?, staged);
    assert_eq!(fs::read(dir.path().join("shared"))?, b"unstaged\n");
    for (name, expected) in [
        ("shared", "unstaged\n"),
        ("tracked.tmp", "tracked ignored edit\n"),
        ("new", "new file\n"),
    ] {
        assert_eq!(
            repo.find_blob(captured.get_path(Path::new(name))?.id())?
                .content(),
            expected.as_bytes()
        );
    }
    assert!(captured.get_name("private.tmp").is_none());
    assert!(captured.get_name("delete").is_none());
    Ok(())
}

fn opened(
    root: &Path,
) -> anyhow::Result<(crate::GitWorkspaces, vcs::command::Prepare)> {
    let _source = repository(&root.join("repo"))?;
    Ok((
        crate::GitWorkspaces::new(root.join("sessions")),
        vcs::command::Prepare {
            name: vcs::command::WorkspaceId(String::from("one")),
            directory: root.join("repo").display().to_string(),
            base: None,
        },
    ))
}

fn failed(problem: project_runtime::Failure) -> anyhow::Error {
    anyhow::anyhow!("{problem:?}")
}

#[tokio::test]
async fn removal_refuses_foreign_branch_detachment_marker_or_unpreserved_files()
-> anyhow::Result<()> {
    use project_runtime::VersionControl as _;
    use vcs::command::{OwnedWorkspace, PreserveAndRemove};
    let root = tempfile::tempdir()?;
    let (adapter, prepare) = opened(root.path())?;
    let name = prepare.name.clone();
    let prepared = adapter.prepare(prepare).await.map_err(failed)?;
    let request = PreserveAndRemove {
        workspace: OwnedWorkspace {
            root: prepared.root.clone(),
            id: name,
            directory: prepared.directory,
        },
    };
    let path = Path::new(&prepared.root);
    let repo = Repository::open(path)?;
    let tip = repo.head()?.peel_to_commit()?.id();
    let marker = repo.path().join("conductor-workspace");
    let before = fs::read(&marker)?;
    fs::write(&marker, "foreign")?;
    assert!(adapter.preserve_and_remove(request.clone()).await.is_err());
    fs::write(&marker, before)?;
    repo.reference("refs/heads/other", tip, false, "other owner")?;
    repo.set_head("refs/heads/other")?;
    assert!(adapter.preserve_and_remove(request.clone()).await.is_err());
    repo.set_head_detached(tip)?;
    assert!(adapter.preserve_and_remove(request.clone()).await.is_err());
    repo.set_head(&format!("refs/heads/{}", prepared.branch))?;
    fs::write(path.join("ignored.tmp"), "private data")?;
    assert!(adapter.preserve_and_remove(request.clone()).await.is_err());
    assert_eq!(fs::read(path.join("ignored.tmp"))?, b"private data");
    fs::remove_file(path.join("ignored.tmp"))?;
    assert_eq!(
        adapter.preserve_and_remove(request).await.map_err(failed)?,
        vcs::Kept::Local
    );
    assert!(!path.exists());
    Ok(())
}

#[tokio::test]
async fn snapshot_capture_and_install_are_separate_and_the_same_attempt_can_resume()
-> anyhow::Result<()> {
    use project_runtime::VersionControl as _;
    use vcs::command::{Capture, Install, OwnedWorkspace, SnapshotAttempt};
    let root = tempfile::tempdir()?;
    let (adapter, prepare) = opened(root.path())?;
    let id = prepare.name.clone();
    let prepared = adapter.prepare(prepare).await.map_err(failed)?;
    let request = Capture {
        workspace: OwnedWorkspace {
            root: prepared.root.clone(),
            id,
            directory: prepared.directory,
        },
        attempt: SnapshotAttempt("turn-1".into()),
    };
    let path = Path::new(&prepared.root);
    let repo = Repository::open(path)?;
    let before = repo.head()?.peel_to_commit()?.id();
    fs::write(path.join("new"), "some work")?;
    let candidate = adapter.capture(request.clone()).await.map_err(failed)?;
    assert_eq!(repo.head()?.peel_to_commit()?.id(), before);
    let install = Install {
        workspace: request.workspace.clone(),
        attempt: request.attempt.clone(),
        revision: candidate.clone(),
    };
    adapter.install(install.clone()).await.map_err(failed)?;
    assert_eq!(repo.head()?.peel_to_commit()?.id().to_string(), candidate.0);
    fs::write(path.join("new"), "later edit")?;
    assert_eq!(adapter.capture(request).await.map_err(failed)?, candidate);
    adapter.install(install).await.map_err(failed)?;
    assert_eq!(fs::read(path.join("new"))?, b"later edit");
    Ok(())
}

#[tokio::test]
async fn removal_preserves_the_revision_at_its_destination_and_replays_after_pruning()
-> anyhow::Result<()> {
    use project_runtime::VersionControl as _;
    use vcs::command::{OwnedWorkspace, PreserveAndRemove};
    let root = tempfile::tempdir()?;
    let (adapter, prepare) = opened(root.path())?;
    let name = prepare.name.clone();
    let source = Repository::open(&prepare.directory)?;
    let remote = Repository::init_bare(root.path().join("remote.git"))?;
    let target = root.path().join("remote.git");
    source.remote(
        "origin",
        target
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("fixture path"))?,
    )?;
    let prepared = adapter.prepare(prepare).await.map_err(failed)?;
    fs::write(Path::new(&prepared.root).join("leftover"), "uncommitted\n")?;
    let request = PreserveAndRemove {
        workspace: OwnedWorkspace {
            root: prepared.root.clone(),
            id: name,
            directory: prepared.directory,
        },
    };
    let kept = adapter
        .preserve_and_remove(request.clone())
        .await
        .map_err(failed)?;
    assert_eq!(
        kept,
        vcs::Kept::Pushed {
            remote: target.display().to_string()
        }
    );
    assert_eq!(
        adapter.preserve_and_remove(request).await.map_err(failed)?,
        kept
    );
    let branch = format!("refs/heads/{}", prepared.branch);
    let published = remote.find_reference(&branch)?.peel_to_commit()?;
    assert_eq!(source.refname_to_id(&branch)?, published.id());
    let id = published.tree()?.get_path(Path::new("leftover"))?.id();
    assert_eq!(remote.find_blob(id)?.content(), b"uncommitted\n");
    Ok(())
}

#[tokio::test]
async fn a_captured_turn_keeps_its_original_revision_when_the_reply_is_lost()
-> anyhow::Result<()> {
    use project_runtime::VersionControl as _;
    use vcs::command::{Capture, OwnedWorkspace, SnapshotAttempt};
    let root = tempfile::tempdir()?;
    let (adapter, prepare) = opened(root.path())?;
    let id = prepare.name.clone();
    let prepared = adapter.prepare(prepare).await.map_err(failed)?;
    let request = Capture {
        workspace: OwnedWorkspace {
            root: prepared.root.clone(),
            id,
            directory: prepared.directory,
        },
        attempt: SnapshotAttempt("turn-2".into()),
    };
    let file = Path::new(&prepared.root).join("shared");
    fs::write(&file, "first capture")?;
    let first = adapter.capture(request.clone()).await.map_err(failed)?;
    fs::write(&file, "newer checkout edits")?;
    assert_eq!(adapter.capture(request).await.map_err(failed)?, first);
    assert_eq!(fs::read(file)?, b"newer checkout edits");
    Ok(())
}
