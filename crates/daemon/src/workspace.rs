//! Daemon-owned Git worktrees and immutable checkpoints. The project's HEAD
//! and index are never used as checkpoint storage.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use n0_future::StreamExt;
use proto::{
    Change, Checkpoint, CheckpointDiff, CheckpointFile, CheckpointId, DiffExtent, FilePatch,
    HostPath, SessionId, Workspace,
};
use tokio::io::AsyncReadExt;
use tokio::process::Child;

const CAPTURE: u64 = 65537;
const PATCH_LIMIT: usize = 65536;
const TOTAL_LIMIT: usize = 262144;
const FILE_LIMIT: usize = 100;

#[derive(Debug, thiserror::Error)]
pub(crate) enum GitError {
    #[error("the selected directory is not a Git repository with a committed HEAD")]
    NotRepository,
    #[error("{0}")]
    Failed(String),
}

struct Capture {
    status: ExitStatus,
    bytes: Vec<u8>,
    error: String,
}

fn failure(context: &str, error: &str) -> GitError {
    GitError::Failed(format!("{context}: {error}"))
}

fn same(actual: &str, expected: &str, context: &str) -> Result<(), GitError> {
    match actual.cmp(expected) {
        Ordering::Equal => Ok(()),
        Ordering::Less | Ordering::Greater => Err(failure(context, "metadata differs")),
    }
}

fn git(path: &Path, args: &[&str]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    command.current_dir(path).args([
        "--no-pager",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
    ]);
    for key in std::env::vars_os()
        .map(|(key, _)| key)
        .filter(|key| key.to_string_lossy().starts_with("GIT_"))
    {
        command.env_remove(key);
    }
    command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

async fn capture(mut child: Child) -> Result<Capture, GitError> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| failure("git", "missing stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| failure("git", "missing stderr"))?;
    let error = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr
            .take(8192)
            .read_to_end(&mut bytes)
            .await
            .map(|_| String::from_utf8_lossy(&bytes).into_owned())
    });
    let mut bytes = Vec::new();
    stdout
        .take(CAPTURE)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| failure("reading git stdout", &error.to_string()))?;
    if let Ordering::Greater = bytes.len().cmp(&PATCH_LIMIT) {
        child
            .kill()
            .await
            .map_err(|error| failure("stopping oversized git output", &error.to_string()))?;
    }
    let status = child
        .wait()
        .await
        .map_err(|error| failure("waiting for git", &error.to_string()))?;
    let error = error
        .await
        .map_err(|error| failure("reading git stderr", &error.to_string()))?
        .map_err(|error| failure("reading git stderr", &error.to_string()))?;
    Ok(Capture {
        status,
        bytes,
        error,
    })
}

async fn execute(command: &mut tokio::process::Command) -> Result<Capture, GitError> {
    let child = command
        .spawn()
        .map_err(|error| failure("starting git", &error.to_string()))?;
    tokio::time::timeout(Duration::from_secs(120), capture(child))
        .await
        .map_err(|error| failure("git timeout", &error.to_string()))?
}

fn checked(output: Capture, context: &str) -> Result<Vec<u8>, GitError> {
    match output.bytes.len().cmp(&PATCH_LIMIT) {
        Ordering::Greater => Err(failure(context, "Git output exceeded the capture limit")),
        Ordering::Less | Ordering::Equal => match output.status.success().then_some(()) {
            Some(()) => Ok(output.bytes),
            None => Err(failure(context, &output.error)),
        },
    }
}

async fn text(path: &Path, args: &[&str]) -> Result<String, GitError> {
    let output = execute(&mut git(path, args)).await?;
    String::from_utf8(checked(output, args.first().copied().unwrap_or("git"))?)
        .map(|text| text.trim_end_matches('\n').to_owned())
        .map_err(|error| failure("Git path or metadata is not UTF-8", &error.to_string()))
}

fn canonical(path: &Path) -> Result<PathBuf, GitError> {
    std::fs::canonicalize(path).map_err(|error| {
        failure(
            &format!("canonicalizing {}", path.display()),
            &error.to_string(),
        )
    })
}

fn host(path: &Path) -> Result<HostPath, GitError> {
    match path.to_str() {
        Some(path) => match path.contains('\n').then_some(()) {
            Some(()) => Err(failure(
                "Git workspace path",
                "newline paths are unsupported",
            )),
            None => Ok(HostPath(String::from(path))),
        },
        None => Err(failure(
            "Git workspace path",
            "non-UTF-8 paths are unsupported",
        )),
    }
}

fn marker(workspace: &Workspace) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n",
        workspace.repository.0,
        workspace.original.0,
        workspace.worktree.0,
        workspace.directory.0,
        workspace.branch
    )
}

async fn git_dir(path: &Path) -> Result<PathBuf, GitError> {
    canonical(Path::new(
        &text(path, &["rev-parse", "--absolute-git-dir"]).await?,
    ))
}

async fn validate(workspace: &Workspace) -> Result<PathBuf, GitError> {
    let path = Path::new(&workspace.worktree.0);
    let root = canonical(path)?;
    let original = Path::new(&workspace.original.0);
    let repository = canonical(Path::new(&workspace.repository.0))?;
    let relative = original
        .strip_prefix(&repository)
        .map_err(|error| failure("original directory outside repository", &error.to_string()))?;
    let session = workspace
        .branch
        .strip_prefix("conductor/")
        .ok_or_else(|| failure("workspace branch", "not owned by Conductor"))?;
    let _ = session
        .parse::<ulid::Ulid>()
        .map_err(|error| failure("workspace branch", &error.to_string()))?;
    let expected_worktree = root
        .parent()
        .ok_or_else(|| failure("workspace", "missing parent"))?
        .join(session);
    same(&host(&root)?.0, &workspace.worktree.0, "canonical worktree")?;
    same(
        &workspace.directory.0,
        &host(&root.join(relative))?.0,
        "selected directory metadata",
    )?;
    same(
        &host(&expected_worktree)?.0,
        &workspace.worktree.0,
        "session worktree name",
    )?;
    let top = text(path, &["rev-parse", "--show-toplevel"]).await?;
    let branch = text(path, &["symbolic-ref", "HEAD"]).await?;
    let common = canonical(Path::new(
        &text(
            path,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await?,
    ))?;
    let source_common = canonical(Path::new(
        &text(
            &repository,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await?,
    ))?;
    let dir = git_dir(path).await?;
    let recorded = std::fs::read_to_string(dir.join("conductor-workspace"))
        .map_err(|error| failure("reading workspace ownership", &error.to_string()))?;
    same(&top, &workspace.worktree.0, "worktree root")?;
    same(
        &branch,
        &format!("refs/heads/{}", workspace.branch),
        "owned branch",
    )?;
    same(
        &host(&common)?.0,
        &host(&source_common)?.0,
        "common repository",
    )?;
    same(&recorded, &marker(workspace), "ownership metadata")?;
    match dir.starts_with(common.join("worktrees")).then_some(()) {
        Some(()) => Ok(dir),
        None => Err(failure(
            "workspace",
            "Git directory is outside repository worktrees",
        )),
    }
}

pub(crate) async fn create(
    data_dir: &Path,
    original: &HostPath,
    session: SessionId,
) -> Result<Workspace, GitError> {
    let original_path = canonical(Path::new(&original.0))?;
    let probe = execute(&mut git(&original_path, &["rev-parse", "--show-toplevel"])).await?;
    match probe.status.success().then_some(()) {
        None => Err(GitError::NotRepository),
        Some(()) => {
            let repository = canonical(Path::new(
                &String::from_utf8(checked(probe, "repository root")?)
                    .map_err(|error| failure("repository root", &error.to_string()))?
                    .trim_end_matches('\n'),
            ))?;
            let head = execute(&mut git(
                &repository,
                &["rev-parse", "--verify", "HEAD^{commit}"],
            ))
            .await?;
            match head.status.success().then_some(()) {
                None => Err(GitError::NotRepository),
                Some(()) => {
                    let head = String::from_utf8(checked(head, "committed HEAD")?)
                        .map_err(|error| failure("committed HEAD", &error.to_string()))?;
                    let relative = original_path
                        .strip_prefix(&repository)
                        .map_err(|error| failure("selected subdirectory", &error.to_string()))?;
                    match host(relative)?.0.as_str() {
                        "" => Ok(()),
                        relative => {
                            let kind = text(
                                &repository,
                                &["cat-file", "-t", &format!("{}:{relative}", head.trim())],
                            )
                            .await?;
                            same(
                                &kind,
                                "tree",
                                "selected subdirectory must exist in committed HEAD",
                            )
                        }
                    }?;
                    let parent = data_dir.join("worktrees");
                    std::fs::create_dir_all(&parent)
                        .map_err(|error| failure("creating worktree parent", &error.to_string()))?;
                    let path = canonical(&parent)?.join(ulid::Ulid::from(session.0).to_string());
                    let relative = original_path
                        .strip_prefix(&repository)
                        .map_err(|error| failure("selected subdirectory", &error.to_string()))?;
                    let workspace = Workspace {
                        repository: host(&repository)?,
                        original: host(&original_path)?,
                        worktree: host(&path)?,
                        directory: host(&path.join(relative))?,
                        branch: format!("conductor/{}", ulid::Ulid::from(session.0)),
                    };
                    match std::fs::symlink_metadata(&path) {
                        Ok(_) => {
                            let _ = validate(&workspace).await?;
                            Ok(workspace)
                        }
                        Err(error) => match error.kind() {
                            std::io::ErrorKind::NotFound => {
                                let _ = text(
                                    &repository,
                                    &[
                                        "worktree",
                                        "add",
                                        "-b",
                                        &workspace.branch,
                                        &workspace.worktree.0,
                                        head.trim(),
                                    ],
                                )
                                .await?;
                                let dir = git_dir(&path).await?;
                                let mut ownership = std::fs::OpenOptions::new()
                                    .write(true)
                                    .create_new(true)
                                    .open(dir.join("conductor-workspace"))
                                    .map_err(|error| {
                                        failure("recording worktree ownership", &error.to_string())
                                    })?;
                                std::io::Write::write_all(
                                    &mut ownership,
                                    marker(&workspace).as_bytes(),
                                )
                                .map_err(|error| {
                                    failure("recording worktree ownership", &error.to_string())
                                })?;
                                let _ = validate(&workspace).await?;
                                Ok(workspace)
                            }
                            _ => Err(failure("worktree path", &error.to_string())),
                        },
                    }
                }
            }
        }
    }
}

fn reference(workspace: &Workspace, checkpoint: CheckpointId) -> Result<String, GitError> {
    let session = workspace
        .branch
        .strip_prefix("conductor/")
        .ok_or_else(|| failure("checkpoint", "not an owned branch"))?;
    Ok(format!("refs/conductor/{session}/{}", checkpoint.0))
}

async fn owned_commit(workspace: &Workspace, commit: &str) -> Result<(), GitError> {
    let session = workspace
        .branch
        .strip_prefix("conductor/")
        .ok_or_else(|| failure("checkpoint", "not an owned branch"))?;
    let refs = text(
        Path::new(&workspace.worktree.0),
        &[
            "for-each-ref",
            "--format=%(objectname)",
            "--count=1",
            &format!("--points-at={commit}"),
            &format!("refs/conductor/{session}/"),
        ],
    )
    .await?;
    match refs.lines().any(|oid| oid == commit).then_some(()) {
        Some(()) => Ok(()),
        None => Err(failure(
            "checkpoint",
            "commit is not an immutable checkpoint of this workspace",
        )),
    }
}

async fn snapshot_index(
    workspace: &Workspace,
    checkpoint: CheckpointId,
    previous: Option<&str>,
    index: &Path,
) -> Result<String, GitError> {
    let path = Path::new(&workspace.worktree.0);
    let parent = match previous {
        Some(commit) => {
            owned_commit(workspace, commit).await?;
            String::from(commit)
        }
        None => text(path, &["rev-parse", "HEAD"]).await?,
    };
    let _ = checked(
        execute(git(path, &["read-tree", &parent]).env("GIT_INDEX_FILE", index)).await?,
        "initializing checkpoint index",
    )?;
    let _ = checked(
        execute(git(path, &["add", "-A", "--", "."]).env("GIT_INDEX_FILE", index)).await?,
        "staging checkpoint",
    )?;
    let tree = String::from_utf8(checked(
        execute(git(path, &["write-tree"]).env("GIT_INDEX_FILE", index)).await?,
        "checkpoint tree",
    )?)
    .map_err(|error| failure("checkpoint tree", &error.to_string()))?;
    let reference = reference(workspace, checkpoint)?;
    let existing = execute(&mut git(path, &["rev-parse", "--verify", &reference])).await?;
    match existing.status.success().then_some(()) {
        Some(()) => {
            let commit = String::from_utf8(checked(existing, "existing checkpoint")?)
                .map_err(|error| failure("existing checkpoint", &error.to_string()))?
                .trim()
                .to_owned();
            let actual_tree = text(path, &["rev-parse", &format!("{commit}^{{tree}}")]).await?;
            let actual_parent = text(path, &["rev-parse", &format!("{commit}^")]).await?;
            same(
                &actual_tree,
                tree.trim(),
                "checkpoint id already records a different tree",
            )?;
            same(
                &actual_parent,
                &parent,
                "checkpoint id already records a different parent",
            )?;
            Ok(commit)
        }
        None => {
            let commit = text_commit(path, tree.trim(), &parent, checkpoint).await?;
            let _ = text(path, &["update-ref", &reference, &commit, ""]).await?;
            Ok(commit)
        }
    }
}

async fn text_commit(
    path: &Path,
    tree: &str,
    parent: &str,
    checkpoint: CheckpointId,
) -> Result<String, GitError> {
    let output = execute(
        git(
            path,
            &[
                "commit-tree",
                tree,
                "-p",
                parent,
                "-m",
                &format!("Conductor checkpoint {}", checkpoint.0),
            ],
        )
        .env("GIT_AUTHOR_NAME", "Conductor")
        .env("GIT_AUTHOR_EMAIL", "conductor@localhost")
        .env("GIT_COMMITTER_NAME", "Conductor")
        .env("GIT_COMMITTER_EMAIL", "conductor@localhost"),
    )
    .await?;
    String::from_utf8(checked(output, "creating checkpoint commit")?)
        .map(|text| text.trim().to_owned())
        .map_err(|error| failure("checkpoint commit", &error.to_string()))
}

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn create(dir: &Path) -> Result<Scratch, GitError> {
        let path = dir.join(format!("conductor-index-{}", ulid::Ulid::new()));
        std::fs::create_dir(&path).map_err(|error| {
            failure("creating checkpoint scratch directory", &error.to_string())
        })?;
        Ok(Scratch { path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // This directory was created exclusively by us. Removing it also
        // handles a lock left by a cancelled Git subprocess; never follows
        // symlinks and never traverses the repository's working files.
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            tracing::warn!(
                "removing checkpoint scratch {}: {error}",
                self.path.display()
            );
        }
    }
}

pub(crate) async fn snapshot(
    workspace: &Workspace,
    session: SessionId,
    checkpoint: CheckpointId,
    previous: Option<&str>,
) -> Result<String, GitError> {
    let dir = validate(workspace).await?;
    same(
        &workspace.branch,
        &format!("conductor/{}", ulid::Ulid::from(session.0)),
        "session owning workspace",
    )?;
    let scratch = Scratch::create(&dir)?;
    snapshot_index(workspace, checkpoint, previous, &scratch.path.join("index")).await
}

pub(crate) async fn preflight(workspace: &Workspace, commit: &str) -> Result<(), GitError> {
    let _ = validate(workspace).await?;
    owned_commit(workspace, commit).await
}

pub(crate) async fn restore(workspace: &Workspace, commit: &str) -> Result<(), GitError> {
    preflight(workspace, commit).await?;
    let path = Path::new(&workspace.worktree.0);
    let _ = text(
        path,
        &[
            "restore",
            &format!("--source={commit}"),
            "--staged",
            "--worktree",
            "--",
            ".",
        ],
    )
    .await?;
    let _ = text(path, &["clean", "-q", "-f", "-d", "--", "."]).await?;
    Ok(())
}

struct Entry {
    path: String,
    change: Change,
}

fn entry(parts: &[&str]) -> Result<Entry, GitError> {
    let status = parts
        .first()
        .copied()
        .ok_or_else(|| failure("diff metadata", "missing status"))?;
    let path = parts
        .get(1)
        .copied()
        .ok_or_else(|| failure("diff metadata", "missing path"))?;
    let change = match status {
        "A" => Change::Write,
        "M" | "T" => Change::Edit,
        "D" => Change::Delete,
        _ => Change::Edit,
    };
    Ok(Entry {
        path: String::from(path),
        change,
    })
}

async fn patch(
    workspace: &Workspace,
    checkpoint: &Checkpoint,
    parent: &Checkpoint,
    entry: Entry,
) -> Result<CheckpointFile, GitError> {
    let output = execute(&mut git(
        Path::new(&workspace.worktree.0),
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--no-color",
            &parent.commit,
            &checkpoint.commit,
            "--",
            &format!(":(literal){}", entry.path),
        ],
    ))
    .await?;
    let patch = match output.bytes.len().cmp(&PATCH_LIMIT) {
        Ordering::Greater => FilePatch::TooLarge,
        Ordering::Less | Ordering::Equal => {
            match String::from_utf8(checked(output, "checkpoint patch")?) {
                Ok(text) => match text.contains("\nBinary files ").then_some(()) {
                    Some(()) => FilePatch::Binary,
                    None => FilePatch::Text(text),
                },
                Err(error) => FilePatch::Unsupported {
                    reason: format!("patch is not UTF-8: {error}"),
                },
            }
        }
    };
    Ok(CheckpointFile {
        path: entry.path,
        change: entry.change,
        patch,
    })
}

pub(crate) async fn diff(
    workspace: &Workspace,
    checkpoint: &Checkpoint,
    parent: &Checkpoint,
) -> Result<CheckpointDiff, GitError> {
    let _ = validate(workspace).await?;
    let path = Path::new(&workspace.worktree.0);
    let checkpoint_ref = reference(workspace, checkpoint.id)?;
    let parent_ref = reference(workspace, parent.id)?;
    let actual = text(path, &["rev-parse", "--verify", &checkpoint_ref]).await?;
    let actual_parent = text(path, &["rev-parse", "--verify", &parent_ref]).await?;
    same(&actual, &checkpoint.commit, "immutable checkpoint ref")?;
    same(&actual_parent, &parent.commit, "immutable parent ref")?;
    let output = execute(&mut git(
        path,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--name-status",
            "-z",
            &parent_ref,
            &checkpoint_ref,
            "--",
        ],
    ))
    .await?;
    match output.bytes.len().cmp(&PATCH_LIMIT) {
        Ordering::Greater => Ok(CheckpointDiff {
            checkpoint: checkpoint.id,
            parent: parent.id,
            files: Vec::new(),
            extent: DiffExtent::Truncated,
        }),
        Ordering::Less | Ordering::Equal => {
            let metadata = String::from_utf8(checked(output, "checkpoint file list")?)
                .map_err(|error| failure("diff contains non-UTF-8 paths", &error.to_string()))?;
            let parts = metadata
                .trim_end_matches('\0')
                .split('\0')
                .collect::<Vec<&str>>();
            let entries = match metadata.as_str() {
                "" => Vec::new(),
                _ => parts
                    .chunks(2)
                    .map(entry)
                    .collect::<Result<Vec<Entry>, GitError>>()?,
            };
            let extent = match entries.len().cmp(&FILE_LIMIT) {
                Ordering::Greater => DiffExtent::Truncated,
                Ordering::Less | Ordering::Equal => DiffExtent::Complete,
            };
            let files = n0_future::stream::iter(entries.into_iter().take(FILE_LIMIT))
                .then(|entry| async move { patch(workspace, checkpoint, parent, entry).await })
                .collect::<Vec<Result<CheckpointFile, GitError>>>()
                .await
                .into_iter()
                .collect::<Result<Vec<CheckpointFile>, GitError>>()?;
            let (files, _, extent) = files.into_iter().fold(
                (Vec::new(), TOTAL_LIMIT, extent),
                |(mut files, budget, extent), file| {
                    let size = match &file.patch {
                        FilePatch::Text(text) => text.len(),
                        FilePatch::Binary | FilePatch::TooLarge | FilePatch::Unsupported { .. } => {
                            0
                        }
                    };
                    match budget.checked_sub(size) {
                        Some(remaining) => {
                            files.push(file);
                            (files, remaining, extent)
                        }
                        None => {
                            files.push(CheckpointFile {
                                patch: FilePatch::TooLarge,
                                ..file
                            });
                            (files, budget, DiffExtent::Truncated)
                        }
                    }
                },
            );
            Ok(CheckpointDiff {
                checkpoint: checkpoint.id,
                parent: parent.id,
                files,
                extent,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GitError, create, diff, host, restore, snapshot, text};
    use proto::{
        Checkpoint, CheckpointId, DiffExtent, FilePatch, HistoryAnchor, SessionId, UndoAvailability,
    };
    use std::path::{Path, PathBuf};

    struct Repository {
        root: PathBuf,
        source: PathBuf,
        data: PathBuf,
    }

    impl Drop for Repository {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    async fn repository() -> Repository {
        let root = std::env::temp_dir().join(format!("conductor-git-test-{}", ulid::Ulid::new()));
        let source = root.join("source");
        let data = root.join("data");
        std::fs::create_dir_all(source.join("sub dir")).unwrap();
        std::fs::write(source.join("sub dir/original.txt"), "original\n").unwrap();
        std::fs::write(source.join("deleted.txt"), "keep\n").unwrap();
        std::fs::write(source.join("binary.dat"), [0, 255, 1]).unwrap();
        std::fs::write(source.join(".gitignore"), "ignored.txt\n").unwrap();
        let _ = text(&source, &["init", "-q"]).await.unwrap();
        let _ = text(&source, &["config", "user.name", "Tests"])
            .await
            .unwrap();
        let _ = text(&source, &["config", "user.email", "tests@localhost"])
            .await
            .unwrap();
        let _ = text(&source, &["add", "-A"]).await.unwrap();
        let _ = text(&source, &["commit", "-qm", "Initial source"])
            .await
            .unwrap();
        Repository { root, source, data }
    }

    fn checkpoint(id: u64, commit: String) -> Checkpoint {
        Checkpoint {
            id: CheckpointId(id),
            parent: None,
            turn_start: None,
            commit,
            history: HistoryAnchor::Fresh,
            undo: UndoAvailability::Available,
        }
    }

    #[cfg(unix)]
    fn executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(not(unix))]
    fn executable(_path: &Path) {}

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode()
    }

    #[cfg(not(unix))]
    fn mode(_path: &Path) -> u32 {
        0
    }

    #[cfg(unix)]
    fn invalid_name(root: &Path) -> Option<PathBuf> {
        use std::os::unix::ffi::OsStringExt;
        Some(root.join(std::ffi::OsString::from_vec(Vec::from([255]))))
    }

    #[cfg(not(unix))]
    fn invalid_name(_root: &Path) -> Option<PathBuf> {
        None
    }

    #[tokio::test]
    async fn snapshots_and_restore_preserve_source_index_head_and_ignored_files() {
        let repo = repository().await;
        std::fs::write(repo.source.join("deleted.txt"), "staged source edit\n").unwrap();
        let _ = text(&repo.source, &["add", "deleted.txt"]).await.unwrap();
        std::fs::write(repo.source.join("sub dir/original.txt"), "dirty source\n").unwrap();
        std::fs::write(repo.source.join("untracked.txt"), "source untracked\n").unwrap();
        std::fs::write(
            repo.source.join(".git/hooks/post-checkout"),
            "#!/bin/sh\nexit 91\n",
        )
        .unwrap();
        executable(&repo.source.join(".git/hooks/post-checkout"));
        let source_index = std::fs::read(repo.source.join(".git/index")).unwrap();
        let source_head = text(&repo.source, &["rev-parse", "HEAD"]).await.unwrap();
        let id = SessionId(ulid::Ulid::new().into());
        let workspace = create(&repo.data, &host(&repo.source.join("sub dir")).unwrap(), id)
            .await
            .unwrap();
        assert_eq!(
            create(&repo.data, &workspace.original, id).await.unwrap(),
            workspace
        );
        let root = Path::new(&workspace.worktree.0);
        let original_mode = mode(&root.join("sub dir/original.txt"));
        let initial = snapshot(&workspace, id, CheckpointId(0), None)
            .await
            .unwrap();
        assert_eq!(
            text(root, &["show", &format!("{initial}:deleted.txt")])
                .await
                .unwrap(),
            "keep"
        );
        std::fs::write(root.join("sub dir/original.txt"), "changed\n").unwrap();
        executable(&root.join("sub dir/original.txt"));
        std::fs::remove_file(root.join("deleted.txt")).unwrap();
        std::fs::write(root.join("space 雪.txt"), "added\n").unwrap();
        std::fs::write(root.join("binary.dat"), [0, 254, 2]).unwrap();
        std::fs::write(root.join("ignored.txt"), "private\n").unwrap();
        let worktree_index =
            std::fs::read(super::git_dir(root).await.unwrap().join("index")).unwrap();
        let changed = snapshot(&workspace, id, CheckpointId(1), Some(&initial))
            .await
            .unwrap();
        assert_eq!(
            snapshot(&workspace, id, CheckpointId(1), Some(&initial))
                .await
                .unwrap(),
            changed
        );
        assert_eq!(
            std::fs::read(super::git_dir(root).await.unwrap().join("index")).unwrap(),
            worktree_index
        );
        assert_eq!(
            text(root, &["show", &format!("{changed}:space 雪.txt")])
                .await
                .unwrap(),
            "added"
        );
        assert_eq!(
            text(root, &["rev-parse", "HEAD"]).await.unwrap(),
            source_head
        );
        std::fs::write(root.join("space 雪.txt"), "different\n").unwrap();
        assert_eq!(
            matches!(
                snapshot(&workspace, id, CheckpointId(1), Some(&initial)).await,
                Err(GitError::Failed(_))
            )
            .then_some(()),
            Some(())
        );
        std::fs::write(root.join("after.txt"), "untracked after checkpoint\n").unwrap();
        for number in std::ops::RangeInclusive::new(0, 2000) {
            std::fs::write(
                root.join(format!(
                    "untracked-with-long-removal-output-{number:05}-雪.txt"
                )),
                "cleanup\n",
            )
            .unwrap();
        }
        restore(&workspace, &initial).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("deleted.txt")).unwrap(),
            "keep\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("sub dir/original.txt")).unwrap(),
            "original\n"
        );
        assert_eq!(mode(&root.join("sub dir/original.txt")), original_mode);
        assert_eq!(
            std::fs::read(root.join("binary.dat")).unwrap(),
            Vec::from([0, 255, 1])
        );
        assert_eq!(
            root.join("space 雪.txt")
                .try_exists()
                .unwrap()
                .then_some(()),
            None
        );
        assert_eq!(
            root.join("after.txt").try_exists().unwrap().then_some(()),
            None
        );
        assert_eq!(
            std::fs::read_dir(root)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .any(|name| name.to_string_lossy().starts_with("untracked-with-long"))
                .then_some(()),
            None
        );
        assert_eq!(
            std::fs::read_to_string(root.join("ignored.txt")).unwrap(),
            "private\n"
        );
        assert_eq!(
            text(root, &["rev-parse", "HEAD"]).await.unwrap(),
            source_head
        );
        assert_eq!(
            std::fs::read(repo.source.join(".git/index")).unwrap(),
            source_index
        );
        assert_eq!(
            text(&repo.source, &["rev-parse", "HEAD"]).await.unwrap(),
            source_head
        );
        assert_eq!(
            std::fs::read_to_string(repo.source.join("sub dir/original.txt")).unwrap(),
            "dirty source\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.source.join("untracked.txt")).unwrap(),
            "source untracked\n"
        );
    }

    #[tokio::test]
    async fn diff_reports_binary_large_unicode_and_file_limit_from_immutable_refs() {
        let repo = repository().await;
        let id = SessionId(ulid::Ulid::new().into());
        let workspace = create(&repo.data, &host(&repo.source).unwrap(), id)
            .await
            .unwrap();
        let root = Path::new(&workspace.worktree.0);
        let initial = checkpoint(
            0,
            snapshot(&workspace, id, CheckpointId(0), None)
                .await
                .unwrap(),
        );
        std::fs::write(root.join("binary.dat"), [0, 254, 2]).unwrap();
        std::fs::write(root.join("large.txt"), "a".repeat(70000)).unwrap();
        std::fs::write(root.join("雪 space.txt"), "hello\n").unwrap();
        let changed = checkpoint(
            1,
            snapshot(&workspace, id, CheckpointId(1), Some(&initial.commit))
                .await
                .unwrap(),
        );
        std::fs::write(root.join("雪 space.txt"), "live edit must not appear\n").unwrap();
        let difference = diff(&workspace, &changed, &initial).await.unwrap();
        assert_eq!(difference.extent, DiffExtent::Complete);
        assert_eq!(
            difference
                .files
                .iter()
                .find(|file| file.path == "binary.dat")
                .unwrap()
                .patch,
            FilePatch::Binary
        );
        assert_eq!(
            difference
                .files
                .iter()
                .find(|file| file.path == "large.txt")
                .unwrap()
                .patch,
            FilePatch::TooLarge
        );
        assert_eq!(
            difference
                .files
                .iter()
                .find(|file| file.path == "雪 space.txt")
                .is_some_and(
                    |file| matches!(&file.patch, FilePatch::Text(patch) if patch.contains("+hello"))
                )
                .then_some(()),
            Some(())
        );
        for number in std::ops::RangeInclusive::new(0, 110) {
            std::fs::write(root.join(format!("file-{number:03}.txt")), "a\n").unwrap();
        }
        let many = checkpoint(
            2,
            snapshot(&workspace, id, CheckpointId(2), Some(&changed.commit))
                .await
                .unwrap(),
        );
        let difference = diff(&workspace, &many, &changed).await.unwrap();
        assert_eq!(difference.extent, DiffExtent::Truncated);
        assert_eq!(difference.files.len(), 100);
        if let Some(path) = invalid_name(root) {
            std::fs::write(path, "unsupported name\n").unwrap();
            let unsupported = checkpoint(
                3,
                snapshot(&workspace, id, CheckpointId(3), Some(&many.commit))
                    .await
                    .unwrap(),
            );
            assert_eq!(
                matches!(
                    diff(&workspace, &unsupported, &many).await,
                    Err(GitError::Failed(_))
                )
                .then_some(()),
                Some(())
            );
        }
    }

    #[tokio::test]
    async fn deleting_selected_directory_can_be_snapshotted_and_undone() {
        let repo = repository().await;
        let id = SessionId(ulid::Ulid::new().into());
        let workspace = create(&repo.data, &host(&repo.source.join("sub dir")).unwrap(), id)
            .await
            .unwrap();
        let initial = snapshot(&workspace, id, CheckpointId(0), None)
            .await
            .unwrap();
        std::fs::remove_dir_all(&workspace.directory.0).unwrap();
        let deleted = snapshot(&workspace, id, CheckpointId(1), Some(&initial))
            .await
            .unwrap();
        assert_ne!(deleted, initial);
        restore(&workspace, &initial).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(Path::new(&workspace.directory.0).join("original.txt"))
                .unwrap(),
            "original\n"
        );
    }

    #[tokio::test]
    async fn cancellation_drops_owned_scratch_index_and_lock() {
        let repo = repository().await;
        let dir = super::git_dir(&repo.source).await.unwrap();
        let (send, receive) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let scratch = super::Scratch::create(&dir).unwrap();
            std::fs::write(scratch.path.join("index"), "index").unwrap();
            std::fs::write(scratch.path.join("index.lock"), "lock").unwrap();
            send.send(scratch.path.clone()).unwrap();
            std::future::pending::<()>().await;
            drop(scratch);
        });
        let scratch = receive.await.unwrap();
        assert_eq!(scratch.try_exists().unwrap().then_some(()), Some(()));
        task.abort();
        let _ = task.await;
        assert_eq!(scratch.try_exists().unwrap().then_some(()), None);
        assert_eq!(
            repo.source
                .join(".git/index")
                .try_exists()
                .unwrap()
                .then_some(()),
            Some(())
        );
    }

    #[tokio::test]
    async fn refuses_uncommitted_repository_unowned_directory_and_changed_branch() {
        let repo = repository().await;
        let plain = repo.root.join("plain");
        std::fs::create_dir(&plain).unwrap();
        assert_eq!(
            matches!(
                create(&repo.data, &host(&plain).unwrap(), SessionId(1)).await,
                Err(GitError::NotRepository)
            )
            .then_some(()),
            Some(())
        );
        let _ = text(&plain, &["init", "-q"]).await.unwrap();
        assert_eq!(
            matches!(
                create(&repo.data, &host(&plain).unwrap(), SessionId(1)).await,
                Err(GitError::NotRepository)
            )
            .then_some(()),
            Some(())
        );
        let id = SessionId(ulid::Ulid::new().into());
        let occupied = repo
            .data
            .join("worktrees")
            .join(ulid::Ulid::from(id.0).to_string());
        std::fs::create_dir_all(&occupied).unwrap();
        std::fs::write(occupied.join("unrelated.txt"), "keep\n").unwrap();
        assert_eq!(
            matches!(
                create(&repo.data, &host(&repo.source).unwrap(), id).await,
                Err(GitError::Failed(_))
            )
            .then_some(()),
            Some(())
        );
        assert_eq!(
            std::fs::read_to_string(occupied.join("unrelated.txt")).unwrap(),
            "keep\n"
        );
        let id = SessionId(ulid::Ulid::new().into());
        let workspace = create(&repo.data, &host(&repo.source).unwrap(), id)
            .await
            .unwrap();
        let commit = snapshot(&workspace, id, CheckpointId(0), None)
            .await
            .unwrap();
        let root = Path::new(&workspace.worktree.0);
        let _ = text(root, &["symbolic-ref", "HEAD", "refs/heads/unrelated"])
            .await
            .unwrap();
        assert_eq!(
            matches!(restore(&workspace, &commit).await, Err(GitError::Failed(_))).then_some(()),
            Some(())
        );
    }
}
