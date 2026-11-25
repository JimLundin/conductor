//! Canvases (ADR 0005): an agent's explanation of an issue's work, as
//! typed blocks.
//!
//! This module reads, checks and writes them and knows nothing of where
//! they are kept (ADR 0006): a store keeps each [`CanvasFile`] under its
//! [`CanvasKey`].

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

/// An explanation of an issue's work, as its writer gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Canvas {
    pub title: String,
    pub blocks: Vec<Block>,
}

/// One part of a canvas, written in JSON as an object of one field named
/// by its kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Block {
    /// Short markdown.
    Prose { text: String },
}

impl Canvas {
    /// The canvas `text` writes in JSON.
    ///
    /// # Errors
    ///
    /// The text isn't a canvas's JSON.
    pub fn parse(text: &str) -> Result<Self, CanvasError> {
        decoded(text)
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
        }
    }
}

/// A canvas as it is kept: stamped with when it was written and by whom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanvasFile {
    /// Milliseconds since the Unix epoch.
    pub written: u64,
    /// As the log signs it, `<node>·<harness>`.
    pub by: String,
    pub canvas: Canvas,
}

impl CanvasFile {
    /// The file `text` writes in JSON, as [`Self::encode`] writes it.
    ///
    /// # Errors
    ///
    /// The text isn't a canvas file's JSON.
    pub fn parse(text: &str) -> Result<Self, CanvasError> {
        decoded(text)
    }

    /// The file's JSON, which [`Self::parse`] reads.
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

/// Why a text isn't a canvas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanvasError {
    /// It isn't the JSON of one, as the JSON reader says.
    Json { reason: String },
}

impl fmt::Display for CanvasError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json { reason } => write!(out, "not a canvas: {reason}"),
        }
    }
}

impl core::error::Error for CanvasError {}

/// A canvas's name, unique within its issue: lowercase words of letters
/// and digits joined by `-`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanvasName(String);

impl CanvasName {
    /// The name `text` is.
    ///
    /// # Errors
    ///
    /// It isn't lowercase words joined by `-`.
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

/// A name that isn't a canvas name.
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

/// Which canvas: its issue's project and id, and its name.
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

/// What an issue's list of canvases says of one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CanvasEntry {
    pub name: CanvasName,
    /// Milliseconds since the Unix epoch.
    pub written: u64,
    pub by: String,
}

impl CanvasEntry {
    /// The entry of `file`, kept as `name`.
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
