//! Issues (docs/home.md §2, §3): what needs doing in a project. The home
//! keeps each in `projects/<project>/issues/<id>.md`, read as its flow
//! (`crate::flow`) declares it: the issue names its kind, status and stage,
//! and its fields and relations are the ones the flow declares.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
pub struct IssueId(#[serde(with = "crate::json::ulid")] pub u128);

impl fmt::Display for IssueId {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::json::ulid::write(self.0, out)
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
    /// The issues each relation from it points to, by relation.
    pub links: BTreeMap<String, Vec<IssueId>>,
    /// The body before the checklist and the log, as its file has it.
    pub body: String,
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
    /// The value of a field it gives as one word or text.
    #[must_use]
    pub fn text(&self, field: &str) -> Option<&str> {
        self.fields.get(field).and_then(Datum::text)
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
///
/// A human-readable format, as TOML is, writes it as its value: text, a
/// list of text or a boolean, and reads a time as its text. The wire's
/// format is not self-describing, so there it is tagged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Datum {
    Text(String),
    Texts(Vec<String>),
    Bool(bool),
}

impl Datum {
    /// The value as one word or text.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Texts(_) | Self::Bool(_) => None,
        }
    }
}

/// [`Datum`] as the wire writes it.
#[derive(Serialize)]
#[serde(rename = "Datum")]
enum TaggedRef<'a> {
    Text(&'a str),
    Texts(&'a [String]),
    Bool(bool),
}

/// [`Datum`] as the wire reads it.
#[derive(Deserialize)]
#[serde(rename = "Datum")]
enum Tagged {
    Text(String),
    Texts(Vec<String>),
    Bool(bool),
}

impl Serialize for Datum {
    fn serialize<S: Serializer>(&self, out: S) -> Result<S::Ok, S::Error> {
        if out.is_human_readable() {
            match self {
                Self::Text(text) => out.serialize_str(text),
                Self::Texts(texts) => texts.serialize(out),
                Self::Bool(flag) => out.serialize_bool(*flag),
            }
        } else {
            match self {
                Self::Text(text) => TaggedRef::Text(text),
                Self::Texts(texts) => TaggedRef::Texts(texts),
                Self::Bool(flag) => TaggedRef::Bool(*flag),
            }
            .serialize(out)
        }
    }
}

impl<'de> Deserialize<'de> for Datum {
    fn deserialize<D: Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        if input.is_human_readable() {
            input.deserialize_any(Plain)
        } else {
            Tagged::deserialize(input).map(|tagged| match tagged {
                Tagged::Text(text) => Self::Text(text),
                Tagged::Texts(texts) => Self::Texts(texts),
                Tagged::Bool(flag) => Self::Bool(flag),
            })
        }
    }
}

/// A [`Datum`] written as its value.
struct Plain;

impl<'de> Visitor<'de> for Plain {
    type Value = Datum;

    fn expecting(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("text, a list of text, true or false, or a time")
    }

    fn visit_bool<E: de::Error>(self, flag: bool) -> Result<Datum, E> {
        Ok(Datum::Bool(flag))
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Datum, E> {
        Ok(Datum::Text(String::from(text)))
    }

    fn visit_string<E: de::Error>(self, text: String) -> Result<Datum, E> {
        Ok(Datum::Text(text))
    }

    fn visit_seq<A: SeqAccess<'de>>(
        self,
        mut items: A,
    ) -> Result<Datum, A::Error> {
        let mut texts = Vec::new();
        while let Some(text) = items.next_element::<String>()? {
            texts.push(text);
        }
        Ok(Datum::Texts(texts))
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogEvent {
    Note,
    Stage { from: String, to: String },
    Status { from: String, to: String },
}

impl fmt::Display for Entry {
    /// `<time> <who> <event>: <text>`, or the text alone when it names no
    /// time and no one.
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.at, &self.by) {
            (Some(at), Some(by)) => {
                write!(out, "{at} {by} {}: {}", self.event, self.text)
            }
            _ => out.write_str(&self.text),
        }
    }
}

impl Entry {
    /// The entry `line` writes, as `Display` writes it: any line that
    /// isn't `<time> <who> <event>: <text>` is a note as written.
    #[must_use]
    pub fn of(line: &str) -> Self {
        Self::structured(line).unwrap_or_else(|| Self {
            at: None,
            by: None,
            event: LogEvent::Note,
            text: String::from(line),
        })
    }

    fn structured(line: &str) -> Option<Self> {
        let (head, text) = line.split_once(": ")?;
        let mut words = head.split(' ');
        let at = words.next()?;
        let by = words.next()?;
        let event = match (words.next()?, words.next(), words.next()) {
            ("note", None, _) => LogEvent::Note,
            (name @ ("stage" | "status"), Some(change), None) => {
                let (before, after) = change.split_once('→')?;
                let (from, to) = (String::from(before), String::from(after));
                if name == "stage" {
                    LogEvent::Stage { from, to }
                } else {
                    LogEvent::Status { from, to }
                }
            }
            _ => return None,
        };
        at.contains('T').then(|| Self {
            at: Some(String::from(at)),
            by: Some(String::from(by)),
            event,
            text: String::from(text),
        })
    }
}

impl fmt::Display for LogEvent {
    /// `note`, `stage <from>→<to>` or `status <from>→<to>`.
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Note => out.write_str("note"),
            Self::Stage { from, to } => write!(out, "stage {from}→{to}"),
            Self::Status { from, to } => write!(out, "status {from}→{to}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistItem {
    pub done: bool,
    pub text: String,
}

/// A new issue, as `home issue new` opens one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewIssue {
    pub project: ProjectId,
    pub kind: String,
    pub title: String,
    /// The fields and relations it starts with, each with its value, in
    /// the order given.
    pub fields: Vec<(String, Datum)>,
}

/// What a move changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Part {
    Stage,
    Status,
}

/// One change to an issue, as the `home` command line and the app make
/// it. The home reads the file it writes back against the flow before
/// keeping it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueChange {
    Move {
        part: Part,
        to: String,
        reason: String,
    },
    /// A declared field's or relation's value.
    Set {
        field: String,
        value: Datum,
    },
    Title(String),
    /// A note, which the writer signs and dates.
    Note(String),
    Body(String),
    Checklist(ChecklistChange),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChecklistChange {
    Add(String),
    Check { item: Item, done: bool },
    Remove(Item),
}

/// A checklist item by its place, from 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub index: usize,
    /// The checklist the place was picked from; when the file's has
    /// changed since, the change is refused. `None` takes the place in
    /// the file as it is.
    pub seen: Option<Vec<ChecklistItem>>,
}

#[cfg(test)]
mod tests {
    use alloc::string::{String, ToString};

    use alloc::vec::Vec;

    use super::{Datum, Entry, LogEvent};

    #[test]
    fn a_datum_crosses_the_wire_tagged_and_reads_back_as_written() {
        for datum in [
            Datum::Text(String::from("high")),
            Datum::Texts(Vec::from([String::from("app")])),
            Datum::Texts(Vec::new()),
            Datum::Bool(true),
        ] {
            let bytes = postcard::to_allocvec(&datum);
            assert_eq!(
                bytes.and_then(|written| postcard::from_bytes::<Datum>(
                    &written
                )),
                Ok(datum)
            );
        }
    }

    fn entry(event: LogEvent, text: &str) -> Entry {
        Entry {
            at: Some(String::from("2026-11-22T17:26:37Z")),
            by: Some(String::from("laptop·claude")),
            event,
            text: String::from(text),
        }
    }

    #[test]
    fn a_log_line_reads_back_as_the_entry_that_wrote_it() {
        let entries = [
            entry(LogEvent::Note, "opened."),
            entry(
                LogEvent::Status {
                    from: String::from("open"),
                    to: String::from("done"),
                },
                "landed: and merged",
            ),
            entry(
                LogEvent::Stage {
                    from: String::from("none"),
                    to: String::from("build"),
                },
                "",
            ),
        ];
        for written in entries {
            assert_eq!(Entry::of(&written.to_string()), written);
        }
    }

    #[test]
    fn a_line_that_names_no_time_and_no_one_is_a_note_as_written() {
        for line in ["just words", "someone note: no time", "a b c d: e"] {
            assert_eq!(
                Entry::of(line),
                Entry {
                    at: None,
                    by: None,
                    event: LogEvent::Note,
                    text: String::from(line),
                }
            );
        }
    }
}
