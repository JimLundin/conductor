use alloc::borrow::Cow;
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::de::{self, DeserializeOwned, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{IssueId, ProjectId};

mod changes;
mod code;
mod diagram;
mod options;
mod progress;
mod statechart;
mod stray;

pub use changes::{
    Change, ChangeGroup, ChangeKind, Changes, ChangesError, Tag,
};
pub use code::CodeRef;
pub use diagram::{Diagram, DiagramError, Edge, Group, Node};
pub use options::{Choice, Options, OptionsError};
pub use progress::{Bar, Progress, ProgressError, Stretch};
pub use statechart::{State, Statechart, StatechartError, Transition};
pub use stray::{Broken, Held, Stray, StrayReason};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, try_from = "Drawn")]
pub struct Canvas {
    title: String,
    blocks: Vec<Block>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename = "Canvas", expecting = "struct Canvas", deny_unknown_fields)]
struct Drawn {
    title: String,
    blocks: Vec<Block>,
}

impl TryFrom<Drawn> for Canvas {
    type Error = CanvasError;

    fn try_from(drawn: Drawn) -> Result<Self, CanvasError> {
        Self::new(drawn.title, drawn.blocks)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Block {
    Prose { text: String },
    Diagram(Diagram),
    Statechart(Statechart),
    Progress(Progress),
    Options(Options),
    Changes(Changes),
    Code(CodeRef),
}

impl Canvas {
    pub fn new(title: String, blocks: Vec<Block>) -> Result<Self, CanvasError> {
        Ok(Self {
            title,
            blocks: blocks
                .into_iter()
                .map(Block::checked)
                .collect::<Result<Vec<Block>, CanvasError>>()?,
        })
    }

    pub fn parse(text: &str) -> Result<Self, CanvasError> {
        decoded::<Drawn>(text).and_then(Self::try_from)
    }

    #[must_use]
    pub fn schema() -> String {
        let mut text =
            serde_json::to_string_pretty(&schemars::schema_for!(Drawn))
                .unwrap_or_default();
        text.push('\n');
        text
    }

    pub(crate) fn markdown(&self) -> String {
        core::iter::once(format!("# {}\n", self.title.trim()))
            .chain(self.blocks.iter().map(Block::markdown))
            .collect::<Vec<String>>()
            .join("\n")
    }
}

impl Block {
    fn markdown(&self) -> String {
        match self {
            Self::Prose { text } => format!("{}\n", text.trim()),
            Self::Diagram(diagram) => diagram.mermaid(),
            Self::Statechart(chart) => chart.mermaid(),
            Self::Progress(progress) => progress.markdown(),
            Self::Options(options) => options.markdown(),
            Self::Changes(changes) => changes.markdown(),
            Self::Code(code) => format!("{}\n", code.link()),
        }
    }

    fn checked(self) -> Result<Self, CanvasError> {
        match self {
            Self::Statechart(chart) => chart.checked().map(Self::Statechart),
            Self::Prose { .. } | Self::Code(_) => Ok(self),
            Self::Diagram(ref diagram) => diagram.check().map(|()| self),
            Self::Progress(ref progress) => progress.check().map(|()| self),
            Self::Options(ref options) => options.check().map(|()| self),
            Self::Changes(ref changes) => changes.check().map(|()| self),
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
    pub fn parse(text: &str) -> Result<Self, CanvasError> {
        let file: Stamped = decoded(text)?;
        Ok(Self {
            written: file.written,
            by: file.by,
            canvas: Canvas::try_from(file.canvas)?,
        })
    }

    #[must_use]
    pub fn encode(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).unwrap_or_default();
        text.push('\n');
        text
    }
}

#[derive(Deserialize)]
#[serde(expecting = "struct CanvasFile", deny_unknown_fields)]
struct Stamped {
    written: u64,
    by: String,
    canvas: Drawn,
}

const MOST_WORDS: usize = 8;

fn distinct<'a>(
    part: &'static str,
    mut ids: impl Iterator<Item = &'a str>,
) -> Result<(), CanvasError> {
    let mut seen = BTreeSet::new();
    ids.find(|id| !seen.insert(*id)).map_or(Ok(()), |id| {
        Err(CanvasError::Twice {
            part,
            id: String::from(id),
        })
    })
}

fn short<'a>(
    mut labels: impl Iterator<Item = &'a String>,
) -> Result<(), CanvasError> {
    labels
        .find(|label| label.split_whitespace().count() > MOST_WORDS)
        .map_or(Ok(()), |label| {
            Err(CanvasError::LongLabel {
                label: String::from(label.trim()),
            })
        })
}

fn line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<&str>>().join(" ")
}

fn quoted(label: &str) -> String {
    format!("\"{}\"", line(label).replace('"', "#quot;"))
}

fn decoded<T: DeserializeOwned>(text: &str) -> Result<T, CanvasError> {
    serde_json::from_str(text).map_err(|error| CanvasError::Json {
        reason: error.to_string(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanvasError {
    Json { reason: String },
    LongLabel { label: String },
    Twice { part: &'static str, id: String },
    Diagram(DiagramError),
    Statechart(StatechartError),
    Progress(ProgressError),
    Options(OptionsError),
    Changes(ChangesError),
}

impl fmt::Display for CanvasError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json { reason } => write!(out, "not a canvas: {reason}"),
            Self::LongLabel { label } => write!(
                out,
                "the label `{label}` is over {MOST_WORDS} words: put the detail in prose"
            ),
            Self::Twice { part, id } => write!(
                out,
                "two {part}s are called `{id}`: give each {part} its own id"
            ),
            Self::Diagram(error) => error.fmt(out),
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

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        String::json_schema(generator)
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
