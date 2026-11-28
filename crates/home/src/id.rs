use alloc::borrow::Cow;
use alloc::string::String;
use core::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

const DIGITS: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

const BITS_PER_DIGIT: u32 = 5;

pub const DRAWS: usize = 64;

pub type Draws = [u64; DRAWS];

mod issue;
pub use issue::{IssueId, IssueKey, IssueName, TrackerId};
#[cfg(test)]
pub use issue::{Text, id as issue_id};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectId(u64);

impl ProjectId {
    const DIGITS: u32 = 6;

    #[must_use]
    pub const fn new(random: u64) -> Self {
        Self(random & mask(Self::DIGITS * BITS_PER_DIGIT))
    }

    #[must_use]
    pub fn mint(draws: &Draws, held: &[Self]) -> Option<Self> {
        draws
            .iter()
            .map(|random| Self::new(*random))
            .find(|id| !held.contains(id))
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        read(text, Self::DIGITS).map(Self)
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write(out, self.0, Self::DIGITS)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Slug(u64);

impl Slug {
    const DIGITS: u32 = 8;

    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value & mask(Self::DIGITS * BITS_PER_DIGIT))
    }
}

impl TrackerId for Slug {
    fn read(text: &str) -> Option<Self> {
        read(text, Self::DIGITS).map(Self)
    }
}

impl fmt::Display for Slug {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write(out, self.0, Self::DIGITS)
    }
}

const fn mask(bits: u32) -> u64 {
    (1 << bits) - 1
}

fn write(out: &mut fmt::Formatter<'_>, value: u64, places: u32) -> fmt::Result {
    (0..places).rev().try_for_each(|place| {
        let digit = (value >> (place * BITS_PER_DIGIT)) & mask(BITS_PER_DIGIT);
        let byte = usize::try_from(digit)
            .ok()
            .and_then(|at| DIGITS.get(at))
            .copied()
            .unwrap_or(b'0');
        fmt::Write::write_char(out, char::from(byte))
    })
}

fn read(text: &str, places: u32) -> Option<u64> {
    let length = usize::try_from(places).ok()?;
    (text.len() == length).then_some(())?;
    text.bytes().try_fold(0, |value, byte| {
        let digit = DIGITS.iter().position(|known| *known == byte)?;
        Some((value << BITS_PER_DIGIT) | u64::try_from(digit).ok()?)
    })
}

macro_rules! textual {
    ($id:ident, $expecting:literal) => {
        impl JsonSchema for $id {
            fn schema_name() -> Cow<'static, str> {
                Cow::Borrowed(stringify!($id))
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                Schema::from(Map::from_iter([
                    (String::from("type"), Value::from("string")),
                    (String::from("minLength"), Value::from($id::DIGITS)),
                    (String::from("maxLength"), Value::from($id::DIGITS)),
                ]))
            }
        }

        impl Serialize for $id {
            fn serialize<S: Serializer>(
                &self,
                out: S,
            ) -> Result<S::Ok, S::Error> {
                out.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $id {
            fn deserialize<D: Deserializer<'de>>(
                input: D,
            ) -> Result<Self, D::Error> {
                struct Text;

                impl Visitor<'_> for Text {
                    type Value = $id;

                    fn expecting(
                        &self,
                        out: &mut fmt::Formatter<'_>,
                    ) -> fmt::Result {
                        out.write_str($expecting)
                    }

                    fn visit_str<E: de::Error>(
                        self,
                        text: &str,
                    ) -> Result<$id, E> {
                        $id::parse(text).ok_or_else(|| {
                            E::invalid_value(de::Unexpected::Str(text), &self)
                        })
                    }
                }

                input.deserialize_str(Text)
            }
        }
    };
}

textual!(ProjectId, "a project id of 6 base 32 digits");

#[cfg(test)]
mod tests;
