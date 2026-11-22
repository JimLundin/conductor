//! Per-turn changes. The pane holds only which diffs are open.

use alloc::rc::Rc;

use crate::prelude::*;
use client::host_view::TurnSnapshot;
use proto::{
    Change, CommandResult, DiffExtent, FilePatch, HostSeq, SnapshotDiff,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffState {
    Hidden,
    /// Waiting for the answer to this request.
    Loading(u64),
    Loaded(Rc<Preview>),
    Failed(String),
}

impl DiffState {
    pub fn answered(answer: Result<CommandResult, String>, host: &str) -> Self {
        match answer {
            Ok(CommandResult::Diff(changes)) => {
                Self::Loaded(Rc::new(preview(changes)))
            }
            Ok(CommandResult::Rejected(reason)) => Self::Failed(reason.label()),
            Ok(
                CommandResult::Accepted
                | CommandResult::Started { .. }
                | CommandResult::IssueWritten { .. }
                | CommandResult::PushPublicKey(_)
                | CommandResult::PushUnavailable
                | CommandResult::FleetStatus(_)
                | CommandResult::FleetInvite(_)
                | CommandResult::FleetPairing(_),
            ) => Self::Failed(format!("{host} did not return changes.")),
            Err(reason) => Self::Failed(reason),
        }
    }
}

/// The changes of the turn a snapshot ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnChanges {
    pub key: String,
    pub snapshot: HostSeq,
    /// The id of the button that opens the diff, which takes the focus
    /// back when it closes.
    pub button: String,
    pub button_disabled: bool,
    pub panel: DiffPanel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffPanel {
    Hidden,
    Loading,
    Failed(String),
    Loaded(Rc<Preview>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// Why no file shows.
    pub empty: Option<&'static str>,
    pub files: Vec<DiffFile>,
    pub truncated: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    /// How the file changed, and its path.
    pub path: String,
    pub patch: Patch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Patch {
    Text { text: String, label: String },
    Note(String),
}

pub fn changes(turn: &TurnSnapshot, diff: Option<&DiffState>) -> TurnChanges {
    let loading = matches!(diff, Some(DiffState::Loading(_)));
    TurnChanges {
        key: format!("snapshot-{}", turn.snapshot.0),
        snapshot: turn.snapshot,
        button: changes_id(turn.snapshot),
        button_disabled: loading,
        panel: match diff {
            None | Some(DiffState::Hidden) => DiffPanel::Hidden,
            Some(DiffState::Loading(_)) => DiffPanel::Loading,
            Some(DiffState::Failed(reason)) => {
                DiffPanel::Failed(reason.clone())
            }
            Some(DiffState::Loaded(changes)) => {
                DiffPanel::Loaded(Rc::clone(changes))
            }
        },
    }
}

fn preview(changes: SnapshotDiff) -> Preview {
    Preview {
        empty: match (changes.files.first(), changes.extent) {
            (None, DiffExtent::Complete) => {
                Some("No file changes in this turn.")
            }
            (None, DiffExtent::Truncated) => {
                Some("No files fit in this preview.")
            }
            (Some(_), _) => None,
        },
        files: changes
            .files
            .into_iter()
            .map(|file| DiffFile {
                path: format!("{} {}", change_label(file.change), file.path),
                patch: match file.patch {
                    FilePatch::Text(patch) => Patch::Text {
                        text: patch,
                        label: format!("{} patch", file.path),
                    },
                    FilePatch::Binary => Patch::Note(String::from(
                        "Binary file changed. Preview unavailable.",
                    )),
                    FilePatch::TooLarge => Patch::Note(String::from(
                        "File exceeds the preview limit.",
                    )),
                    FilePatch::Unsupported { reason } => {
                        Patch::Note(format!("Preview unavailable: {reason}"))
                    }
                },
            })
            .collect(),
        truncated: match changes.extent {
            DiffExtent::Complete => None,
            DiffExtent::Truncated => {
                Some("Preview limit reached. Some changes are omitted.")
            }
        },
    }
}

const fn change_label(change: Change) -> &'static str {
    match change {
        Change::Write => "Added",
        Change::Edit => "Modified",
        Change::Delete => "Deleted",
    }
}

pub fn changes_id(snapshot: HostSeq) -> String {
    format!("snapshot-{}-changes", snapshot.0)
}
