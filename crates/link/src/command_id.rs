//! A command's id on the client link, which the app mints so a retried
//! command answers from its receipt.

use alloc::string::String;
use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use ulid::Ulid;

/// A client-generated, fleet-unique 128-bit random id for idempotent
/// retries, whose text is a ULID's as the `ulid` crate writes it.
///
/// A human-readable format writes the text, since JSON's numbers are exact
/// only to 2^53; others write the number.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, JsonSchema,
)]
pub struct CommandId(#[schemars(with = "String")] pub u128);

impl CommandId {
    /// The id `text` writes, in either case, as the app mints it in lower
    /// case. The crate drops the bits of a first digit above 7, so text
    /// that doesn't write back is refused.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let id = Ulid::from_string(text).ok()?;
        let mut buffer = [0; ulid::ULID_LEN];
        id.array_to_str(&mut buffer)
            .eq_ignore_ascii_case(text)
            .then_some(Self(id.0))
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        Ulid(self.0).fmt(out)
    }
}

impl Serialize for CommandId {
    fn serialize<S: Serializer>(&self, out: S) -> Result<S::Ok, S::Error> {
        if out.is_human_readable() {
            out.collect_str(self)
        } else {
            self.0.serialize(out)
        }
    }
}

impl<'de> Deserialize<'de> for CommandId {
    fn deserialize<D: Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        if input.is_human_readable() {
            let text = String::deserialize(input)?;
            Self::parse(&text).ok_or_else(|| {
                de::Error::invalid_value(de::Unexpected::Str(&text), &"a ULID")
            })
        } else {
            u128::deserialize(input).map(Self)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CommandId;

    const MAX: &str = r#""7ZZZZZZZZZZZZZZZZZZZZZZZZZ""#;

    #[test]
    fn a_command_id_is_written_as_the_ulid_crate_writes_a_ulid() {
        assert_eq!(
            serde_json::to_string(&CommandId(u128::MAX)).ok().as_deref(),
            Some(MAX)
        );
    }

    #[test]
    fn a_command_id_in_lower_case_reads_as_in_upper_case() {
        assert_eq!(
            serde_json::from_str::<CommandId>(&MAX.to_ascii_lowercase()).ok(),
            Some(CommandId(u128::MAX))
        );
    }

    #[test]
    fn postcard_writes_a_command_id_as_its_number() {
        let id = 0x0123_4567_89ab_cdef_0123_4567_89ab_cdef_u128;
        assert_eq!(
            postcard::to_allocvec(&CommandId(id)).ok(),
            postcard::to_allocvec(&id).ok()
        );
    }

    #[test]
    fn only_26_crockford_digits_with_a_first_below_8_are_a_command_id() {
        for text in [
            r#""""#,
            r#""0000000000000000000000000""#,
            r#""80000000000000000000000000""#,
            r#""0000000000000000000000000U""#,
        ] {
            assert!(serde_json::from_str::<CommandId>(text).is_err(), "{text}");
        }
    }
}
