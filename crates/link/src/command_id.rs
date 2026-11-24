//! A command's id on the client link, which the app mints so a retried
//! command answers from its receipt.

use alloc::string::String;
use core::fmt;

use schemars::JsonSchema;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A client-generated, fleet-unique 128-bit random id for idempotent retries.
///
/// A human-readable format writes it as a ULID is written, Crockford's
/// base 32 in 26 lower-case digits, since JSON's numbers are exact only to
/// 2^53; others write the number. The `ulid` crate reads and writes the
/// digits.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, JsonSchema,
)]
pub struct CommandId(#[schemars(with = "String")] pub u128);

impl CommandId {
    /// The id `text` writes, only as `Display` writes it.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let id = ulid::Ulid::from_string(text).ok()?.0;
        let mut buffer = [0; ulid::ULID_LEN];
        let written = ulid::Ulid(id).array_to_str(&mut buffer);
        written.make_ascii_lowercase();
        (*written == *text).then_some(Self(id))
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buffer = [0; ulid::ULID_LEN];
        let text = ulid::Ulid(self.0).array_to_str(&mut buffer);
        text.make_ascii_lowercase();
        out.write_str(text)
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
            input.deserialize_str(Text)
        } else {
            u128::deserialize(input).map(Self)
        }
    }
}

struct Text;

impl Visitor<'_> for Text {
    type Value = CommandId;

    fn expecting(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("26 lower-case Crockford base 32 digits")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<CommandId, E> {
        CommandId::parse(text)
            .ok_or_else(|| E::invalid_value(de::Unexpected::Str(text), &self))
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::CommandId;

    #[test]
    fn a_command_id_reads_back_from_the_text_it_writes() {
        for id in [0, 1, u128::MAX, 0x0123_4567_89ab_cdef_0123_4567_89ab_cdef] {
            assert_eq!(
                CommandId::parse(&CommandId(id).to_string()),
                Some(CommandId(id))
            );
        }
        assert_eq!(
            CommandId(u128::MAX).to_string(),
            "7zzzzzzzzzzzzzzzzzzzzzzzzz"
        );
    }

    #[test]
    fn only_26_lower_case_crockford_digits_below_2_128_are_a_command_id() {
        for text in [
            "",
            "0000000000000000000000000",
            "80000000000000000000000000",
            "0000000000000000000000000u",
            "0000000000000000000000000A",
        ] {
            assert_eq!(CommandId::parse(text), None, "{text}");
        }
    }
}
