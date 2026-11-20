//! Runs the Git and filesystem steps of `host::workspace` plans.

use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use host::workspace::{Answer, Output, Plan, Step};
use proto::{
    Checkpoint, CheckpointDiff, CheckpointId, HostPath, Kept, SessionId,
    Workspace,
};
use tokio::io::AsyncReadExt;
use tokio::process::Child;

pub use host::workspace::GitError;

fn failure(context: &str, error: &str) -> GitError {
    GitError::failed(context, error)
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

async fn capture(mut child: Child) -> Result<Output, GitError> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| failure("git", "missing stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| failure("git", "missing stderr"))?;
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr
            .take(8192)
            .read_to_end(&mut bytes)
            .await
            .map(|_| String::from_utf8_lossy(&bytes).into_owned())
    });
    let mut bytes = Vec::new();
    stdout
        .take(Output::CAPTURE as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| failure("reading git stdout", &error.to_string()))?;
    if bytes.len() == Output::CAPTURE {
        child.kill().await.map_err(|error| {
            failure("stopping oversized git output", &error.to_string())
        })?;
    }
    let status = child
        .wait()
        .await
        .map_err(|error| failure("waiting for git", &error.to_string()))?;
    let error = stderr_task
        .await
        .map_err(|error| failure("reading git stderr", &error.to_string()))?
        .map_err(|error| failure("reading git stderr", &error.to_string()))?;
    Ok(Output {
        bytes,
        success: status.success(),
        error,
    })
}

async fn execute(
    command: &mut tokio::process::Command,
) -> Result<Output, GitError> {
    let child = command
        .spawn()
        .map_err(|error| failure("starting git", &error.to_string()))?;
    tokio::time::timeout(Duration::from_secs(120), capture(child))
        .await
        .map_err(|error| failure("git timeout", &error.to_string()))?
}

pub async fn text(path: &Path, args: &[&str]) -> Result<String, GitError> {
    run(Plan::git_text("", args), Some(path)).await
}

fn canonical(path: &Path) -> Result<PathBuf, GitError> {
    std::fs::canonicalize(path).map_err(|error| {
        failure(
            &format!("canonicalizing {}", path.display()),
            &error.to_string(),
        )
    })
}

#[cfg(test)]
fn host(path: &Path) -> Result<HostPath, GitError> {
    path.to_str()
        .map(|value| HostPath(value.to_owned()))
        .ok_or_else(|| {
            failure("Git workspace path", "non-UTF-8 paths are unsupported")
        })
}

#[cfg(test)]
async fn git_dir(path: &Path) -> Result<PathBuf, GitError> {
    canonical(Path::new(
        &text(path, &["rev-parse", "--absolute-git-dir"]).await?,
    ))
}

pub async fn create(
    data_dir: &Path,
    original: &HostPath,
    session: SessionId,
    base: Option<&str>,
) -> Result<Workspace, GitError> {
    run(Plan::create(original, session, base), Some(data_dir)).await
}

pub async fn snapshot(
    workspace: &Workspace,
    session: SessionId,
    checkpoint: CheckpointId,
    previous: Option<&str>,
) -> Result<String, GitError> {
    run(
        Plan::snapshot(workspace, session, checkpoint, previous),
        None,
    )
    .await
}

pub async fn preflight(
    workspace: &Workspace,
    commit: &str,
) -> Result<(), GitError> {
    run(Plan::preflight(workspace, commit), None).await
}

pub async fn restore(
    workspace: &Workspace,
    commit: &str,
) -> Result<(), GitError> {
    run(Plan::restore(workspace, commit), None).await
}

pub async fn diff(
    workspace: &Workspace,
    checkpoint: &Checkpoint,
    parent: &Checkpoint,
) -> Result<CheckpointDiff, GitError> {
    run(Plan::diff(workspace, checkpoint, parent), None).await
}

pub async fn remove(workspace: &Workspace) -> Result<Kept, GitError> {
    let kept = run(Plan::remove(workspace), None).await?;
    tracing::info!(branch = workspace.branch, ?kept, "removed a workspace");
    Ok(kept)
}

pub async fn land(workspace: &Workspace, issue: &str) -> Result<u64, String> {
    run(Plan::land(workspace, issue), None)
        .await
        .map_err(|error| error.to_string())
}

pub async fn push_branch(
    workspace: &Workspace,
    branch: &str,
) -> Option<(String, Result<(), String>)> {
    run(Plan::push(workspace, branch), None)
        .await
        .ok()
        .flatten()
}

async fn run<T>(
    mut plan: Plan<'_, T>,
    shell_path: Option<&Path>,
) -> Result<T, GitError> {
    let mut runner = Runner {
        shell_path,
        scratch: None,
    };
    let mut answer = None;
    loop {
        match plan.advance(answer) {
            ControlFlow::Continue((step, remaining)) => {
                plan = remaining;
                answer = Some(runner.step(step).await);
            }
            ControlFlow::Break(result) => return result,
        }
    }
}

struct Runner<'a> {
    shell_path: Option<&'a Path>,
    scratch: Option<Scratch>,
}

impl Runner<'_> {
    async fn step(&mut self, step: Step) -> Result<Answer, GitError> {
        match step {
            Step::Git {
                path,
                args,
                index,
                identity,
            } => self.git(&path, &args, index, identity).await,
            Step::OpenIndex(directory) => {
                self.scratch = Some(Scratch::create(Path::new(&directory))?);
                Ok(Answer::Done)
            }
            Step::CloseIndex => {
                self.scratch = None;
                Ok(Answer::Done)
            }
            filesystem => self.filesystem(filesystem),
        }
    }

    async fn git(
        &self,
        path: &str,
        args: &[String],
        index: bool,
        identity: bool,
    ) -> Result<Answer, GitError> {
        let cwd = self.command_path(path)?;
        let words = args.iter().map(String::as_str).collect::<Vec<_>>();
        let mut command = git(cwd, &words);
        if index {
            let scratch = self
                .scratch
                .as_ref()
                .ok_or_else(|| failure("git", "missing scratch index"))?;
            command.env("GIT_INDEX_FILE", scratch.path.join("index"));
        }
        if identity {
            command
                .env("GIT_AUTHOR_NAME", "Conductor")
                .env("GIT_AUTHOR_EMAIL", "conductor@localhost")
                .env("GIT_COMMITTER_NAME", "Conductor")
                .env("GIT_COMMITTER_EMAIL", "conductor@localhost");
        }
        execute(&mut command).await.map(Answer::Git)
    }

    fn command_path<'a>(&'a self, path: &'a str) -> Result<&'a Path, GitError> {
        if path.is_empty() {
            self.shell_path
                .ok_or_else(|| failure("git", "missing command directory"))
        } else {
            Ok(Path::new(path))
        }
    }

    fn filesystem(&self, step: Step) -> Result<Answer, GitError> {
        match step {
            Step::Canonicalize(path) => canonical(Path::new(&path))
                .map(|resolved| path_answer(&resolved)),
            Step::WorktreeParent => {
                let parent = self
                    .shell_path
                    .ok_or_else(|| {
                        failure("workspace", "missing data directory")
                    })?
                    .join("worktrees");
                std::fs::create_dir_all(&parent).map_err(|error| {
                    failure("creating worktree parent", &error.to_string())
                })?;
                canonical(&parent).map(|resolved| path_answer(&resolved))
            }
            Step::Relative {
                path,
                root,
                context,
            } => Path::new(&path)
                .strip_prefix(root)
                .map(path_answer)
                .map_err(|error| failure(context, &error.to_string())),
            Step::Join { root, suffix } => {
                Ok(path_answer(&Path::new(&root).join(suffix)))
            }
            Step::Parent(path) => Path::new(&path)
                .parent()
                .map(path_answer)
                .ok_or_else(|| failure("workspace", "missing parent")),
            Step::Within { path, root } => {
                Ok(Answer::Found(Path::new(&path).starts_with(root)))
            }
            Step::Exists(path) => exists(Path::new(&path)).map(Answer::Found),
            Step::IsDirectory(path) => {
                Ok(Answer::Found(Path::new(&path).is_dir()))
            }
            Step::ReadOwner(directory) => std::fs::read_to_string(
                Path::new(&directory).join("conductor-workspace"),
            )
            .map(Answer::Text)
            .map_err(|error| {
                failure("reading workspace ownership", &error.to_string())
            }),
            Step::WriteOwner { directory, marker } => {
                write_owner(Path::new(&directory), &marker)
                    .map(|()| Answer::Done)
            }
            Step::Git { .. } | Step::OpenIndex(_) | Step::CloseIndex => {
                Err(failure("workspace", "unexpected filesystem step"))
            }
        }
    }
}

fn path_answer(path: &Path) -> Answer {
    Answer::Path(path.to_str().map(str::to_owned))
}

fn exists(path: &Path) -> Result<bool, GitError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(failure("worktree path", &error.to_string())),
    }
}

fn write_owner(directory: &Path, marker: &str) -> Result<(), GitError> {
    let opened = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join("conductor-workspace"));
    opened
        .and_then(|mut file| {
            std::io::Write::write_all(&mut file, marker.as_bytes())
        })
        .map_err(|error| {
            failure("recording worktree ownership", &error.to_string())
        })
}

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn create(dir: &Path) -> Result<Self, GitError> {
        let path = dir.join(format!("conductor-index-{}", ulid::Ulid::new()));
        std::fs::create_dir(&path).map_err(|error| {
            failure("creating checkpoint scratch directory", &error.to_string())
        })?;
        Ok(Self { path })
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

#[cfg(test)]
mod tests {
    use super::{
        GitError, create, diff, host, remove, restore, snapshot, text,
    };
    use proto::{
        Checkpoint, CheckpointId, DiffExtent, FilePatch, HistoryAnchor, Kept,
        SessionId, UndoAvailability, Workspace,
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

    async fn repository() -> Result<Repository, Box<dyn std::error::Error>> {
        let root = std::env::temp_dir()
            .join(format!("conductor-git-test-{}", ulid::Ulid::new()));
        let source = root.join("source");
        let data = root.join("data");
        std::fs::create_dir_all(source.join("sub dir"))?;
        std::fs::write(source.join("sub dir/original.txt"), "original\n")?;
        std::fs::write(source.join("deleted.txt"), "keep\n")?;
        std::fs::write(source.join("binary.dat"), [0, 255, 1])?;
        std::fs::write(source.join(".gitignore"), "ignored.txt\n")?;
        let _ = text(&source, &["init", "-q"]).await?;
        let _ = text(&source, &["config", "user.name", "Tests"]).await?;
        let _ =
            text(&source, &["config", "user.email", "tests@localhost"]).await?;
        let _ = text(&source, &["add", "-A"]).await?;
        let _ = text(&source, &["commit", "-qm", "Initial source"]).await?;
        Ok(Repository { root, source, data })
    }

    #[tokio::test]
    async fn removing_keeps_the_dirty_workspace_on_its_pushed_branch()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = repository().await?;
        let remote = repo.root.join("remote.git");
        let _ =
            text(&repo.root, &["init", "-q", "--bare", "remote.git"]).await?;
        let url = remote.display().to_string();
        let _ = text(&repo.source, &["remote", "add", "origin", &url]).await?;
        let id = SessionId(u128::from(ulid::Ulid::new()));
        let original = super::host(&repo.source)?;
        let workspace = create(&repo.data, &original, id, None).await?;
        let checkpointed =
            snapshot(&workspace, id, CheckpointId(0), None).await?;
        let root = PathBuf::from(&workspace.worktree.0);
        std::fs::write(root.join("new.txt"), "left uncommitted\n")?;
        assert_eq!(remove(&workspace).await?, Kept::Pushed { remote: url });
        assert!(!root.exists());
        let branch = host::workspace::head(&workspace);
        let pushed = text(&remote, &["rev-parse", &branch]).await?;
        assert_eq!(text(&repo.source, &["rev-parse", &branch]).await?, pushed);
        assert_eq!(
            text(&remote, &["show", &format!("{pushed}:new.txt")]).await?,
            "left uncommitted"
        );
        let refs = host::workspace::checkpoints(&workspace)?;
        assert_eq!(text(&repo.source, &["for-each-ref", &refs]).await?, "");
        assert!(
            text(&repo.source, &["cat-file", "-e", &checkpointed])
                .await
                .is_ok()
        );
        Ok(())
    }

    #[tokio::test]
    async fn removing_a_workspace_without_a_remote_keeps_its_branch_locally()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = repository().await?;
        let id = SessionId(u128::from(ulid::Ulid::new()));
        let original = super::host(&repo.source)?;
        let workspace = create(&repo.data, &original, id, None).await?;
        let head = text(&repo.source, &["rev-parse", "HEAD"]).await?;
        assert_eq!(remove(&workspace).await?, Kept::Local);
        let branch = host::workspace::head(&workspace);
        assert_eq!(text(&repo.source, &["rev-parse", &branch]).await?, head);
        assert!(!Path::new(&workspace.worktree.0).exists());
        Ok(())
    }

    /// A session's workspace on `issue`, a branch at the source's HEAD,
    /// and a commit on the issue's branch that touches `file`.
    async fn diverged(
        repo: &Repository,
        file: &str,
    ) -> Result<(Workspace, String), Box<dyn std::error::Error>> {
        let issue = String::from("refs/heads/issue/x");
        let head = text(&repo.source, &["rev-parse", "HEAD"]).await?;
        let _ = text(&repo.source, &["update-ref", &issue, &head]).await?;
        let id = SessionId(u128::from(ulid::Ulid::new()));
        let original = super::host(&repo.source)?;
        let workspace = create(&repo.data, &original, id, Some(&issue)).await?;
        let other = repo.root.join("other");
        let _ = text(
            &repo.source,
            &[
                "worktree",
                "add",
                "-q",
                &other.display().to_string(),
                "issue/x",
            ],
        )
        .await?;
        std::fs::write(other.join(file), "on the issue\n")?;
        let _ = text(&other, &["add", "-A"]).await?;
        let _ = text(&other, &["commit", "-qm", "on the issue"]).await?;
        let _ = text(
            &repo.source,
            &[
                "worktree",
                "remove",
                "--force",
                &other.display().to_string(),
            ],
        )
        .await?;
        Ok((workspace, issue))
    }

    #[tokio::test]
    async fn landing_merges_diverged_branches_and_refuses_a_conflict()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = repository().await?;
        let (workspace, issue) = diverged(&repo, "issue-only.txt").await?;
        let root = PathBuf::from(&workspace.worktree.0);
        std::fs::write(root.join("session-only.txt"), "in the session\n")?;
        assert_eq!(super::land(&workspace, &issue).await, Ok(1));
        let merged =
            text(&repo.source, &["rev-parse", &format!("{issue}^2")]).await?;
        assert_eq!(
            merged,
            text(&repo.source, &["rev-parse", &workspace.branch]).await?,
            "the issue's branch is a merge with the session's"
        );
        for file in ["issue-only.txt", "session-only.txt"] {
            assert!(
                text(
                    &repo.source,
                    &["cat-file", "-e", &format!("{issue}:{file}")]
                )
                .await
                .is_ok()
            );
        }

        let conflicted = repository().await?;
        let (clashing, base) = diverged(&conflicted, "deleted.txt").await?;
        let at = text(&conflicted.source, &["rev-parse", &base]).await?;
        std::fs::write(
            PathBuf::from(&clashing.worktree.0).join("deleted.txt"),
            "in the session\n",
        )?;
        assert!(
            super::land(&clashing, &base)
                .await
                .is_err_and(|reason| reason.contains("conflicts")),
            "a conflict lands nothing"
        );
        assert_eq!(
            text(&conflicted.source, &["rev-parse", &base]).await?,
            at,
            "the issue's branch stays where it was"
        );
        Ok(())
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
    fn executable(path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
    }

    #[cfg(not(unix))]
    fn executable(_path: &Path) -> std::io::Result<()> {
        Ok(())
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> std::io::Result<u32> {
        use std::os::unix::fs::PermissionsExt;
        Ok(std::fs::metadata(path)?.permissions().mode())
    }

    #[cfg(not(unix))]
    fn mode(_path: &Path) -> std::io::Result<u32> {
        Ok(0)
    }

    /// A file name that isn't UTF-8 makes the diff fail.
    #[cfg(unix)]
    async fn assert_invalid_name_fails(
        workspace: &Workspace,
        id: SessionId,
        many: &Checkpoint,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::ffi::OsStringExt;
        let root = Path::new(&workspace.worktree.0);
        std::fs::write(
            root.join(std::ffi::OsString::from_vec(Vec::from([255]))),
            "unsupported name\n",
        )?;
        let unsupported = checkpoint(
            3,
            snapshot(workspace, id, CheckpointId(3), Some(&many.commit))
                .await?,
        );
        assert_eq!(
            matches!(
                diff(workspace, &unsupported, many).await,
                Err(GitError::Failed(_))
            )
            .then_some(()),
            Some(())
        );
        Ok(())
    }

    #[cfg(not(unix))]
    async fn assert_invalid_name_fails(
        _workspace: &Workspace,
        _id: SessionId,
        _many: &Checkpoint,
    ) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }

    /// The source checkout's index and head, after staging, editing and
    /// adding files and a hook that fails any checkout there.
    async fn dirty_source(
        repo: &Repository,
    ) -> Result<(Vec<u8>, String), Box<dyn std::error::Error>> {
        std::fs::write(
            repo.source.join("deleted.txt"),
            "staged source edit\n",
        )?;
        let _ = text(&repo.source, &["add", "deleted.txt"]).await?;
        std::fs::write(
            repo.source.join("sub dir/original.txt"),
            "dirty source\n",
        )?;
        std::fs::write(
            repo.source.join("untracked.txt"),
            "source untracked\n",
        )?;
        std::fs::write(
            repo.source.join(".git/hooks/post-checkout"),
            "#!/bin/sh\nexit 91\n",
        )?;
        executable(&repo.source.join(".git/hooks/post-checkout"))?;
        let source_index = std::fs::read(repo.source.join(".git/index"))?;
        let source_head = text(&repo.source, &["rev-parse", "HEAD"]).await?;
        Ok((source_index, source_head))
    }

    /// The source checkout is as `dirty_source` left it.
    async fn assert_source_untouched(
        repo: &Repository,
        source_index: &[u8],
        source_head: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            std::fs::read(repo.source.join(".git/index"))?,
            source_index
        );
        assert_eq!(
            text(&repo.source, &["rev-parse", "HEAD"]).await?,
            source_head
        );
        assert_eq!(
            std::fs::read_to_string(repo.source.join("sub dir/original.txt"))?,
            "dirty source\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.source.join("untracked.txt"))?,
            "source untracked\n"
        );
        Ok(())
    }

    #[tokio::test]
    async fn snapshots_and_restore_preserve_source_index_head_and_ignored_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = repository().await?;
        let (source_index, source_head) = dirty_source(&repo).await?;
        let id = SessionId(ulid::Ulid::new().into());
        let workspace =
            create(&repo.data, &host(&repo.source.join("sub dir"))?, id, None)
                .await?;
        assert_eq!(
            create(&repo.data, &workspace.original, id, None).await?,
            workspace
        );
        let root = Path::new(&workspace.worktree.0);
        let original_mode = mode(&root.join("sub dir/original.txt"))?;
        let initial = snapshot(&workspace, id, CheckpointId(0), None).await?;
        assert_eq!(
            initial, source_head,
            "a checkpoint that changed nothing is the branch's tip"
        );
        assert_eq!(
            text(root, &["show", &format!("{initial}:deleted.txt")]).await?,
            "keep"
        );
        std::fs::write(root.join("sub dir/original.txt"), "changed\n")?;
        executable(&root.join("sub dir/original.txt"))?;
        std::fs::remove_file(root.join("deleted.txt"))?;
        std::fs::write(root.join("space 雪.txt"), "added\n")?;
        std::fs::write(root.join("binary.dat"), [0, 254, 2])?;
        std::fs::write(root.join("ignored.txt"), "private\n")?;
        let changed =
            snapshot(&workspace, id, CheckpointId(1), Some(&initial)).await?;
        assert_eq!(
            snapshot(&workspace, id, CheckpointId(1), Some(&initial)).await?,
            changed
        );
        assert_eq!(
            (
                text(root, &["rev-parse", "HEAD"]).await?,
                text(root, &["status", "--porcelain"]).await?
            ),
            (changed.clone(), String::new()),
            "the turn is a commit on the branch, and the index follows it"
        );
        assert_eq!(
            text(root, &["show", &format!("{changed}:space 雪.txt")]).await?,
            "added"
        );
        std::fs::write(root.join("space 雪.txt"), "different\n")?;
        assert_eq!(
            matches!(
                snapshot(&workspace, id, CheckpointId(1), Some(&initial)).await,
                Err(GitError::Failed(_))
            )
            .then_some(()),
            Some(())
        );
        std::fs::write(root.join("after.txt"), "untracked after checkpoint\n")?;
        for number in std::ops::RangeInclusive::new(0, 2000) {
            std::fs::write(
                root.join(format!(
                    "untracked-with-long-removal-output-{number:05}-雪.txt"
                )),
                "cleanup\n",
            )?;
        }
        restore(&workspace, &initial).await?;
        assert_eq!(
            std::fs::read_to_string(root.join("deleted.txt"))?,
            "keep\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("sub dir/original.txt"))?,
            "original\n"
        );
        assert_eq!(mode(&root.join("sub dir/original.txt"))?, original_mode);
        assert_eq!(
            std::fs::read(root.join("binary.dat"))?,
            Vec::from([0, 255, 1])
        );
        assert_eq!(root.join("space 雪.txt").try_exists()?.then_some(()), None);
        assert_eq!(root.join("after.txt").try_exists()?.then_some(()), None);
        assert_eq!(
            std::fs::read_dir(root)?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|entry| entry.file_name())
                .any(|name| name
                    .to_string_lossy()
                    .starts_with("untracked-with-long"))
                .then_some(()),
            None
        );
        assert_eq!(
            std::fs::read_to_string(root.join("ignored.txt"))?,
            "private\n"
        );
        assert_eq!(
            text(root, &["rev-parse", "HEAD"]).await?,
            source_head,
            "undoing resets the session's branch"
        );
        assert_source_untouched(&repo, &source_index, &source_head).await
    }

    #[tokio::test]
    async fn diff_reports_binary_large_unicode_and_file_limit_from_immutable_refs()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = repository().await?;
        let id = SessionId(ulid::Ulid::new().into());
        let workspace =
            create(&repo.data, &host(&repo.source)?, id, None).await?;
        let root = Path::new(&workspace.worktree.0);
        let initial = checkpoint(
            0,
            snapshot(&workspace, id, CheckpointId(0), None).await?,
        );
        std::fs::write(root.join("binary.dat"), [0, 254, 2])?;
        std::fs::write(root.join("large.txt"), "a".repeat(70000))?;
        std::fs::write(root.join("雪 space.txt"), "hello\n")?;
        let changed = checkpoint(
            1,
            snapshot(&workspace, id, CheckpointId(1), Some(&initial.commit))
                .await?,
        );
        std::fs::write(
            root.join("雪 space.txt"),
            "live edit must not appear\n",
        )?;
        let difference = diff(&workspace, &changed, &initial).await?;
        assert_eq!(difference.extent, DiffExtent::Complete);
        assert_eq!(
            difference
                .files
                .iter()
                .find(|file| file.path == "binary.dat")
                .ok_or("expected a value")?
                .patch,
            FilePatch::Binary
        );
        assert_eq!(
            difference
                .files
                .iter()
                .find(|file| file.path == "large.txt")
                .ok_or("expected a value")?
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
            std::fs::write(root.join(format!("file-{number:03}.txt")), "a\n")?;
        }
        let many = checkpoint(
            2,
            snapshot(&workspace, id, CheckpointId(2), Some(&changed.commit))
                .await?,
        );
        let difference = diff(&workspace, &many, &changed).await?;
        assert_eq!(difference.extent, DiffExtent::Truncated);
        assert_eq!(difference.files.len(), 100);
        assert_invalid_name_fails(&workspace, id, &many).await
    }

    #[tokio::test]
    async fn deleting_selected_directory_can_be_snapshotted_and_undone()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = repository().await?;
        let id = SessionId(ulid::Ulid::new().into());
        let workspace =
            create(&repo.data, &host(&repo.source.join("sub dir"))?, id, None)
                .await?;
        let initial = snapshot(&workspace, id, CheckpointId(0), None).await?;
        std::fs::remove_dir_all(&workspace.directory.0)?;
        let deleted =
            snapshot(&workspace, id, CheckpointId(1), Some(&initial)).await?;
        assert_ne!(deleted, initial);
        restore(&workspace, &initial).await?;
        assert_eq!(
            std::fs::read_to_string(
                Path::new(&workspace.directory.0).join("original.txt")
            )?,
            "original\n"
        );
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_drops_owned_scratch_index_and_lock()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = repository().await?;
        let dir = super::git_dir(&repo.source).await?;
        let (send, receive) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let scratch = super::Scratch::create(&dir)?;
            std::fs::write(scratch.path.join("index"), "index")?;
            std::fs::write(scratch.path.join("index.lock"), "lock")?;
            send.send(scratch.path.clone())
                .map_err(|_| "the test stopped")?;
            std::future::pending::<()>().await;
            drop(scratch);
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        });
        let scratch = receive.await?;
        assert_eq!(scratch.try_exists()?.then_some(()), Some(()));
        task.abort();
        let _ = task.await;
        assert_eq!(scratch.try_exists()?.then_some(()), None);
        assert_eq!(
            repo.source.join(".git/index").try_exists()?.then_some(()),
            Some(())
        );
        Ok(())
    }

    #[tokio::test]
    async fn refuses_uncommitted_repository_unowned_directory_and_changed_branch()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = repository().await?;
        let plain = repo.root.join("plain");
        std::fs::create_dir(&plain)?;
        assert_eq!(
            matches!(
                create(&repo.data, &host(&plain)?, SessionId(1), None).await,
                Err(GitError::NotRepository)
            )
            .then_some(()),
            Some(())
        );
        let _ = text(&plain, &["init", "-q"]).await?;
        assert_eq!(
            matches!(
                create(&repo.data, &host(&plain)?, SessionId(1), None).await,
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
        std::fs::create_dir_all(&occupied)?;
        std::fs::write(occupied.join("unrelated.txt"), "keep\n")?;
        assert_eq!(
            matches!(
                create(&repo.data, &host(&repo.source)?, id, None).await,
                Err(GitError::Failed(_))
            )
            .then_some(()),
            Some(())
        );
        assert_eq!(
            std::fs::read_to_string(occupied.join("unrelated.txt"))?,
            "keep\n"
        );
        let id = SessionId(ulid::Ulid::new().into());
        let workspace =
            create(&repo.data, &host(&repo.source)?, id, None).await?;
        let commit = snapshot(&workspace, id, CheckpointId(0), None).await?;
        let root = Path::new(&workspace.worktree.0);
        let _ = text(root, &["symbolic-ref", "HEAD", "refs/heads/unrelated"])
            .await?;
        assert_eq!(
            matches!(
                restore(&workspace, &commit).await,
                Err(GitError::Failed(_))
            )
            .then_some(()),
            Some(())
        );
        Ok(())
    }
}
