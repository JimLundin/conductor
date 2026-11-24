//! The rules of a session's Git worktree and its snapshots.
//!
//! The names, branches and refs Conductor owns, what Git's output says, how
//! a snapshot's diff is bounded, and the steps that remove a workspace
//! while keeping its branch. The shell runs each requested step.

use alloc::format;
use alloc::string::ToString;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::HostPath;
use crate::log::HostSeq;
use crate::prelude::*;
use crate::session::SessionId;

/// The most of Git's output the host reads, and the largest patch it sends.
const PATCH_LIMIT: usize = 65536;

mod plan;
pub use plan::{Answer, Plan, Step};
/// The most patch text one diff carries.
const TOTAL_LIMIT: usize = 262_144;
/// The most files one diff lists.
const FILE_LIMIT: usize = 100;

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
    /// Stdout, cut off just past the per-file patch limit.
    pub bytes: Vec<u8>,
    pub success: bool,
    /// The start of stderr.
    pub error: String,
}

/// A file a snapshot diff lists, before its patch is read.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    path: String,
    change: DiffChange,
}

#[must_use]
fn failure(context: &str, error: &str) -> GitError {
    GitError::Failed(format!("{context}: {error}"))
}

/// # Errors
///
/// When `actual` isn't `expected`.
fn same(actual: &str, expected: &str, context: &str) -> Result<(), GitError> {
    (actual == expected)
        .then_some(())
        .ok_or_else(|| failure(context, "metadata differs"))
}

/// Whether Git wrote more than the host reads, so the shell stops it.
#[must_use]
const fn oversized(len: usize) -> bool {
    len > PATCH_LIMIT
}

/// The output of a run that must succeed within the capture limit.
///
/// # Errors
///
/// When the output was cut off or Git failed.
fn checked(output: Output, context: &str) -> Result<Vec<u8>, GitError> {
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
fn utf8(bytes: Vec<u8>, context: &str) -> Result<String, GitError> {
    String::from_utf8(bytes)
        .map_err(|error| failure(context, &error.to_string()))
}

/// A path or metadata line Git printed, without its final newlines.
///
/// # Errors
///
/// When the run failed or printed something that isn't UTF-8.
fn text(output: Output, context: &str) -> Result<String, GitError> {
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
fn probed(output: Output, context: &str) -> Result<String, GitError> {
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
fn host_path(path: Option<&str>) -> Result<HostPath, GitError> {
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
fn worktree(session: SessionId) -> String {
    session.to_string()
}

/// The branch Conductor owns for the session.
#[must_use]
pub(crate) fn branch(session: SessionId) -> String {
    format!("{BRANCHES}{}", worktree(session))
}

/// The session a branch Conductor owns belongs to, as its worktree's name.
///
/// # Errors
///
/// When Conductor doesn't own the branch.
fn owner(workspace: &Workspace) -> Result<String, GitError> {
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

/// Where the workspace's snapshots are kept: one ref each, beside the
/// branch, so no commit of the project holds them.
///
/// # Errors
///
/// When Conductor doesn't own the branch.
pub fn snapshots(workspace: &Workspace) -> Result<String, GitError> {
    let session = workspace
        .branch
        .strip_prefix(BRANCHES)
        .ok_or_else(|| failure("snapshot", "not an owned branch"))?;
    Ok(format!("refs/conductor/{session}/"))
}

/// The object a selected subdirectory must be a tree in, at `head`; `None`
/// for the repository's root.
#[must_use]
fn subtree(head: &str, relative: &HostPath) -> Option<String> {
    (!relative.0.is_empty()).then(|| format!("{head}:{}", relative.0))
}

fn reference(workspace: &Workspace, commit: &str) -> Result<String, GitError> {
    if !matches!(commit.len(), 40 | 64)
        || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(failure("snapshot", "expected a full commit object ID"));
    }
    Ok(format!("{}{commit}", snapshots(workspace)?))
}

/// A turn's snapshot is a commit on the session's branch: a new one on
/// its tip, or the tip itself when the turn changed no file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Turn {
    Tip,
    Commit,
}

/// Whether a turn whose worktree holds `tree` makes a commit on a branch
/// whose tip holds `tip_tree`.
#[must_use]
fn turn(tip_tree: &str, tree: &str) -> Turn {
    if tip_tree == tree {
        Turn::Tip
    } else {
        Turn::Commit
    }
}

/// The files to patch from `git diff --name-status -z`, at most
/// the file limit, and whether that is all of them.
///
/// # Errors
///
/// When Git failed, a path isn't UTF-8, or a record is cut short.
fn listing(output: Output) -> Result<(Vec<Entry>, DiffExtent), GitError> {
    if oversized(output.bytes.len()) {
        Ok((Vec::new(), DiffExtent::Truncated))
    } else {
        let metadata =
            String::from_utf8(checked(output, "snapshot file list")?).map_err(
                |error| {
                    failure("diff contains non-UTF-8 paths", &error.to_string())
                },
            )?;
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
        "A" => DiffChange::Write,
        "D" => DiffChange::Delete,
        _ => DiffChange::Edit,
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
fn patch(output: Output, entry: Entry) -> Result<SnapshotFile, GitError> {
    let patch = if oversized(output.bytes.len()) {
        FilePatch::TooLarge
    } else {
        match String::from_utf8(checked(output, "snapshot patch")?) {
            Ok(text) if text.contains("\nBinary files ") => FilePatch::Binary,
            Ok(text) => FilePatch::Text(text),
            Err(error) => FilePatch::Unsupported {
                reason: format!("patch is not UTF-8: {error}"),
            },
        }
    };
    Ok(SnapshotFile {
        path: entry.path,
        change: entry.change,
        patch,
    })
}

/// Keeps the patches within the total patch budget, in order: a patch past it is
/// sent as too large, and the diff as truncated.
#[must_use]
fn budget(
    files: Vec<SnapshotFile>,
    counted: DiffExtent,
) -> (Vec<SnapshotFile>, DiffExtent) {
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
                kept.push(SnapshotFile {
                    patch: FilePatch::TooLarge,
                    ..file
                });
                (kept, budget, DiffExtent::Truncated)
            }
        },
    );
    (kept, extent)
}

/// The workspace's branch as a full ref.
#[must_use]
pub fn head(workspace: &Workspace) -> String {
    format!("refs/heads/{}", workspace.branch)
}

/// Where the branch was kept, from the push the plan ran, if it had one.
#[must_use]
pub(crate) fn kept(pushed: Option<(String, Result<(), String>)>) -> Kept {
    match pushed {
        None => Kept::Local,
        Some((remote, Ok(()))) => Kept::Pushed { remote },
        Some((_, Err(reason))) => Kept::PushFailed { reason },
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Workspace {
    pub repository: HostPath,
    pub original: HostPath,
    pub worktree: HostPath,
    pub directory: HostPath,
    pub branch: String,
}

/// Where a removed session's branch was kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Kept {
    /// In the checkout only: it has no `origin`.
    Local,
    Pushed {
        remote: String,
    },
    /// In the checkout only, since the push failed.
    PushFailed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Snapshot {
    /// The existing `UserMessage` event for this turn; None for initial/autonomous turns.
    pub turn_start: Option<HostSeq>,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotDiff {
    pub snapshot: HostSeq,
    pub files: Vec<SnapshotFile>,
    pub extent: DiffExtent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotFile {
    pub path: String,
    pub change: DiffChange,
    pub patch: FilePatch,
}

/// What a snapshot did to a file, as Git's diff status says.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum DiffChange {
    /// Added: the whole file is new.
    Write,
    /// Modified, or any other status.
    Edit,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum FilePatch {
    Text(String),
    Binary,
    TooLarge,
    Unsupported { reason: String },
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum DiffExtent {
    Complete,
    Truncated,
}

#[cfg(test)]
mod tests;

/// What `git merge-tree --write-tree --name-only --no-messages` gave: the
/// merged tree, or why there is none.
///
/// # Errors
///
/// The files that conflict, or what Git said when it failed otherwise.
fn merged(success: bool, output: &str, error: &str) -> Result<String, String> {
    let mut lines = output.lines();
    let tree = lines.next().unwrap_or_default();
    match (success, error.trim()) {
        (true, _) => Ok(String::from(tree)),
        (false, "") => Err(format!(
            "the session's branch conflicts with the issue's in {}; merge them by hand",
            lines.collect::<Vec<&str>>().join(", ")
        )),
        (false, said) => {
            Err(format!("the branches couldn't be merged: {said}"))
        }
    }
}

/// Where `branch`, a full ref, is checked out, from `git worktree list
/// --porcelain`.
#[must_use]
pub fn checked_out(porcelain: &str, branch: &str) -> Option<String> {
    let wanted = format!("branch {branch}");
    porcelain.split("\0\0").find_map(|record| {
        let mut lines = record.split('\0');
        let path = lines.next()?.strip_prefix("worktree ")?;
        lines.any(|line| line == wanted).then(|| String::from(path))
    })
}

/// The message of the merge commit that lands `session` on `issue`.
#[must_use]
fn merge_message(issue: &str, session: &str) -> String {
    format!("Land {session} on {issue}")
}
