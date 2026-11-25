use alloc::borrow::Cow;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::de::{self, DeserializeOwned, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::{IssueId, ProjectId};

mod changes;
mod code;
mod options;
mod progress;
mod statechart;

pub use changes::{Change, ChangeGroup, ChangeKind, ChangesError, Tag};
pub use code::CodeRef;
pub use options::{Choice, OptionsError};
pub use progress::{Bar, ProgressError, Stretch};
pub use statechart::{State, Statechart, StatechartError, Transition};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Canvas {
    pub title: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Block {
    Prose {
        text: String,
    },
    Statechart(Statechart),
    Progress {
        label: String,
        bar: Bar,
    },
    Options {
        question: String,
        options: Vec<Choice>,
        pick: usize,
        why: String,
    },
    Changes {
        groups: Vec<ChangeGroup>,
    },
    Code(CodeRef),
}

impl Canvas {
    /// # Errors
    ///
    /// The text is not a canvas.
    pub fn parse(text: &str) -> Result<Self, CanvasError> {
        let canvas: Self = decoded(text)?;
        canvas.blocks.iter().try_for_each(Block::check)?;
        Ok(canvas)
    }

    #[must_use]
    pub fn markdown(&self) -> String {
        core::iter::once(format!("# {}\n", self.title.trim()))
            .chain(self.blocks.iter().map(Block::markdown))
            .collect::<Vec<String>>()
            .join("\n")
    }
}

impl Block {
    #[must_use]
    pub fn markdown(&self) -> String {
        match self {
            Self::Prose { text } => format!("{}\n", text.trim()),
            Self::Statechart(chart) => chart.mermaid(),
            Self::Progress { label, bar } => progress::markdown(label, bar),
            Self::Options {
                question,
                options,
                pick,
                why,
            } => options::markdown(question, options, *pick, why),
            Self::Changes { groups } => changes::markdown(groups),
            Self::Code(code) => format!("{}\n", code.link()),
        }
    }

    fn check(&self) -> Result<(), CanvasError> {
        match self {
            Self::Prose { .. } | Self::Code(_) => Ok(()),
            Self::Statechart(chart) => {
                chart.check().map_err(CanvasError::Statechart)
            }
            Self::Progress { label, bar } => {
                progress::check(label, bar).map_err(CanvasError::Progress)
            }
            Self::Options {
                question,
                options,
                pick,
                ..
            } => options::check(question, options, *pick)
                .map_err(CanvasError::Options),
            Self::Changes { groups } => {
                changes::check(groups).map_err(CanvasError::Changes)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanvasFile {
    pub written: u64,
    pub by: String,
    pub canvas: Canvas,
}

impl CanvasFile {
    /// # Errors
    ///
    /// The text is not a canvas file.
    pub fn parse(text: &str) -> Result<Self, CanvasError> {
        decoded(text)
    }

    #[must_use]
    pub fn encode(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }
}

fn decoded<T: DeserializeOwned>(text: &str) -> Result<T, CanvasError> {
    serde_json::from_str(text).map_err(|error| CanvasError::Json {
        reason: error.to_string(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanvasError {
    Json { reason: String },
    Statechart(StatechartError),
    Progress(ProgressError),
    Options(OptionsError),
    Changes(ChangesError),
}

impl fmt::Display for CanvasError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json { reason } => write!(out, "not a canvas: {reason}"),
            Self::Statechart(error) => error.fmt(out),
            Self::Progress(error) => error.fmt(out),
            Self::Options(error) => error.fmt(out),
            Self::Changes(error) => error.fmt(out),
        }
    }
}

impl core::error::Error for CanvasError {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanvasName(String);

impl CanvasName {
    /// # Errors
    ///
    /// The text is not lowercase words joined by `-`.
    pub fn parse(text: &str) -> Result<Self, BadName> {
        text.split('-')
            .all(|word| {
                !word.is_empty()
                    && word.bytes().all(|byte| {
                        byte.is_ascii_lowercase() || byte.is_ascii_digit()
                    })
            })
            .then(|| Self(String::from(text)))
            .ok_or_else(|| BadName {
                name: String::from(text),
            })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CanvasName {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BadName {
    pub name: String,
}

impl fmt::Display for BadName {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            out,
            "`{}` isn't a canvas name: name it with lowercase words joined by `-`, such as `design-tree`",
            self.name
        )
    }
}

impl core::error::Error for BadName {}

impl JsonSchema for CanvasName {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("CanvasName")
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        Schema::from(Map::from_iter([
            (String::from("type"), Value::from("string")),
            (
                String::from("pattern"),
                Value::from("^[a-z0-9]+(-[a-z0-9]+)*$"),
            ),
        ]))
    }
}

impl Serialize for CanvasName {
    fn serialize<S: Serializer>(&self, out: S) -> Result<S::Ok, S::Error> {
        out.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CanvasName {
    fn deserialize<D: Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        input.deserialize_str(Name)
    }
}

struct Name;

impl Visitor<'_> for Name {
    type Value = CanvasName;

    fn expecting(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("lowercase words joined by `-`")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<CanvasName, E> {
        CanvasName::parse(text).map_err(E::custom)
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
)]
pub struct CanvasKey {
    pub project: ProjectId,
    pub issue: IssueId,
    pub name: CanvasName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CanvasEntry {
    pub name: CanvasName,
    pub written: u64,
    pub by: String,
}

impl CanvasEntry {
    #[must_use]
    pub fn of(name: CanvasName, file: &CanvasFile) -> Self {
        Self {
            name,
            written: file.written,
            by: file.by.clone(),
        }
    }
}

#[cfg(test)]
mod tests;
