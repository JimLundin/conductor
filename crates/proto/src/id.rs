//! Issue and project ids (docs/home.md §3), minted, read and written here
//! alone: lower-case base 32 digits, the most significant first, so text
//! order is number order. A shell gives the clock and the randomness.

use alloc::borrow::Cow;
use alloc::string::String;
use core::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

const DIGITS: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// The bits one digit carries.
const BITS: u32 = 5;

/// How many draws minting one id takes, each tried in turn while it gives
/// an id the home already has.
pub const DRAWS: usize = 64;

/// The randomness a shell gives to mint one id, of which each id keeps
/// the bits it has room for.
pub type Draws = [u64; DRAWS];

/// An issue's id: 8 digits, the first 6 the seconds since [`Self::EPOCH`]
/// it was minted at, the last 2 random.
///
/// Ids sort by when they were minted, and two minted in the same second
/// differ only by chance, so minting draws again for an id the home
/// already has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IssueId(u64);

impl IssueId {
    /// 2026-01-01T00:00:00Z in seconds since the Unix epoch.
    pub const EPOCH: u64 = 1_767_225_600;

    const DIGITS: u32 = 8;
    const RANDOM_BITS: u32 = 2 * BITS;
    const TIME_BITS: u32 = (Self::DIGITS - 2) * BITS;

    /// The id minted `seconds` after [`Self::EPOCH`] with the draw
    /// `random`, of which it keeps the bits it has room for.
    #[must_use]
    pub const fn new(seconds: u64, random: u64) -> Self {
        Self(
            ((seconds & mask(Self::TIME_BITS)) << Self::RANDOM_BITS)
                | (random & mask(Self::RANDOM_BITS)),
        )
    }

    /// The first id not `held` that is minted at `now`, in seconds since
    /// the Unix epoch, with each of `draws` in turn.
    ///
    /// None when `now` is outside the years an id can count, or every draw
    /// gives a held id.
    #[must_use]
    pub fn mint(now: u64, draws: &Draws, held: &[Self]) -> Option<Self> {
        let seconds = now
            .checked_sub(Self::EPOCH)
            .filter(|seconds| *seconds <= mask(Self::TIME_BITS))?;
        draws
            .iter()
            .map(|random| Self::new(seconds, *random))
            .find(|id| !held.contains(id))
    }

    /// The id `text` writes, if it is one.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        read(text, Self::DIGITS).map(Self)
    }
}

impl fmt::Display for IssueId {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write(out, self.0, Self::DIGITS)
    }
}

/// A project's id, its folder's name in the home's `projects/`: 6 random
/// digits. Its name and title may change; its id never does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectId(u64);

impl ProjectId {
    const DIGITS: u32 = 6;

    /// The id the draw `random` gives, of which it keeps the bits it has
    /// room for.
    #[must_use]
    pub const fn new(random: u64) -> Self {
        Self(random & mask(Self::DIGITS * BITS))
    }

    /// The first id of `draws` not `held`, if any.
    #[must_use]
    pub fn mint(draws: &Draws, held: &[Self]) -> Option<Self> {
        draws
            .iter()
            .map(|random| Self::new(*random))
            .find(|id| !held.contains(id))
    }

    /// The id `text` writes, if it is one.
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

const fn mask(bits: u32) -> u64 {
    (1 << bits) - 1
}

/// `value`'s last `places` digits.
fn write(out: &mut fmt::Formatter<'_>, value: u64, places: u32) -> fmt::Result {
    (0..places).rev().try_for_each(|place| {
        let digit = (value >> (place * BITS)) & mask(BITS);
        let byte = usize::try_from(digit)
            .ok()
            .and_then(|at| DIGITS.get(at))
            .copied()
            .unwrap_or(b'0');
        fmt::Write::write_char(out, char::from(byte))
    })
}

/// The value of `text`, if it is exactly `places` digits.
fn read(text: &str, places: u32) -> Option<u64> {
    let length = usize::try_from(places).ok()?;
    (text.len() == length).then_some(())?;
    text.bytes().try_fold(0, |value, byte| {
        let digit = DIGITS.iter().position(|known| *known == byte)?;
        Some((value << BITS) | u64::try_from(digit).ok()?)
    })
}

/// Serde writes an id as its text, in every format, and its schema is that
/// text's.
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

textual!(IssueId, "an issue id of 8 base 32 digits");
textual!(ProjectId, "a project id of 6 base 32 digits");

#[cfg(test)]
mod tests;
