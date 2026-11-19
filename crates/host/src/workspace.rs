//! The rules of a session's Git worktree and its checkpoints.
//!
//! The names, branches and refs Conductor owns, what Git's output says, how
//! a checkpoint's diff is bounded, and the steps that remove a workspace
//! while keeping its branch. The shell runs `git` and touches the
//! filesystem.

use alloc::format;
use alloc::string::ToString;

use proto::{
    Change, CheckpointFile, CheckpointId, DiffExtent, FilePatch, HostPath,
    Kept, SessionId, Workspace,
};

use crate::prelude::*;

/// The most of Git's output the host reads, and the largest patch it sends.
pub const PATCH_LIMIT: usize = 65536;
/// The most patch text one diff carries.
pub const TOTAL_LIMIT: usize = 262_144;
/// The most files one diff lists.
pub const FILE_LIMIT: usize = 100;

const BRANCHES: &str = "conductor/";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitError {
    #[error(
        "the selected directory is not a Git repository with a committed HEAD"
    )]
    NotRepository,
    #[error("{0}")]
    Failed(String),
}

/// What one `git` run gave back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// Stdout, cut off just past [`PATCH_LIMIT`].
    pub bytes: Vec<u8>,
    pub success: bool,
    /// The start of stderr.
    pub error: String,
}

/// A file a checkpoint diff lists, before its patch is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub change: Change,
}

#[must_use]
pub fn failure(context: &str, error: &str) -> GitError {
    GitError::Failed(format!("{context}: {error}"))
}

/// # Errors
///
/// When `actual` isn't `expected`.
pub fn same(
    actual: &str,
    expected: &str,
    context: &str,
) -> Result<(), GitError> {
    (actual == expected)
        .then_some(())
        .ok_or_else(|| failure(context, "metadata differs"))
}

/// Whether Git wrote more than the host reads, so the shell stops it.
#[must_use]
pub const fn oversized(len: usize) -> bool {
    len > PATCH_LIMIT
}

/// The output of a run that must succeed within the capture limit.
///
/// # Errors
///
/// When the output was cut off or Git failed.
pub fn checked(output: Output, context: &str) -> Result<Vec<u8>, GitError> {
    match (oversized(output.bytes.len()), output.success) {
        (true, _) => {
            Err(failure(context, "Git output exceeded the capture limit"))
        }
        (false, true) => Ok(output.bytes),
        (false, false) => Err(failure(context, &output.error)),
    }
}

/// # Errors
///
/// When the bytes aren't UTF-8.
pub fn utf8(bytes: Vec<u8>, context: &str) -> Result<String, GitError> {
    String::from_utf8(bytes)
        .map_err(|error| failure(context, &error.to_string()))
}

/// A path or metadata line Git printed, without its final newlines.
///
/// # Errors
///
/// When the run failed or printed something that isn't UTF-8.
pub fn text(output: Output, context: &str) -> Result<String, GitError> {
    utf8(
        checked(output, context)?,
        "Git path or metadata is not UTF-8",
    )
    .map(|text| text.trim_end_matches('\n').to_owned())
}

/// The output of a probe whose failure means the directory has no
/// repository with a committed `HEAD`.
///
/// # Errors
///
/// [`GitError::NotRepository`] when Git failed.
pub fn probed(output: Output, context: &str) -> Result<String, GitError> {
    if output.success {
        utf8(checked(output, context)?, context)
    } else {
        Err(GitError::NotRepository)
    }
}

/// A path the host can record: UTF-8 (`None` when it isn't), with no
/// newline, since the ownership marker is one path per line.
///
/// # Errors
///
/// When the path is neither.
pub fn host_path(path: Option<&str>) -> Result<HostPath, GitError> {
    let text = path.ok_or_else(|| {
        failure("Git workspace path", "non-UTF-8 paths are unsupported")
    })?;
    if text.contains('\n') {
        Err(failure(
            "Git workspace path",
            "newline paths are unsupported",
        ))
    } else {
        Ok(HostPath(String::from(text)))
    }
}

/// The name of the session's worktree directory.
#[must_use]
pub fn worktree(session: SessionId) -> String {
    ulid::Ulid::from(session.0).to_string()
}

/// The branch Conductor owns for the session.
#[must_use]
pub fn branch(session: SessionId) -> String {
    format!("{BRANCHES}{}", worktree(session))
}

/// The session a branch Conductor owns belongs to, as its worktree's name.
///
/// # Errors
///
/// When Conductor doesn't own the branch.
pub fn owner(workspace: &Workspace) -> Result<String, GitError> {
    let session = workspace
        .branch
        .strip_prefix(BRANCHES)
        .ok_or_else(|| failure("workspace branch", "not owned by Conductor"))?;
    let _ = session
        .parse::<ulid::Ulid>()
        .map_err(|error| failure("workspace branch", &error.to_string()))?;
    Ok(String::from(session))
}

/// What the worktree's Git directory records about its owner, one field
/// per line.
#[must_use]
pub fn marker(workspace: &Workspace) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n",
        workspace.repository.0,
        workspace.original.0,
        workspace.worktree.0,
        workspace.directory.0,
        workspace.branch
    )
}

/// Where the workspace's checkpoints are kept: one ref each, beside the
/// branch, so no commit of the project holds them.
///
/// # Errors
///
/// When Conductor doesn't own the branch.
pub fn checkpoints(workspace: &Workspace) -> Result<String, GitError> {
    let session = workspace
        .branch
        .strip_prefix(BRANCHES)
        .ok_or_else(|| failure("checkpoint", "not an owned branch"))?;
    Ok(format!("refs/conductor/{session}/"))
}

/// The checkpoint's ref.
///
/// # Errors
///
/// When Conductor doesn't own the branch.
pub fn reference(
    workspace: &Workspace,
    checkpoint: CheckpointId,
) -> Result<String, GitError> {
    Ok(format!("{}{}", checkpoints(workspace)?, checkpoint.0))
}

/// The object a selected subdirectory must be a tree in, at `head`; `None`
/// for the repository's root.
#[must_use]
pub fn subtree(head: &str, relative: &HostPath) -> Option<String> {
    (!relative.0.is_empty()).then(|| format!("{head}:{}", relative.0))
}

/// `refs` are the objects `for-each-ref` lists among the workspace's
/// checkpoints at `commit`.
///
/// # Errors
///
/// When none is `commit`, so it isn't one of the workspace's checkpoints.
pub fn owned(refs: &str, commit: &str) -> Result<(), GitError> {
    refs.lines()
        .any(|oid| oid == commit)
        .then_some(())
        .ok_or_else(|| {
            failure(
                "checkpoint",
                "commit is not an immutable checkpoint of this workspace",
            )
        })
}

/// The commit a checkpoint's ref already names, or `None` when Git found
/// no such ref and the checkpoint is new.
///
/// # Errors
///
/// When Git's answer can't be read.
pub fn existing(output: Output) -> Result<Option<String>, GitError> {
    if output.success {
        utf8(
            checked(output, "existing checkpoint")?,
            "existing checkpoint",
        )
        .map(|commit| Some(commit.trim().to_owned()))
    } else {
        Ok(None)
    }
}

/// A checkpoint id is immutable: taken again, it must record the same tree
/// on the same parent.
///
/// # Errors
///
/// When the recorded tree or parent differs.
pub fn reuse(
    recorded: (&str, &str),
    tree: &str,
    parent: &str,
) -> Result<(), GitError> {
    let (recorded_tree, recorded_parent) = recorded;
    same(
        recorded_tree,
        tree,
        "checkpoint id already records a different tree",
    )?;
    same(
        recorded_parent,
        parent,
        "checkpoint id already records a different parent",
    )
}

/// The checkpoint commit's message.
#[must_use]
pub fn message(checkpoint: CheckpointId) -> String {
    format!("Conductor checkpoint {}", checkpoint.0)
}

/// The files to patch from `git diff --name-status -z`, at most
/// [`FILE_LIMIT`], and whether that is all of them.
///
/// # Errors
///
/// When Git failed, a path isn't UTF-8, or a record is cut short.
pub fn listing(output: Output) -> Result<(Vec<Entry>, DiffExtent), GitError> {
    if oversized(output.bytes.len()) {
        Ok((Vec::new(), DiffExtent::Truncated))
    } else {
        let metadata =
            String::from_utf8(checked(output, "checkpoint file list")?)
                .map_err(|error| {
                    failure("diff contains non-UTF-8 paths", &error.to_string())
                })?;
        let entries = entries(&metadata)?;
        let extent = if entries.len() > FILE_LIMIT {
            DiffExtent::Truncated
        } else {
            DiffExtent::Complete
        };
        Ok((entries.into_iter().take(FILE_LIMIT).collect(), extent))
    }
}

fn entries(metadata: &str) -> Result<Vec<Entry>, GitError> {
    match metadata {
        "" => Ok(Vec::new()),
        _ => metadata
            .trim_end_matches('\0')
            .split('\0')
            .collect::<Vec<&str>>()
            .chunks(2)
            .map(entry)
            .collect(),
    }
}

fn entry(parts: &[&str]) -> Result<Entry, GitError> {
    let status = parts
        .first()
        .ok_or_else(|| failure("diff metadata", "missing status"))?;
    let path = parts
        .get(1)
        .ok_or_else(|| failure("diff metadata", "missing path"))?;
    let change = match *status {
        "A" => Change::Write,
        "D" => Change::Delete,
        _ => Change::Edit,
    };
    Ok(Entry {
        path: String::from(*path),
        change,
    })
}

/// The entry's file, from the output of its `git diff`.
///
/// # Errors
///
/// When Git failed.
pub fn patch(output: Output, entry: Entry) -> Result<CheckpointFile, GitError> {
    let patch = if oversized(output.bytes.len()) {
        FilePatch::TooLarge
    } else {
        match String::from_utf8(checked(output, "checkpoint patch")?) {
            Ok(text) if text.contains("\nBinary files ") => FilePatch::Binary,
            Ok(text) => FilePatch::Text(text),
            Err(error) => FilePatch::Unsupported {
                reason: format!("patch is not UTF-8: {error}"),
            },
        }
    };
    Ok(CheckpointFile {
        path: entry.path,
        change: entry.change,
        patch,
    })
}

/// Keeps the patches within [`TOTAL_LIMIT`], in order: a patch past it is
/// sent as too large, and the diff as truncated.
#[must_use]
pub fn budget(
    files: Vec<CheckpointFile>,
    counted: DiffExtent,
) -> (Vec<CheckpointFile>, DiffExtent) {
    let (kept, _, extent) = files.into_iter().fold(
        (Vec::new(), TOTAL_LIMIT, counted),
        |(mut kept, budget, so_far), file| {
            let size = match &file.patch {
                FilePatch::Text(text) => text.len(),
                FilePatch::Binary
                | FilePatch::TooLarge
                | FilePatch::Unsupported { .. } => 0,
            };
            if let Some(remaining) = budget.checked_sub(size) {
                kept.push(file);
                (kept, remaining, so_far)
            } else {
                kept.push(CheckpointFile {
                    patch: FilePatch::TooLarge,
                    ..file
                });
                (kept, budget, DiffExtent::Truncated)
            }
        },
    );
    (kept, extent)
}

/// The message of the commit that keeps what a removed workspace had not
/// committed.
pub const KEPT: &str =
    "Conductor: the workspace's uncommitted changes when it was removed";

/// What the shell knows of a workspace before removing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// The tree of the branch's head.
    pub head_tree: String,
    /// The tree of everything in the worktree that Git doesn't ignore;
    /// `None` when its directory is gone.
    pub tree: Option<String>,
    /// The checkout's `origin`, if it has one.
    pub origin: Option<String>,
}

/// One step of removing a workspace, in order. A failed `Commit` or
/// `Remove` stops the removal, so nothing is lost; a failed `Push` keeps
/// the branch locally and goes on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Commit `tree` on the branch, after its head, with [`KEPT`].
    Commit { tree: String },
    /// Run `git` with these arguments in the checkout.
    Push { remote: String, args: Vec<String> },
    /// Remove the worktree's directory.
    Remove,
    /// Forget the worktree whose directory is gone already.
    Prune,
    /// Delete every ref under this prefix: the session's checkpoints.
    Forget(String),
}

/// The steps that remove `workspace` and keep its branch. Only a branch of
/// Conductor's own is pushed, only to `origin`, and never forced.
///
/// # Errors
///
/// When Conductor doesn't own the workspace's branch.
pub fn plan(
    workspace: &Workspace,
    facts: &Facts,
) -> Result<Vec<Step>, GitError> {
    let _ = owner(workspace)?;
    let branch = &workspace.branch;
    let commit = facts
        .tree
        .clone()
        .filter(|tree| *tree != facts.head_tree)
        .map(|tree| Step::Commit { tree });
    let push = facts.origin.clone().map(|remote| Step::Push {
        remote,
        args: Vec::from([
            String::from("push"),
            String::from("--quiet"),
            String::from("origin"),
            format!("refs/heads/{branch}:refs/heads/{branch}"),
        ]),
    });
    let remove = match facts.tree {
        Some(_) => Step::Remove,
        None => Step::Prune,
    };
    Ok(commit
        .into_iter()
        .chain(push)
        .chain([remove, Step::Forget(checkpoints(workspace)?)])
        .collect())
}

/// Where the branch was kept, from the push the plan ran, if it had one.
#[must_use]
pub fn kept(pushed: Option<(String, Result<(), String>)>) -> Kept {
    match pushed {
        None => Kept::Local,
        Some((remote, Ok(()))) => Kept::Pushed { remote },
        Some((_, Err(reason))) => Kept::PushFailed { reason },
    }
}

#[cfg(test)]
mod tests;
