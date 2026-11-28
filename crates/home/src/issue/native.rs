use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeIssue {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    pub open: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueWrite {
    AddLabel(String),
    RemoveLabel(String),
    Title { from: String, to: String },
    Body { from: String, to: String },
    Close { reason: String, was: Option<String> },
    Reopen { was: Option<String> },
}

impl NativeIssue {
    #[must_use]
    pub fn revision(&self) -> String {
        fn field(hash: &mut Sha256, text: &str) {
            hash.update((text.len() as u64).to_be_bytes());
            hash.update(text.as_bytes());
        }
        let mut hash = Sha256::new();
        field(&mut hash, &self.title);
        field(&mut hash, &self.body);
        hash.update([u8::from(self.open), u8::from(self.reason.is_some())]);
        if let Some(reason) = &self.reason {
            field(&mut hash, reason);
        }
        let mut labels: Vec<_> = self.labels.iter().collect();
        labels.sort_unstable();
        for label in labels {
            field(&mut hash, label);
        }
        data_encoding::HEXLOWER.encode(&hash.finalize())
    }

    #[must_use]
    pub fn labelled(&self, name: &str) -> bool {
        self.labels.iter().any(|label| same_label(label, name))
    }
}

fn same_label(one: &str, other: &str) -> bool {
    one.to_lowercase() == other.to_lowercase()
}

impl IssueWrite {
    #[must_use]
    pub fn holds(&self, issue: &NativeIssue) -> bool {
        match self {
            Self::AddLabel(name) => issue.labelled(name),
            Self::RemoveLabel(name) => !issue.labelled(name),
            Self::Title { to, .. } => issue.title == *to,
            Self::Body { to, .. } => issue.body == *to,
            Self::Close { .. } => !issue.open,
            Self::Reopen { .. } => issue.open,
        }
    }

    #[must_use]
    pub fn undone(&self, issue: &NativeIssue) -> NativeIssue {
        let mut before = issue.clone();
        match self {
            Self::AddLabel(name) => {
                before.labels.retain(|label| !same_label(label, name));
            }
            Self::RemoveLabel(name) => before.labels.push(name.clone()),
            Self::Title { from, .. } => before.title.clone_from(from),
            Self::Body { from, .. } => before.body.clone_from(from),
            Self::Close { was, .. } => {
                before.open = true;
                before.reason.clone_from(was);
            }
            Self::Reopen { was } => {
                before.open = false;
                before.reason.clone_from(was);
            }
        }
        before
    }
}
