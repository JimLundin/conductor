use alloc::borrow::Cow;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::canvas::CanvasEntry;
use crate::{IssueId, IssueKey, ProjectId, UnixMillis};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Issue {
    pub id: IssueId,
    pub project: ProjectId,
    pub created: Option<UnixMillis>,
    pub title: String,
    pub kind: String,
    pub status: String,
    pub stage: Option<String>,
    pub fields: BTreeMap<String, Datum>,
    pub links: BTreeMap<String, Vec<IssueKey>>,
    pub body: String,
    pub sections: Vec<Section>,
    pub checklist: Vec<ChecklistItem>,
    pub log: Vec<Entry>,
    pub notes: Vec<Note>,
    pub branch: String,
    pub canvases: Vec<CanvasEntry>,
}

impl Issue {
    #[must_use]
    pub fn text(&self, field: &str) -> Option<&str> {
        self.fields.get(field).and_then(Datum::text)
    }

    #[must_use]
    pub fn texts(&self, field: &str) -> &[String] {
        match self.fields.get(field) {
            Some(Datum::Texts(texts)) => texts,
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Section {
    pub heading: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Datum {
    Text(String),
    Texts(Vec<String>),
    Bool(bool),
}

impl Datum {
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Texts(_) | Self::Bool(_) => None,
        }
    }
}

#[derive(Serialize)]
#[serde(rename = "Datum")]
enum TaggedRef<'a> {
    Text(&'a str),
    Texts(&'a [String]),
    Bool(bool),
}

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

impl JsonSchema for Datum {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("Datum")
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let kinds = [
            generator.subschema_for::<String>(),
            generator.subschema_for::<Vec<String>>(),
            generator.subschema_for::<bool>(),
        ];
        Schema::from(Map::from_iter([(
            String::from("anyOf"),
            Value::Array(kinds.into_iter().map(Schema::to_value).collect()),
        )]))
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Entry {
    pub at: Option<String>,
    pub by: Option<String>,
    pub event: LogEvent,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum LogEvent {
    Note,
    Stage { from: String, to: String },
    Status { from: String, to: String },
}

impl fmt::Display for Entry {
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
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Note => out.write_str("note"),
            Self::Stage { from, to } => write!(out, "stage {from}→{to}"),
            Self::Status { from, to } => write!(out, "status {from}→{to}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChecklistItem {
    pub done: bool,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NewIssue {
    pub project: ProjectId,
    pub kind: String,
    pub title: String,
    pub fields: Vec<(String, Datum)>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum Part {
    Stage,
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum IssueChange {
    Move {
        part: Part,
        to: String,
        reason: String,
        signed_off: bool,
    },
    Set {
        field: String,
        value: Datum,
    },
    Title(String),
    Note(String),
    Body(String),
    Checklist(ChecklistChange),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ChecklistChange {
    Add(String),
    Check { item: Item, done: bool },
    Remove(Item),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Item {
    pub index: usize,
    pub seen: Option<Vec<ChecklistItem>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Note {
    pub id: String,
    pub author: String,
    pub created: UnixMillis,
    pub text: String,
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
