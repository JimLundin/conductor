use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use home_files::git::{self as executable, Output};
use host::log::HostSeq;
#[cfg(test)]
use host::session::SessionDirectory;
mod owned;
mod plan;
use home_files::git::{Index, Instruction};
use host::workspace::Workspace;
pub use owned::Workspaces;
use plan::{Answer, Plan, Step};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Child;
use vcs::Kept;

pub use vcs::Error as GitError;

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

const BLOB_LIMIT: usize = 1024 * 1024;

async fn capture(
    mut child: Child,
    capture_limit: usize,
) -> Result<Output, GitError> {
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
        .take(capture_limit as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| failure("reading git stdout", &error.to_string()))?;
    if bytes.len() == capture_limit {
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
    capture_limit: usize,
) -> Result<Output, GitError> {
    let child = command
        .spawn()
        .map_err(|error| failure("starting git", &error.to_string()))?;
    tokio::time::timeout(
        Duration::from_secs(120),
        capture(child, capture_limit),
    )
    .await
    .map_err(|error| failure("git timeout", &error.to_string()))?
}

pub async fn text(path: &Path, args: &[&str]) -> Result<String, GitError> {
    executable::text(
        execute(&mut git(path, args), Output::CAPTURE).await?,
        args.first().copied().unwrap_or("git"),
    )
}

pub async fn repository_text(
    path: &Path,
    request: Instruction,
) -> Result<String, GitError> {
    let command = executable::command(&request);
    let args: Vec<&str> = command.args.iter().map(String::as_str).collect();
    text(path, &args).await
}

pub async fn blob(path: &Path, object: &str) -> Result<Vec<u8>, GitError> {
    let mut command = git(path, &["cat-file", "blob", object]);
    let output = execute(&mut command, BLOB_LIMIT + 1).await?;
    if output.bytes.len() > BLOB_LIMIT {
        Err(failure(
            "reading git blob",
            "committed metadata exceeds 1 MiB",
        ))
    } else if output.success {
        Ok(output.bytes)
    } else {
        Err(failure("reading git blob", &output.error))
    }
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
fn host(path: &Path) -> Result<SessionDirectory, GitError> {
    path.to_str()
        .map(|value| SessionDirectory(value.to_owned()))
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

fn head(workspace: &Workspace) -> String {
    executable::head(&workspace.branch)
}

#[cfg(test)]
fn snapshots(workspace: &Workspace) -> Result<String, GitError> {
    Ok(format!(
        "refs/conductor/{}/",
        host::workspace::owner(workspace)?
    ))
}

pub fn trace_removed(branch: &str, kept: &Kept) {
    let outcome = match kept {
        Kept::Local => "local",
        Kept::Pushed { .. } => "pushed",
        Kept::PushFailed { .. } => "push_failed",
    };
    tracing::info!(branch, outcome, "removed a workspace");
}

pub async fn landing_receipt(
    workspace: &Workspace,
    issue: &str,
    attempt: HostSeq,
) -> Result<Option<u64>, String> {
    run(Plan::landing_receipt(
        workspace,
        issue,
        vcs::command::IntegrationId(attempt.0.to_string()),
    ))
    .await
    .map_err(|error| error.to_string())
}

pub async fn land(
    workspace: &Workspace,
    issue: &str,
    attempt: HostSeq,
) -> Result<u64, String> {
    run(Plan::land(
        workspace,
        issue,
        vcs::command::IntegrationId(attempt.0.to_string()),
    ))
    .await
    .map_err(|error| error.to_string())
}

pub async fn push_branch(
    workspace: &Workspace,
    branch: &str,
) -> Option<(String, Result<(), String>)> {
    run(Plan::push(workspace, branch)).await.ok().flatten()
}

async fn run<T>(mut plan: Plan<'_, T>) -> Result<T, GitError> {
    let mut runner = Runner { scratch: None };
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

struct Runner {
    scratch: Option<Scratch>,
}

impl Runner {
    async fn step(&mut self, step: Step) -> Result<Answer, GitError> {
        match step {
            Step::Repository { path, request } => {
                self.repository(&path, request).await
            }
            Step::OpenIndex(directory) => {
                self.scratch = Some(Scratch::create(Path::new(&directory))?);
                Ok(Answer::Done)
            }
            Step::CloseIndex => {
                self.scratch = None;
                Ok(Answer::Done)
            }
            filesystem => Self::filesystem(filesystem),
        }
    }

    async fn repository(
        &self,
        path: &str,
        request: Instruction,
    ) -> Result<Answer, GitError> {
        let output = if let Instruction::Integrate {
            branch,
            commit,
            expected,
            receipt,
        } = &request
        {
            Self::land_refs(path, branch, commit, expected, receipt).await?
        } else {
            let command = executable::command(&request);
            self.git(
                path,
                &command.args,
                command.index == Index::Scratch,
                command.identity,
            )
            .await?
        };
        Ok(Answer::Repository(executable::reply(&request, output)))
    }

    async fn land_refs(
        repository: &str,
        issue: &str,
        commit: &str,
        old: &str,
        receipt: &str,
    ) -> Result<Output, GitError> {
        let updates = format!(
            "update {issue}\0{commit}\0{old}\0create {receipt}\0{commit}\0"
        );
        let mut child =
            git(Path::new(repository), &["update-ref", "--stdin", "-z"])
                .stdin(Stdio::piped())
                .spawn()
                .map_err(|error| {
                    failure("starting git update-ref", &error.to_string())
                })?;
        let output = tokio::time::timeout(Duration::from_secs(120), async {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| failure("update-ref", "missing stdin"))?;
            stdin.write_all(updates.as_bytes()).await.map_err(|error| {
                failure("writing git update-ref", &error.to_string())
            })?;
            drop(stdin);
            capture(child, Output::CAPTURE).await
        })
        .await
        .map_err(|error| {
            failure("git update-ref timeout", &error.to_string())
        })??;
        Ok(output)
    }

    async fn git(
        &self,
        path: &str,
        args: &[String],
        index: bool,
        identity: bool,
    ) -> Result<Output, GitError> {
        let cwd = Path::new(path);
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
        execute(&mut command, Output::CAPTURE).await
    }

    fn filesystem(step: Step) -> Result<Answer, GitError> {
        match step {
            Step::Canonicalize(path) => canonical(Path::new(&path))
                .map(|resolved| path_answer(&resolved)),
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
            Step::IsDirectory(path) => {
                directory_present(Path::new(&path)).map(Answer::Found)
            }
            Step::ReadOwner(directory) => std::fs::read_to_string(
                Path::new(&directory).join("conductor-workspace"),
            )
            .map(Answer::Text)
            .map_err(|error| {
                failure("reading workspace ownership", &error.to_string())
            }),
            Step::Repository { .. } | Step::OpenIndex(_) | Step::CloseIndex => {
                Err(failure("workspace", "unexpected filesystem step"))
            }
        }
    }
}

fn path_answer(path: &Path) -> Answer {
    Answer::Path(path.to_str().map(str::to_owned))
}

fn directory_present(path: &Path) -> Result<bool, GitError> {
    match path_metadata(path)? {
        Some(metadata) if metadata.is_dir() => Ok(true),
        Some(_) => Err(failure("worktree path", "expected a directory")),
        None => Ok(false),
    }
}

fn path_metadata(path: &Path) -> Result<Option<std::fs::Metadata>, GitError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(failure("worktree path", &error.to_string())),
    }
}

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn create(dir: &Path) -> Result<Self, GitError> {
        let path = dir.join(format!("conductor-index-{}", ulid::Ulid::new()));
        std::fs::create_dir(&path).map_err(|error| {
            failure("creating snapshot scratch directory", &error.to_string())
        })?;
        Ok(Self { path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            tracing::warn!(
                "removing snapshot scratch {}: {error}",
                self.path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{GitError, host, landing_receipt, text};
    use host::log::HostSeq;
    use host::session::SessionId;
    use host::workspace::Workspace;
    use std::path::{Path, PathBuf};
    use vcs::{DiffExtent, FilePatch, Kept, Worktree};

    struct Repository {
        root: PathBuf,
        source: PathBuf,
        data: PathBuf,
    }

    impl Repository {
        async fn snapshot(
            &self,
            workspace: &Workspace,
            id: SessionId,
        ) -> Result<String, GitError> {
            let attempt =
                vcs::command::SnapshotAttempt(ulid::Ulid::new().to_string());
            workspaces(&self.data)
                .snapshot(workspace, id, attempt)
                .await
        }
    }

    impl Drop for Repository {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn prevent_repository_discovery(path: &Path) -> std::io::Result<()> {
        std::fs::write(path.join(".git"), "gitdir: missing-repository\n")
    }

    fn workspaces(data: &Path) -> super::Workspaces {
        super::Workspaces::new(
            vcs_git2::registration::local(data),
            vcs_git2::registration::register,
        )
    }

    async fn create(
        data: &Path,
        original: &host::session::SessionDirectory,
        id: SessionId,
        base: Option<&str>,
    ) -> Result<Workspace, GitError> {
        workspaces(data).create(original, id, base).await
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
    async fn separate_turns_capture_new_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = repository().await?;
        let id = SessionId(ulid::Ulid::new().into());
        let workspace =
            create(&repo.data, &host(&repo.source)?, id, None).await?;
        let root = Path::new(&workspace.worktree.0);
        let _ = repo.snapshot(&workspace, id).await?;
        std::fs::write(root.join("retry.txt"), "first\n")?;
        let first = repo.snapshot(&workspace, id).await?;
        std::fs::write(root.join("retry.txt"), "second\n")?;
        let recovered = repo.snapshot(&workspace, id).await?;
        assert_ne!(first, recovered);
        assert_eq!(
            text(root, &["show", &format!("{recovered}:retry.txt")]).await?,
            "second"
        );
        assert_eq!(text(root, &["rev-parse", "HEAD"]).await?, recovered);
        Ok(())
    }

    #[tokio::test]
    async fn diffs_span_agent_commits_and_only_compare_this_sessions_recorded_commits()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = repository().await?;
        let id = SessionId(ulid::Ulid::new().into());
        let workspace =
            create(&repo.data, &host(&repo.source)?, id, None).await?;
        let root = Path::new(&workspace.worktree.0);
        let initial = repo.snapshot(&workspace, id).await?;
        for name in ["agent-one.txt", "agent-two.txt"] {
            std::fs::write(root.join(name), name)?;
            let _ = text(root, &["add", name]).await?;
            let _ = text(root, &["commit", "-qm", name]).await?;
        }
        std::fs::write(root.join("remaining.txt"), "remaining\n")?;
        let completed = repo.snapshot(&workspace, id).await?;
        let difference = workspaces(&repo.data)
            .diff(&workspace, id, HostSeq(12), &completed, &initial)
            .await?;
        assert_eq!(
            difference
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["agent-one.txt", "agent-two.txt", "remaining.txt"]
        );
        assert_eq!(repo.snapshot(&workspace, id).await?, completed);
        assert_eq!(
            workspaces(&repo.data)
                .diff(&workspace, id, HostSeq(15), &completed, &completed)
                .await?
                .files,
            Vec::<vcs::SnapshotFile>::new()
        );
        let foreign_id = SessionId(ulid::Ulid::new().into());
        let foreign =
            create(&repo.data, &host(&repo.source)?, foreign_id, None).await?;
        let _ = repo.snapshot(&foreign, foreign_id).await?;
        assert!(
            workspaces(&repo.data)
                .diff(&foreign, foreign_id, HostSeq(12), &completed, &initial)
                .await
                .is_err()
        );
        assert!(
            workspaces(&repo.data)
                .diff(&workspace, id, HostSeq(12), "HEAD", &initial)
                .await
                .is_err()
        );
        let _ = text(root, &["reset", "--hard", &initial]).await?;
        let reference = format!("{}{completed}", super::snapshots(&workspace)?);
        assert_eq!(
            text(root, &["rev-parse", "--verify", &reference]).await?,
            completed
        );
        assert_eq!(
            workspaces(&repo.data).remove(&workspace, id).await?,
            Kept::Local
        );
        assert_eq!(
            text(&repo.source, &["rev-parse", "--verify", &reference]).await?,
            completed
        );
        Ok(())
    }

    #[tokio::test]
    async fn committed_metadata_reports_the_blob_limit()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = repository().await?;
        assert_eq!(
            super::blob(&repo.source, "HEAD:deleted.txt").await?,
            b"keep\n"
        );
        for (file, size) in [
            ("exact.txt", super::BLOB_LIMIT),
            ("large.txt", super::BLOB_LIMIT + 1),
        ] {
            std::fs::write(repo.source.join(file), "x".repeat(size))?;
        }
        let _ = text(&repo.source, &["add", "exact.txt", "large.txt"]).await?;
        let _ =
            text(&repo.source, &["commit", "-qm", "Large metadata"]).await?;
        assert_eq!(
            super::blob(&repo.source, "HEAD:exact.txt").await?.len(),
            super::BLOB_LIMIT
        );
        assert!(
            super::blob(&repo.source, "HEAD:large.txt")
                .await
                .is_err_and(|reason| reason
                    .to_string()
                    .contains("metadata exceeds"))
        );
        Ok(())
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
        let snapshotted = repo.snapshot(&workspace, id).await?;
        let root = PathBuf::from(&workspace.worktree.0);
        std::fs::write(root.join("new.txt"), "left uncommitted\n")?;
        assert_eq!(
            workspaces(&repo.data).remove(&workspace, id).await?,
            Kept::Pushed { remote: url }
        );
        assert!(!root.exists());
        let branch = super::head(&workspace);
        let pushed = text(&remote, &["rev-parse", &branch]).await?;
        assert_eq!(text(&repo.source, &["rev-parse", &branch]).await?, pushed);
        assert_eq!(
            text(&remote, &["show", &format!("{pushed}:new.txt")]).await?,
            "left uncommitted"
        );
        let refs = super::snapshots(&workspace)?;
        assert_eq!(
            text(
                &repo.source,
                &["for-each-ref", "--format=%(objectname)", &refs]
            )
            .await?,
            snapshotted
        );
        assert!(
            text(&repo.source, &["cat-file", "-e", &snapshotted])
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
        assert_eq!(
            workspaces(&repo.data).remove(&workspace, id).await?,
            Kept::Local
        );
        let branch = super::head(&workspace);
        assert_eq!(text(&repo.source, &["rev-parse", &branch]).await?, head);
        assert!(!Path::new(&workspace.worktree.0).exists());
        Ok(())
    }

    #[tokio::test]
    async fn capture_retry_after_a_locked_index_keeps_the_branch_and_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = repository().await?;
        let id = SessionId(u128::from(ulid::Ulid::new()));
        let original = super::host(&repo.source)?;
        let workspace = create(&repo.data, &original, id, None).await?;
        let path = Path::new(&workspace.worktree.0);
        std::fs::write(path.join("new.txt"), "snapshotted\n")?;
        let lock = text(
            path,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "index.lock",
            ],
        )
        .await?;
        let before = text(path, &["rev-parse", "HEAD"]).await?;
        let attempt = vcs::command::SnapshotAttempt(String::from("turn-5"));
        let backend = workspaces(&repo.data);
        std::fs::write(&lock, "other writer")?;
        assert!(
            backend
                .snapshot(&workspace, id, attempt.clone())
                .await
                .is_err_and(|error| error.to_string().contains("index.lock"))
        );
        assert_eq!(text(path, &["rev-parse", "HEAD"]).await?, before);
        assert_eq!(
            std::fs::read_to_string(path.join("new.txt"))?,
            "snapshotted\n"
        );
        assert_eq!(std::fs::read(&lock)?, b"other writer");
        std::fs::remove_file(lock)?;
        let committed = backend.snapshot(&workspace, id, attempt).await?;
        assert_eq!(text(path, &["rev-parse", "HEAD"]).await?, committed);
        assert_eq!(text(path, &["show", "HEAD:new.txt"]).await?, "snapshotted");
        let _ = text(path, &["diff", "--cached", "--quiet"]).await?;
        Ok(())
    }

    #[tokio::test]
    async fn removal_preserves_staged_only_changes_and_refs_after_inspection_errors()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = repository().await?;
        let id = SessionId(u128::from(ulid::Ulid::new()));
        let original = super::host(&repo.source)?;
        let workspace = create(&repo.data, &original, id, None).await?;
        let initial = repo.snapshot(&workspace, id).await?;
        let root = Path::new(&workspace.worktree.0);
        let file = root.join("deleted.txt");
        std::fs::write(&file, "staged only\n")?;
        let _ = text(root, &["add", "deleted.txt"]).await?;
        std::fs::write(&file, "keep\n")?;
        let refs = super::snapshots(&workspace)?;
        let list_refs =
            ["for-each-ref", "--format=%(objectname) %(refname)", &refs];
        let before = text(&repo.source, &list_refs).await?;
        assert_eq!(before, format!("{initial} {refs}{initial}"));
        assert!(
            workspaces(&repo.data)
                .remove(&workspace, id)
                .await
                .is_err_and(|reason| {
                    reason.to_string().contains("partial staging")
                })
        );
        assert_eq!(text(root, &["show", ":deleted.txt"]).await?, "staged only");
        let mut inaccessible = workspace;
        inaccessible.worktree =
            Worktree(super::host(&file.join("unreachable"))?.0);
        assert!(
            workspaces(&repo.data)
                .remove(&inaccessible, id)
                .await
                .is_err_and(|reason| reason
                    .to_string()
                    .contains("worktree path"))
        );
        assert_eq!(text(&repo.source, &list_refs).await?, before);
        Ok(())
    }

    #[tokio::test]
    async fn synchronization_refuses_to_move_a_checked_out_issue_branch()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = repository().await?;
        let identity = home::IssueId::new("fixture", "issue-x")
            .ok_or("fixture identity")?;
        crate::home::ready_issue_branch(
            &repo.source,
            &identity,
            "refs/heads/issue/x",
        )
        .await?;
        let (workspace, issue) = diverged(&repo, "remote-only.txt").await?;
        let remote = repo.root.join("remote.git");
        let _ =
            text(&repo.root, &["init", "-q", "--bare", "remote.git"]).await?;
        let remote_url = remote.display().to_string();
        let _ = text(&repo.source, &["remote", "add", "origin", &remote_url])
            .await?;
        let _ =
            text(&repo.source, &["push", "--quiet", "origin", &issue]).await?;
        let before =
            text(&repo.source, &["rev-parse", &workspace.branch]).await?;
        let _ = text(&repo.source, &["update-ref", &issue, &before]).await?;
        let linked = repo.root.join("issue checkout");
        let linked_name = linked.display().to_string();
        let _ = text(
            &repo.source,
            &["worktree", "add", "-q", &linked_name, "issue/x"],
        )
        .await?;
        std::fs::write(linked.join("deleted.txt"), "dirty\n")?;
        assert!(
            crate::home::ready_issue_branch(&repo.source, &identity, &issue)
                .await
                .is_err_and(|reason| reason.contains(&linked_name))
        );
        assert_eq!(text(&repo.source, &["rev-parse", &issue]).await?, before);
        assert_eq!(
            std::fs::read_to_string(linked.join("deleted.txt"))?,
            "dirty\n"
        );
        assert_eq!(
            text(&linked, &["status", "--porcelain"]).await?,
            " M deleted.txt"
        );
        Ok(())
    }

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
        let attempt = HostSeq(10);
        assert_eq!(landing_receipt(&workspace, &issue, attempt).await?, None);
        assert_eq!(super::land(&workspace, &issue, attempt).await, Ok(1));
        assert_eq!(
            landing_receipt(&workspace, &issue, attempt).await?,
            Some(1)
        );
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
        let attempt = HostSeq(11);
        std::fs::write(
            PathBuf::from(&clashing.worktree.0).join("deleted.txt"),
            "in the session\n",
        )?;
        assert!(
            super::land(&clashing, &base, attempt)
                .await
                .is_err_and(|reason| reason.contains("conflicts")),
            "a conflict lands nothing"
        );
        assert_eq!(landing_receipt(&clashing, &base, attempt).await?, None);
        assert_eq!(
            text(&conflicted.source, &["rev-parse", &base]).await?,
            at,
            "the issue's branch stays where it was"
        );
        Ok(())
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
    async fn assert_invalid_name_fails(
        workspaces: &super::Workspaces,
        workspace: &Workspace,
        id: SessionId,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::ffi::OsStringExt;
        let root = Path::new(&workspace.worktree.0);
        std::fs::write(
            root.join(std::ffi::OsString::from_vec(Vec::from([255]))),
            "unsupported name\n",
        )?;
        let refused = workspaces
            .snapshot(
                workspace,
                id,
                vcs::command::SnapshotAttempt(String::from("invalid-path")),
            )
            .await;
        assert!(
            matches!(refused, Err(GitError::Failed(reason)) if reason.contains("non-UTF-8"))
        );
        Ok(())
    }

    #[cfg(not(unix))]
    async fn assert_invalid_name_fails(
        _workspaces: &super::Workspaces,
        _workspace: &Workspace,
        _id: SessionId,
    ) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }

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
    async fn snapshots_preserve_source_index_head_and_ignored_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = repository().await?;
        let (source_index, source_head) = dirty_source(&repo).await?;
        let id = SessionId(ulid::Ulid::new().into());
        let started_in = host(&repo.source.join("sub dir"))?;
        let workspace = create(&repo.data, &started_in, id, None).await?;
        assert_eq!(create(&repo.data, &started_in, id, None).await?, workspace);
        let root = Path::new(&workspace.worktree.0);
        let initial = repo.snapshot(&workspace, id).await?;
        assert_eq!(
            initial, source_head,
            "a snapshot that changed nothing is the branch's tip"
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
        let changed = repo.snapshot(&workspace, id).await?;
        assert_eq!(repo.snapshot(&workspace, id).await?, changed);
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
        assert_eq!(
            std::fs::read_to_string(root.join("ignored.txt"))?,
            "private\n"
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
        let initial = repo.snapshot(&workspace, id).await?;
        std::fs::write(root.join("binary.dat"), [0, 254, 2])?;
        std::fs::write(root.join("large.txt"), "a".repeat(70000))?;
        std::fs::write(root.join("雪 space.txt"), "hello\n")?;
        let changed = repo.snapshot(&workspace, id).await?;
        std::fs::write(
            root.join("雪 space.txt"),
            "live edit must not appear\n",
        )?;
        let difference = workspaces(&repo.data)
            .diff(&workspace, id, HostSeq(1), &changed, &initial)
            .await?;
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
        let many = repo.snapshot(&workspace, id).await?;
        let difference = workspaces(&repo.data)
            .diff(&workspace, id, HostSeq(1), &many, &changed)
            .await?;
        assert_eq!(difference.extent, DiffExtent::Truncated);
        assert_eq!(difference.files.len(), 100);
        assert_invalid_name_fails(&workspaces(&repo.data), &workspace, id).await
    }

    #[tokio::test]
    async fn deleting_selected_directory_can_be_snapshotted()
    -> Result<(), Box<dyn std::error::Error>> {
        let repo = repository().await?;
        let id = SessionId(ulid::Ulid::new().into());
        let workspace =
            create(&repo.data, &host(&repo.source.join("sub dir"))?, id, None)
                .await?;
        let initial = repo.snapshot(&workspace, id).await?;
        std::fs::remove_dir_all(&workspace.directory.0)?;
        let deleted = repo.snapshot(&workspace, id).await?;
        assert_ne!(deleted, initial);
        assert!(
            text(
                Path::new(&workspace.worktree.0),
                &["show", &format!("{deleted}:sub dir/original.txt")]
            )
            .await
            .is_err()
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
        prevent_repository_discovery(&plain)?;
        assert_eq!(
            matches!(
                create(&repo.data, &host(&plain)?, SessionId(1), None).await,
                Err(GitError::NotRepository)
            )
            .then_some(()),
            Some(())
        );
        std::fs::remove_file(plain.join(".git"))?;
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
        let occupied = repo.data.join("worktrees").join(id.to_string());
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
        let commit = repo.snapshot(&workspace, id).await?;
        let root = Path::new(&workspace.worktree.0);
        let _ = text(root, &["symbolic-ref", "HEAD", "refs/heads/unrelated"])
            .await?;
        assert_eq!(
            matches!(
                workspaces(&repo.data)
                    .diff(&workspace, id, HostSeq(1), &commit, &commit)
                    .await,
                Err(GitError::Failed(_))
            )
            .then_some(()),
            Some(())
        );
        Ok(())
    }
}
