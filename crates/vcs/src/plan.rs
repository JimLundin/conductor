use alloc::{format, string::String, vec::Vec};

use crate::{Checkout, DiffExtent, FilePatch, Kept, SnapshotFile};

pub const PATCH_LIMIT: usize = 65_536;
pub const FILE_LIMIT: usize = 100;
const TOTAL_LIMIT: usize = 262_144;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Snapshot {
    Tip,
    Commit,
}

#[must_use]
pub fn snapshot(tip_tree: &str, tree: &str) -> Snapshot {
    if tip_tree == tree {
        Snapshot::Tip
    } else {
        Snapshot::Commit
    }
}

#[must_use]
pub fn index_preserved(branch: &str, files: &str, staged: &str) -> bool {
    staged == branch || staged == files
}

#[must_use]
pub fn kept(pushed: Option<(String, Result<(), String>)>) -> Kept {
    match pushed {
        None => Kept::Local,
        Some((remote, Ok(()))) => Kept::Pushed { remote },
        Some((_, Err(reason))) => Kept::PushFailed { reason },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trees {
    pub branch: String,
    pub worktree: Option<String>,
}

impl Trees {
    #[must_use]
    pub fn changed(&self) -> Option<&str> {
        self.worktree.as_deref().filter(|tree| *tree != self.branch)
    }
}

#[must_use]
pub fn checked_out<'a>(
    worktrees: &'a [Checkout],
    branch: &str,
) -> Option<&'a str> {
    worktrees
        .iter()
        .find(|tree| tree.branch.as_deref() == Some(branch))
        .map(|tree| tree.path.as_str())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Integration {
    Unchanged,
    CheckedOut(String),
    FastForward,
    Merge,
}

#[must_use]
pub fn integration(
    ahead: u64,
    checked_out: Option<&str>,
    fast_forward: bool,
) -> Integration {
    if ahead == 0 {
        Integration::Unchanged
    } else if let Some(path) = checked_out {
        Integration::CheckedOut(String::from(path))
    } else if fast_forward {
        Integration::FastForward
    } else {
        Integration::Merge
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ownership {
    pub repository: String,
    pub original: String,
    pub worktree: String,
    pub directory: String,
    pub branch: String,
}

impl Ownership {
    #[must_use]
    pub fn marker(&self) -> String {
        format!(
            "{}\n{}\n{}\n{}\n{}\n",
            self.repository,
            self.original,
            self.worktree,
            self.directory,
            self.branch
        )
    }

    #[must_use]
    pub fn matches(&self, recorded: &str) -> bool {
        self.marker() == recorded
    }
}

#[must_use]
pub fn budget(
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

#[must_use]
pub fn bounded_entries(
    entries: Vec<crate::Entry>,
) -> (Vec<crate::Entry>, DiffExtent) {
    let extent = if entries.len() > FILE_LIMIT {
        DiffExtent::Truncated
    } else {
        DiffExtent::Complete
    };
    (entries.into_iter().take(FILE_LIMIT).collect(), extent)
}

pub struct RepositoryFacts {
    pub root: String,
    pub branch: String,
    pub common: String,
    pub source_common: String,
    pub linked: bool,
}

impl Ownership {
    #[must_use]
    pub fn paths(&self, root: &str, selected: &str) -> Option<crate::Error> {
        if root != self.worktree {
            Some(crate::Error::failed(
                "canonical worktree",
                "metadata differs",
            ))
        } else if selected != self.directory {
            Some(crate::Error::failed(
                "selected directory metadata",
                "metadata differs",
            ))
        } else {
            None
        }
    }

    #[must_use]
    pub fn repository(
        &self,
        observed: &RepositoryFacts,
    ) -> Option<crate::Error> {
        if observed.root != self.worktree {
            Some(crate::Error::failed("worktree root", "metadata differs"))
        } else if observed.branch != self.branch {
            Some(crate::Error::failed("owned branch", "metadata differs"))
        } else if observed.common != observed.source_common {
            Some(crate::Error::failed(
                "common repository",
                "metadata differs",
            ))
        } else if !observed.linked {
            Some(crate::Error::failed(
                "workspace",
                "Git directory is outside repository worktrees",
            ))
        } else {
            None
        }
    }
}
