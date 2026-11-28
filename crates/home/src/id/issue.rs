use alloc::{
    borrow::Cow,
    string::{String, ToString},
};
use core::fmt;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::ProjectId;

pub trait TrackerId: fmt::Display + Sized {
    fn read(text: &str) -> Option<Self>;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IssueId(String);

impl IssueId {
    #[must_use]
    pub fn of<T: TrackerId>(id: &T) -> Self {
        Self(id.to_string())
    }

    #[must_use]
    pub fn tracked<T: TrackerId>(&self) -> Option<T> {
        T::read(&self.0)
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
pub struct IssueKey {
    pub project: ProjectId,
    pub id: IssueId,
}

impl IssueKey {
    #[must_use]
    pub fn said_from(&self, from: ProjectId) -> String {
        if self.project == from {
            self.id.to_string()
        } else {
            self.to_string()
        }
    }
}

impl fmt::Display for IssueKey {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{} in {}", self.id, self.project)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IssueName {
    pub project: ProjectId,
    pub id: String,
}

impl fmt::Display for IssueId {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(&self.0)
    }
}

impl Serialize for IssueId {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for IssueId {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        if text.is_empty() {
            return Err(serde::de::Error::custom(
                "a kept issue id is a non-empty string",
            ));
        }
        Ok(Self(text))
    }
}

impl JsonSchema for IssueId {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("IssueId")
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": "string",
            "minLength": 1
        })
    }
}

#[cfg(test)]
pub struct Text(String);

#[cfg(test)]
impl fmt::Display for Text {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(&self.0)
    }
}

#[cfg(test)]
impl TrackerId for Text {
    fn read(text: &str) -> Option<Self> {
        (!text.is_empty()).then(|| Self(String::from(text)))
    }
}

#[cfg(test)]
pub fn id(text: &str) -> IssueId {
    IssueId(String::from(text))
}
