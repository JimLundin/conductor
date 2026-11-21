//! Issues (docs/home.md §2, §3): what needs doing in a project. The home
//! keeps each in `projects/<project>/issues/<id>.md`; Conductor is the only
//! place they are written.

use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

/// A ULID, unique across every project and node, as its 128 bits. Its file
/// and its branch name it in lower case.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
pub struct IssueId(pub u128);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub id: IssueId,
    pub title: String,
    /// What to build and why, as Markdown.
    pub body: String,
    pub status: IssueStatus,
    pub priority: Priority,
    pub labels: Vec<String>,
    /// The branch the issue's sessions land on.
    pub branch: String,
    /// When it was opened, as RFC 3339 in UTC.
    pub created: String,
    pub checklist: Vec<ChecklistItem>,
    /// Append-only, each as its author wrote it, oldest first.
    pub notes: Vec<String>,
}

/// `open ─► ready ─► in-progress ─► review ─► done`, or `dropped` from any.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum IssueStatus {
    Open,
    Ready,
    InProgress,
    Review,
    Done,
    Dropped,
}

impl IssueStatus {
    pub const ALL: [Self; 6] = [
        Self::Open,
        Self::Ready,
        Self::InProgress,
        Self::Review,
        Self::Done,
        Self::Dropped,
    ];

    /// The word an issue file and the app use.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Ready => "ready",
            Self::InProgress => "in-progress",
            Self::Review => "review",
            Self::Done => "done",
            Self::Dropped => "dropped",
        }
    }

    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.word() == word)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize,
)]
pub enum Priority {
    Low,
    Normal,
    High,
    Urgent,
}

impl Priority {
    pub const ALL: [Self; 4] =
        [Self::Low, Self::Normal, Self::High, Self::Urgent];

    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
            Self::Urgent => "urgent",
        }
    }

    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|priority| priority.word() == word)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistItem {
    pub done: bool,
    pub text: String,
}

/// What a new issue opens with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueFields {
    pub title: String,
    pub body: String,
    pub priority: Priority,
    pub labels: Vec<String>,
}

/// One change to an existing issue, as small as a person makes it, so
/// changes made on two hosts at once touch different lines of its file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueChange {
    Title(String),
    Body(String),
    /// The labels it has from now on.
    Labels(Vec<String>),
    Priority(Priority),
    Status(IssueStatus),
    /// Checks or unchecks the checklist's first item with this text.
    Check {
        item: String,
        done: bool,
    },
    AddItem(String),
    /// Removes the checklist's first item with this text.
    RemoveItem(String),
    /// A note, which the host signs and dates.
    Note(String),
    Transition {
        from: IssueStatus,
        to: IssueStatus,
    },
    Checklist {
        snapshot: Vec<ChecklistItem>,
        index: usize,
        action: ChecklistAction,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChecklistAction {
    SetDone(bool),
    Remove,
}
