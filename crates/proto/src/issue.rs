//! Issues (docs/home.md §2, §3): what needs doing in a project. The home
//! keeps each in `projects/<project>/issues/<id>.md`, read as its flow
//! (`crate::flow`) declares it: the issue names its kind, status and stage,
//! and its fields and relations are the ones the flow declares.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use serde::{Deserialize, Serialize};

use crate::ProjectId;

/// A ULID, unique across every project and node, as its 128 bits. Its file
/// and its branch name it in lower case, as `Display` writes it.
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

impl fmt::Display for IssueId {
    /// Crockford's base 32 in 26 digits, the first carrying the top 3 bits.
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        const DIGITS: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
        (0..26u32).rev().try_for_each(|place| {
            let digit = (self.0 >> (place * 5)) & 31;
            let byte = usize::try_from(digit)
                .ok()
                .and_then(|at| DIGITS.get(at))
                .copied()
                .unwrap_or(b'0');
            fmt::Write::write_char(out, char::from(byte))
        })
    }
}

/// An issue as the flow reads its file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub id: IssueId,
    pub project: ProjectId,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub stage: Option<String>,
    /// The declared fields it gives, by name.
    pub fields: BTreeMap<String, Datum>,
    /// The ids of the issues each relation from it points to, by relation.
    pub links: BTreeMap<String, Vec<String>>,
    /// The body's sections in order, the text before the first heading
    /// as one with no heading; the checklist and the log aren't among them.
    pub sections: Vec<Section>,
    /// The items of the `## Checklist` section.
    pub checklist: Vec<ChecklistItem>,
    /// The `## Notes` section, oldest first.
    pub log: Vec<Entry>,
    /// The branch its sessions land on.
    pub branch: String,
}

impl Issue {
    /// The body as Markdown: its sections, each under its heading.
    #[must_use]
    pub fn body(&self) -> String {
        self.sections
            .iter()
            .map(|section| {
                if section.heading.is_empty() {
                    section.text.clone()
                } else if section.text.is_empty() {
                    format!("## {}", section.heading)
                } else {
                    format!("## {}\n\n{}", section.heading, section.text)
                }
            })
            .collect::<Vec<String>>()
            .join("\n\n")
    }

    /// The value of a field it gives as one word or text.
    #[must_use]
    pub fn text(&self, field: &str) -> Option<&str> {
        match self.fields.get(field) {
            Some(Datum::Text(text)) => Some(text),
            _ => None,
        }
    }

    /// The values of a field it gives as a list.
    #[must_use]
    pub fn texts(&self, field: &str) -> &[String] {
        match self.fields.get(field) {
            Some(Datum::Texts(texts)) => texts,
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    /// Empty for the text before the first heading.
    pub heading: String,
    pub text: String,
}

/// A field's value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Datum {
    Text(String),
    Texts(Vec<String>),
    Bool(bool),
}

/// A line of an issue's log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// When and by whom, when the line says so.
    pub at: Option<String>,
    pub by: Option<String>,
    pub event: LogEvent,
    pub text: String,
}

impl Entry {
    /// The entry as its line has it, without the list marker:
    /// `<time> <who> <event>: <text>`, or the text alone.
    #[must_use]
    pub fn line(&self) -> String {
        match (&self.at, &self.by) {
            (Some(at), Some(by)) => {
                let event = match &self.event {
                    LogEvent::Note => String::from("note"),
                    LogEvent::Stage { from, to } => {
                        format!("stage {from}→{to}")
                    }
                    LogEvent::Status { from, to } => {
                        format!("status {from}→{to}")
                    }
                };
                format!("{at} {by} {event}: {}", self.text)
            }
            _ => self.text.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogEvent {
    Note,
    Stage { from: String, to: String },
    Status { from: String, to: String },
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
    pub labels: Vec<String>,
}

/// One change to an existing issue, as small as a person makes it, so
/// changes made on two hosts at once touch different lines of its file.
/// A status, and a field's value, is one of the flow's words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueChange {
    Title(String),
    Body(String),
    /// The labels it has from now on.
    Labels(Vec<String>),
    /// Gives a field one of the words the flow declares for it.
    Set {
        field: String,
        value: String,
    },
    Status(String),
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
        from: String,
        to: String,
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
