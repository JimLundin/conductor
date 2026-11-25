use alloc::{borrow::Cow, format, string::String, vec::Vec};
use core::fmt;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IssueId {
    pub(crate) authority: String,
    pub(crate) key: String,
}

impl IssueId {
    #[must_use]
    pub fn new(authority: &str, key: &str) -> Option<Self> {
        if authority.is_empty() || key.is_empty() {
            return None;
        }
        Some(Self {
            authority: String::from(authority),
            key: String::from(key),
        })
    }

    #[must_use]
    pub fn authority(&self) -> &str {
        &self.authority
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (authority, key) = text.split_once('~')?;
        let id = Self::new(&decode(authority)?, &decode(key)?)?;
        (format!("{id}") == text).then_some(id)
    }
}

impl fmt::Display for IssueId {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{}~{}", encode(&self.authority), encode(&self.key))
    }
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
                char::from(byte).into()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn decode(text: &str) -> Option<String> {
    let mut bytes = text.bytes();
    let mut decoded = Vec::new();
    while let Some(byte) = bytes.next() {
        decoded.push(if byte == b'%' {
            let high = char::from(bytes.next()?).to_digit(16)?;
            let low = char::from(bytes.next()?).to_digit(16)?;
            u8::try_from(high * 16 + low).ok()?
        } else {
            byte
        });
    }
    String::from_utf8(decoded).ok()
}

impl Serialize for IssueId {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for IssueId {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| {
            serde::de::Error::custom(
                "an issue identity needs a canonical authority and opaque key",
            )
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
            "pattern": "^(?:[A-Za-z0-9_-]|%[0-9A-F]{2})+~(?:[A-Za-z0-9_-]|%[0-9A-F]{2})+$"
        })
    }
}

#[cfg(test)]
mod tests {
    use super::IssueId;
    use alloc::string::ToString;

    #[test]
    fn qualified_identities_keep_authorities_and_opaque_keys_distinct() {
        let first = IssueId::parse("tracker-a~rest-identity-42");
        let other = IssueId::parse("tracker-b~rest-identity-42");
        assert!(first.is_some());
        assert!(other.is_some());
        assert_ne!(first, other);
        assert_eq!(IssueId::parse("42"), None);
        assert_eq!(IssueId::parse("#42"), None);
    }

    #[test]
    fn delimiters_and_unicode_in_opaque_components_have_one_spelling() {
        let identity = IssueId::new("https://tracker.example/~", "one/å~two");
        let text = "https%3A%2F%2Ftracker%2Eexample%2F%7E~one%2F%C3%A5%7Etwo";
        assert_eq!(
            identity.as_ref().map(ToString::to_string).as_deref(),
            Some(text)
        );
        assert_eq!(IssueId::parse(text), identity);
        assert_eq!(IssueId::parse("tracker~%41"), None);
        assert_eq!(IssueId::parse("tracker~%2f"), None);
    }
}
