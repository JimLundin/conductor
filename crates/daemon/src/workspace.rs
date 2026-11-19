//! Daemon-owned Git worktrees and immutable checkpoints. The project's HEAD
//! and index are never used as checkpoint storage. What Git's output means
//! and which names, refs and limits apply are [`host::workspace`]; this runs
//! `git` and touches the filesystem.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use host::workspace::{
    Entry, Facts, Output, checked, failure, oversized, owned, probed, same,
};
use n0_future::StreamExt;
use proto::{
    Checkpoint, CheckpointDiff, CheckpointFile, CheckpointId, HostPath, Kept,
    SessionId, Workspace,
};
use tokio::io::AsyncReadExt;
use tokio::process::Child;

pub use host::workspace::GitError;

/// Reads one byte past the limit, so an oversized output shows.
const CAPTURE: u64 = 65537;

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
        .take(CAPTURE)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| failure("reading git stdout", &error.to_string()))?;
    if oversized(bytes.len()) {
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
    let output = execute(&mut git(path, args)).await?;
    host::workspace::text(output, args.first().copied().unwrap_or("git"))
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
    host::workspace::host_path(path.to_str())
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
    let relative = original.strip_prefix(&repository).map_err(|error| {
        failure("original directory outside repository", &error.to_string())
    })?;
    let session = host::workspace::owner(workspace)?;
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
        .map_err(|error| {
            failure("reading workspace ownership", &error.to_string())
        })?;
    same(&top, &workspace.worktree.0, "worktree root")?;
    same(&branch, &host::workspace::head(workspace), "owned branch")?;
    same(
        &host(&common)?.0,
        &host(&source_common)?.0,
        "common repository",
    )?;
    same(
        &recorded,
        &host::workspace::marker(workspace),
        "ownership metadata",
    )?;
    if dir.starts_with(common.join("worktrees")) {
        Ok(dir)
    } else {
        Err(failure(
            "workspace",
            "Git directory is outside repository worktrees",
        ))
    }
}

/// The session's worktree of the repository at `original`, on its own
/// branch from `base`, a ref, or else the repository's committed `HEAD`.
pub async fn create(
    data_dir: &Path,
    original: &HostPath,
    session: SessionId,
    base: Option<&str>,
) -> Result<Workspace, GitError> {
    let original_path = canonical(Path::new(&original.0))?;
    let repository = repository_root(&original_path).await?;
    let head = start_commit(&repository, base).await?;
    let relative =
        original_path.strip_prefix(&repository).map_err(|error| {
            failure("selected subdirectory", &error.to_string())
        })?;
    check_subdirectory(&repository, head.trim(), relative).await?;
    let parent = data_dir.join("worktrees");
    std::fs::create_dir_all(&parent).map_err(|error| {
        failure("creating worktree parent", &error.to_string())
    })?;
    let path = canonical(&parent)?.join(host::workspace::worktree(session));
    let workspace = Workspace {
        repository: host(&repository)?,
        original: host(&original_path)?,
        worktree: host(&path)?,
        directory: host(&path.join(relative))?,
        branch: host::workspace::branch(session),
    };
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) => match error.kind() {
            std::io::ErrorKind::NotFound => {
                add_worktree(&repository, &workspace, head.trim(), &path)
                    .await?;
            }
            _ => return Err(failure("worktree path", &error.to_string())),
        },
    }
    let _ = validate(&workspace).await?;
    Ok(workspace)
}

/// The commit a session's branch starts at: `base`'s, or the committed
/// `HEAD`'s.
async fn start_commit(
    repository: &Path,
    base: Option<&str>,
) -> Result<String, GitError> {
    match base {
        Some(reference) => {
            let wanted = format!("{reference}^{{commit}}");
            text(repository, &["rev-parse", "--verify", &wanted]).await
        }
        None => committed_head(repository).await,
    }
}

async fn repository_root(original: &Path) -> Result<PathBuf, GitError> {
    let probe =
        execute(&mut git(original, &["rev-parse", "--show-toplevel"])).await?;
    let root = probed(probe, "repository root")?;
    canonical(Path::new(root.trim_end_matches('\n')))
}

async fn committed_head(repository: &Path) -> Result<String, GitError> {
    let head = execute(&mut git(
        repository,
        &["rev-parse", "--verify", "HEAD^{commit}"],
    ))
    .await?;
    probed(head, "committed HEAD")
}

async fn check_subdirectory(
    repository: &Path,
    head: &str,
    relative: &Path,
) -> Result<(), GitError> {
    match host::workspace::subtree(head, &host(relative)?) {
        None => Ok(()),
        Some(object) => {
            let kind = text(repository, &["cat-file", "-t", &object]).await?;
            same(
                &kind,
                "tree",
                "selected subdirectory must exist in committed HEAD",
            )
        }
    }
}

async fn add_worktree(
    repository: &Path,
    workspace: &Workspace,
    head: &str,
    path: &Path,
) -> Result<(), GitError> {
    let _ = text(
        repository,
        &[
            "worktree",
            "add",
            "-b",
            &workspace.branch,
            &workspace.worktree.0,
            head,
        ],
    )
    .await?;
    let dir = git_dir(path).await?;
    let mut ownership = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join("conductor-workspace"))
        .map_err(|error| {
            failure("recording worktree ownership", &error.to_string())
        })?;
    std::io::Write::write_all(
        &mut ownership,
        host::workspace::marker(workspace).as_bytes(),
    )
    .map_err(|error| {
        failure("recording worktree ownership", &error.to_string())
    })
}

async fn owned_commit(
    workspace: &Workspace,
    commit: &str,
) -> Result<(), GitError> {
    let refs = text(
        Path::new(&workspace.worktree.0),
        &[
            "for-each-ref",
            "--format=%(objectname)",
            "--count=1",
            &format!("--points-at={commit}"),
            &host::workspace::checkpoints(workspace)?,
        ],
    )
    .await?;
    owned(&refs, commit)
}

/// The tree of everything in the worktree at `path` that Git doesn't
/// ignore, staged on top of `base` in the scratch `index`.
async fn tree(
    path: &Path,
    base: &str,
    index: &Path,
) -> Result<String, GitError> {
    let _ = checked(
        execute(git(path, &["read-tree", base]).env("GIT_INDEX_FILE", index))
            .await?,
        "initializing checkpoint index",
    )?;
    let _ = checked(
        execute(
            git(path, &["add", "-A", "--", "."]).env("GIT_INDEX_FILE", index),
        )
        .await?,
        "staging checkpoint",
    )?;
    host::workspace::utf8(
        checked(
            execute(git(path, &["write-tree"]).env("GIT_INDEX_FILE", index))
                .await?,
            "checkpoint tree",
        )?,
        "checkpoint tree",
    )
    .map(|tree| tree.trim().to_owned())
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
    let tree = tree(path, &parent, index).await?;
    let reference = host::workspace::reference(workspace, checkpoint)?;
    let existing =
        execute(&mut git(path, &["rev-parse", "--verify", &reference])).await?;
    if let Some(commit) = host::workspace::existing(existing)? {
        let recorded_tree =
            text(path, &["rev-parse", &format!("{commit}^{{tree}}")]).await?;
        let recorded_parent =
            text(path, &["rev-parse", &format!("{commit}^")]).await?;
        host::workspace::reuse(
            (&recorded_tree, &recorded_parent),
            &tree,
            &parent,
        )?;
        Ok(commit)
    } else {
        let commit = text_commit(
            path,
            &tree,
            &[&parent],
            &host::workspace::message(checkpoint),
        )
        .await?;
        let _ = text(path, &["update-ref", &reference, &commit, ""]).await?;
        Ok(commit)
    }
}

async fn text_commit(
    path: &Path,
    tree: &str,
    parents: &[&str],
    message: &str,
) -> Result<String, GitError> {
    let mut args = Vec::from(["commit-tree", tree]);
    for parent in parents {
        args.extend(["-p", parent]);
    }
    args.extend(["-m", message]);
    let output = execute(
        git(path, &args)
            .env("GIT_AUTHOR_NAME", "Conductor")
            .env("GIT_AUTHOR_EMAIL", "conductor@localhost")
            .env("GIT_COMMITTER_NAME", "Conductor")
            .env("GIT_COMMITTER_EMAIL", "conductor@localhost"),
    )
    .await?;
    host::workspace::utf8(
        checked(output, "creating checkpoint commit")?,
        "checkpoint commit",
    )
    .map(|text| text.trim().to_owned())
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

pub async fn snapshot(
    workspace: &Workspace,
    session: SessionId,
    checkpoint: CheckpointId,
    previous: Option<&str>,
) -> Result<String, GitError> {
    let dir = validate(workspace).await?;
    same(
        &workspace.branch,
        &host::workspace::branch(session),
        "session owning workspace",
    )?;
    let scratch = Scratch::create(&dir)?;
    snapshot_index(workspace, checkpoint, previous, &scratch.path.join("index"))
        .await
}

pub async fn preflight(
    workspace: &Workspace,
    commit: &str,
) -> Result<(), GitError> {
    let _ = validate(workspace).await?;
    owned_commit(workspace, commit).await
}

pub async fn restore(
    workspace: &Workspace,
    commit: &str,
) -> Result<(), GitError> {
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
    host::workspace::patch(output, entry)
}

pub async fn diff(
    workspace: &Workspace,
    checkpoint: &Checkpoint,
    parent: &Checkpoint,
) -> Result<CheckpointDiff, GitError> {
    let _ = validate(workspace).await?;
    let path = Path::new(&workspace.worktree.0);
    let checkpoint_ref = host::workspace::reference(workspace, checkpoint.id)?;
    let parent_ref = host::workspace::reference(workspace, parent.id)?;
    let actual =
        text(path, &["rev-parse", "--verify", &checkpoint_ref]).await?;
    let actual_parent =
        text(path, &["rev-parse", "--verify", &parent_ref]).await?;
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
    let (entries, counted) = host::workspace::listing(output)?;
    let patched = n0_future::stream::iter(entries)
        .then(|entry| async move {
            patch(workspace, checkpoint, parent, entry).await
        })
        .collect::<Vec<Result<CheckpointFile, GitError>>>()
        .await
        .into_iter()
        .collect::<Result<Vec<CheckpointFile>, GitError>>()?;
    let (files, extent) = host::workspace::budget(patched, counted);
    Ok(CheckpointDiff {
        checkpoint: checkpoint.id,
        parent: parent.id,
        files,
        extent,
    })
}

/// Removes the workspace by [`host::workspace::plan`]: commits what it
/// holds onto its branch, pushes that to `origin`, and removes the worktree
/// and its checkpoint refs. The branch stays, and the answer says where.
///
/// # Errors
///
/// When a step other than the push fails, which stops the removal.
pub async fn remove(workspace: &Workspace) -> Result<Kept, GitError> {
    let facts = facts(workspace).await?;
    let mut pushed = None;
    for step in host::workspace::plan(workspace, &facts)? {
        pushed = run(workspace, step).await?.or(pushed);
    }
    let kept = host::workspace::kept(pushed);
    tracing::info!(branch = workspace.branch, ?kept, "removed a workspace");
    Ok(kept)
}

/// The push a step ran, and how it went.
type Pushed = Option<(String, Result<(), String>)>;

async fn run(
    workspace: &Workspace,
    step: host::workspace::Step,
) -> Result<Pushed, GitError> {
    use host::workspace::Step;
    let repository = Path::new(&workspace.repository.0);
    match step {
        Step::Commit { tree } => keep(workspace, &tree).await.map(|()| None),
        Step::Push { remote, args } => {
            let words = args.iter().map(String::as_str).collect::<Vec<&str>>();
            let pushed = text(repository, &words)
                .await
                .map(drop)
                .map_err(|error| error.to_string());
            Ok(Some((remote, pushed)))
        }
        Step::Remove => {
            let _ = validate(workspace).await?;
            text(
                repository,
                &["worktree", "remove", "--force", &workspace.worktree.0],
            )
            .await
            .map(|_| None)
        }
        Step::Prune => {
            text(repository, &["worktree", "prune"]).await.map(|_| None)
        }
        Step::Forget(prefix) => {
            forget(repository, &prefix).await.map(|()| None)
        }
    }
}

/// What removing the workspace needs to know: the tree of its branch, the
/// tree of what it holds now if it is still on disk, and its checkout's
/// `origin`.
async fn facts(workspace: &Workspace) -> Result<Facts, GitError> {
    let repository = Path::new(&workspace.repository.0);
    let branch = format!("{}^{{tree}}", host::workspace::head(workspace));
    let head_tree = text(repository, &["rev-parse", &branch]).await?;
    let tree = if Path::new(&workspace.worktree.0).is_dir() {
        let dir = validate(workspace).await?;
        let scratch = Scratch::create(&dir)?;
        let path = Path::new(&workspace.worktree.0);
        Some(tree(path, "HEAD", &scratch.path.join("index")).await?)
    } else {
        None
    };
    let origin = text(repository, &["remote", "get-url", "origin"])
        .await
        .ok();
    Ok(Facts {
        head_tree,
        tree,
        origin,
    })
}

/// Commits `tree` on the worktree's branch, after its head, so the branch
/// holds what the worktree had not committed.
async fn keep(workspace: &Workspace, tree: &str) -> Result<(), GitError> {
    let path = Path::new(&workspace.worktree.0);
    let head = text(path, &["rev-parse", "HEAD"]).await?;
    let commit =
        text_commit(path, tree, &[&head], host::workspace::KEPT).await?;
    let branch = host::workspace::head(workspace);
    text(path, &["update-ref", &branch, &commit, &head])
        .await
        .map(drop)
}

/// Deletes every ref under `prefix`, as many at a time as one capture
/// lists.
async fn forget(repository: &Path, prefix: &str) -> Result<(), GitError> {
    let mut names = refs(repository, prefix).await?;
    while !names.is_empty() {
        for name in names.lines() {
            let _ = text(repository, &["update-ref", "-d", name]).await?;
        }
        names = refs(repository, prefix).await?;
    }
    Ok(())
}

async fn refs(repository: &Path, prefix: &str) -> Result<String, GitError> {
    text(
        repository,
        &["for-each-ref", "--count=500", "--format=%(refname)", prefix],
    )
    .await
}

/// Lands the session's branch on the issue's, `issue`, a full ref, in the
/// checkout: commits what its worktree holds first, then moves the issue's
/// branch on as `host::finish::Branches` says. The commits it landed.
///
/// # Errors
///
/// Why nothing landed.
pub async fn land(workspace: &Workspace, issue: &str) -> Result<u64, String> {
    keep_uncommitted(workspace)
        .await
        .map_err(|err| err.to_string())?;
    let repository = Path::new(&workspace.repository.0);
    let session = host::workspace::head(workspace);
    let branches = branches(repository, issue, &session)
        .await
        .map_err(|err| err.to_string())?;
    let ahead = branches.ahead;
    let moved = match branches.landing() {
        host::finish::Landing::Nothing => return Ok(0),
        host::finish::Landing::Refuse(reason) => return Err(reason),
        host::finish::Landing::FastForward => {
            fast_forward(repository, issue, &session).await
        }
        host::finish::Landing::Merge => {
            merge(repository, issue, &session).await
        }
    };
    moved.map(|()| ahead).map_err(|err| err.to_string())
}

/// Commits the worktree's changes onto its branch, if it has any.
async fn keep_uncommitted(workspace: &Workspace) -> Result<(), GitError> {
    let found = facts(workspace).await?;
    match found.tree.filter(|tree| *tree != found.head_tree) {
        Some(tree) => keep(workspace, &tree).await,
        None => Ok(()),
    }
}

async fn branches(
    repository: &Path,
    issue: &str,
    session: &str,
) -> Result<host::finish::Branches, GitError> {
    let range = format!("{issue}..{session}");
    let counted = text(repository, &["rev-list", "--count", &range]).await?;
    let fast_forward =
        text(repository, &["merge-base", "--is-ancestor", issue, session])
            .await
            .is_ok();
    let listed = text(repository, &["worktree", "list", "--porcelain"]).await?;
    let ahead = counted
        .trim()
        .parse()
        .map_err(|_| failure("landing", "Git counted no commits"))?;
    Ok(host::finish::Branches {
        ahead,
        fast_forward,
        checked_out: host::finish::checked_out(&listed, issue),
    })
}

async fn fast_forward(
    repository: &Path,
    issue: &str,
    session: &str,
) -> Result<(), GitError> {
    let old = text(repository, &["rev-parse", issue]).await?;
    let new = text(repository, &["rev-parse", session]).await?;
    text(repository, &["update-ref", issue, &new, &old])
        .await
        .map(drop)
}

/// A merge commit of both branches on the issue's, made without a
/// worktree.
async fn merge(
    repository: &Path,
    issue: &str,
    session: &str,
) -> Result<(), GitError> {
    let output = execute(&mut git(
        repository,
        &[
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            issue,
            session,
        ],
    ))
    .await?;
    let tree = host::finish::merged(
        output.success,
        &String::from_utf8_lossy(&output.bytes),
        &output.error,
    )
    .map_err(|reason| failure("landing", &reason))?;
    let old = text(repository, &["rev-parse", issue]).await?;
    let new = text(repository, &["rev-parse", session]).await?;
    let message = host::finish::merge_message(issue, session);
    let commit =
        text_commit(repository, tree.trim(), &[&old, &new], &message).await?;
    text(repository, &["update-ref", issue, &commit, &old])
        .await
        .map(drop)
}

/// Pushes `branch`, a full ref, to the checkout's `origin`, never forced:
/// the remote and how it went, or `None` without an `origin`.
pub async fn push_branch(
    workspace: &Workspace,
    branch: &str,
) -> Option<(String, Result<(), String>)> {
    let repository = Path::new(&workspace.repository.0);
    let origin = text(repository, &["remote", "get-url", "origin"])
        .await
        .ok()?;
    let refspec = format!("{branch}:{branch}");
    let pushed = text(repository, &["push", "--quiet", "origin", &refspec])
        .await
        .map(drop)
        .map_err(|err| err.to_string());
    Some((origin, pushed))
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
            text(root, &["show", &format!("{initial}:deleted.txt")]).await?,
            "keep"
        );
        std::fs::write(root.join("sub dir/original.txt"), "changed\n")?;
        executable(&root.join("sub dir/original.txt"))?;
        std::fs::remove_file(root.join("deleted.txt"))?;
        std::fs::write(root.join("space 雪.txt"), "added\n")?;
        std::fs::write(root.join("binary.dat"), [0, 254, 2])?;
        std::fs::write(root.join("ignored.txt"), "private\n")?;
        let worktree_index =
            std::fs::read(super::git_dir(root).await?.join("index"))?;
        let changed =
            snapshot(&workspace, id, CheckpointId(1), Some(&initial)).await?;
        assert_eq!(
            snapshot(&workspace, id, CheckpointId(1), Some(&initial)).await?,
            changed
        );
        assert_eq!(
            std::fs::read(super::git_dir(root).await?.join("index"))?,
            worktree_index
        );
        assert_eq!(
            text(root, &["show", &format!("{changed}:space 雪.txt")]).await?,
            "added"
        );
        assert_eq!(text(root, &["rev-parse", "HEAD"]).await?, source_head);
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
        assert_eq!(text(root, &["rev-parse", "HEAD"]).await?, source_head);
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
