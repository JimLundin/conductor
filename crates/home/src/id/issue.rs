use alloc::{borrow::Cow, string::String};
use core::fmt;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::ProjectId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IssueId(pub(crate) String);

impl IssueId {
    #[must_use]
    pub fn new(text: &str) -> Option<Self> {
        (!text.is_empty()).then(|| Self(String::from(text)))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
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

impl fmt::Display for IssueKey {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{}/{}", self.project, self.id)
    }
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
        Self::new(&text).ok_or_else(|| {
            serde::de::Error::custom("an issue id is a non-empty string")
        })
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
mod tests {
    use super::IssueId;
    use alloc::string::ToString;

    #[test]
    fn an_issue_id_reads_back_the_string_it_was_built_from() {
        for text in ["0tm7aaaa", "146", "https://tracker.example/~one/å"] {
            let id = IssueId::new(text);
            assert_eq!(id.as_ref().map(IssueId::as_str), Some(text));
            assert_eq!(
                id.map(|built| built.to_string()).as_deref(),
                Some(text)
            );
        }
    }

    #[test]
    fn an_empty_string_is_no_issue_id() {
        assert_eq!(IssueId::new(""), None);
    }
}
