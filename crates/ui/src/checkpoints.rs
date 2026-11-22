//! Per-turn changes. The pane holds only which diffs are open.

use alloc::rc::Rc;

use crate::prelude::*;
use client::host_view::TurnCheckpoint;
use proto::{
    Change, CheckpointDiff, CommandResult, DiffExtent, FilePatch, HostSeq,
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
                | CommandResult::IssueCreated { .. }
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

/// The changes of the turn a checkpoint ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnChanges {
    pub key: String,
    pub checkpoint: HostSeq,
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

pub fn changes(turn: &TurnCheckpoint, diff: Option<&DiffState>) -> TurnChanges {
    let loading = matches!(diff, Some(DiffState::Loading(_)));
    TurnChanges {
        key: format!("checkpoint-{}", turn.checkpoint.0),
        checkpoint: turn.checkpoint,
        button: changes_id(turn.checkpoint),
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

fn preview(changes: CheckpointDiff) -> Preview {
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

pub fn changes_id(checkpoint: HostSeq) -> String {
    format!("checkpoint-{}-changes", checkpoint.0)
}
