//! The id of a pending issue edit, which the app mints in the issue
//! command: a retry sends the same edit again under a new command, and the
//! home applies it once.

use alloc::borrow::Cow;
use alloc::string::String;
use core::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

const DIGITS: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
const LEN: u32 = 26;

/// 128 random bits.
///
/// A human-readable format writes it as a ULID is written, Crockford's
/// base 32 in 26 lower-case digits, the first carrying the top 3 bits,
/// since JSON's numbers are exact only to 2^53; others write the number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EditId(pub u128);

impl EditId {
    /// The id `text` writes, only as `Display` writes it.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if u32::try_from(bytes.len()).ok() != Some(LEN)
            || bytes.first().is_none_or(|first| *first > b'7')
        {
            return None;
        }
        bytes
            .iter()
            .try_fold(0u128, |id, byte| {
                let digit = DIGITS.iter().position(|known| known == byte)?;
                Some((id << 5) | u128::try_from(digit).ok()?)
            })
            .map(Self)
    }
}

impl fmt::Display for EditId {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        (0..LEN).rev().try_for_each(|place| {
            let digit = (self.0 >> (place * 5)) & 31;
            let byte = usize::try_from(digit)
                .ok()
                .and_then(|at| DIGITS.get(at))
                .copied()
                .unwrap_or(b'0');
            fmt::Write::write_char(out, char::from(byte))
        })
    }
}

impl Serialize for EditId {
    fn serialize<S: Serializer>(&self, out: S) -> Result<S::Ok, S::Error> {
        if out.is_human_readable() {
            out.collect_str(self)
        } else {
            self.0.serialize(out)
        }
    }
}

impl<'de> Deserialize<'de> for EditId {
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
    type Value = EditId;

    fn expecting(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("26 lower-case Crockford base 32 digits")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<EditId, E> {
        EditId::parse(text)
            .ok_or_else(|| E::invalid_value(de::Unexpected::Str(text), &self))
    }
}

/// As JSON writes it: text.
impl JsonSchema for EditId {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("EditId")
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        Schema::from(Map::from_iter([(
            String::from("type"),
            Value::from("string"),
        )]))
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::EditId;

    #[test]
    fn an_edit_id_reads_back_from_the_text_it_writes() {
        for id in [0, 1, u128::MAX, 0x0123_4567_89ab_cdef_0123_4567_89ab_cdef] {
            assert_eq!(
                EditId::parse(&EditId(id).to_string()),
                Some(EditId(id))
            );
        }
        assert_eq!(EditId(u128::MAX).to_string(), "7zzzzzzzzzzzzzzzzzzzzzzzzz");
    }

    #[test]
    fn only_26_crockford_digits_with_a_first_below_8_are_an_edit_id() {
        for text in [
            "",
            "0000000000000000000000000",
            "80000000000000000000000000",
            "0000000000000000000000000u",
            "0000000000000000000000000A",
        ] {
            assert_eq!(EditId::parse(text), None, "{text}");
        }
    }
}
