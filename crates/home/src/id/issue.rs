use alloc::{
    borrow::Cow,
    string::{String, ToString},
};
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

impl IssueKey {
    #[must_use]
    pub fn named(text: &str, from: ProjectId) -> Option<Self> {
        if let Some((prefix, rest)) = text.split_once('/')
            && let Some(project) = ProjectId::parse(prefix)
        {
            return Some(Self {
                project,
                id: IssueId::new(rest)?,
            });
        }
        Some(Self {
            project: from,
            id: IssueId::new(text)?,
        })
    }

    #[must_use]
    pub fn named_from(&self, from: ProjectId) -> String {
        if self.project == from {
            self.id.to_string()
        } else {
            self.to_string()
        }
    }
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
    use super::{IssueId, IssueKey, ProjectId};
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
    fn a_bare_id_names_an_issue_of_its_own_project() {
        let here = ProjectId::new(1);
        let named = IssueKey::named("0trfecpc", here);
        assert_eq!(
            named.as_ref().map(|key| (key.project, key.id.as_str())),
            Some((here, "0trfecpc"))
        );
        assert_eq!(
            named.map(|key| key.named_from(here)).as_deref(),
            Some("0trfecpc")
        );
    }

    #[test]
    fn a_project_and_an_id_name_an_issue_of_another_project() {
        let here = ProjectId::new(1);
        let there = ProjectId::parse("hyn162");
        let named = IssueKey::named("hyn162/0trfecpc", here);
        assert_eq!(
            named
                .as_ref()
                .map(|key| (Some(key.project), key.id.as_str())),
            Some((there, "0trfecpc"))
        );
        assert_eq!(
            named.map(|key| key.named_from(here)).as_deref(),
            Some("hyn162/0trfecpc")
        );
        assert_eq!(IssueKey::named("hyn162/", here), None);
    }

    #[test]
    fn an_empty_string_is_no_issue_id() {
        assert_eq!(IssueId::new(""), None);
    }
}
