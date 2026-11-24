//! Where a type's JSON differs from its postcard. JavaScript's numbers are
//! exact only to 2^53 and JSON keys are only strings, so a 128-bit id is
//! its text and an [`crate::log::Epoch`] its decimal digits, each only where
//! [`Serializer::is_human_readable`], and a map keyed by a struct is a list
//! of pairs. postcard's bytes stay as they were.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A 128-bit id another crate owns, written as the host writes its own:
/// in JSON its ULID text, in postcard its bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ulid(pub u128);

impl Serialize for Ulid {
    fn serialize<S: Serializer>(&self, out: S) -> Result<S::Ok, S::Error> {
        ulid::serialize(&self.0, out)
    }
}

impl<'de> Deserialize<'de> for Ulid {
    fn deserialize<D: Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        ulid::deserialize(input).map(Self)
    }
}

/// A 128-bit id as a ULID is written: Crockford's base 32 in 26 lower-case
/// digits, the first carrying the top 3 bits.
pub mod ulid {
    use super::{
        Deserializer, Serialize, Serializer, String, Visitor, de, fmt,
    };

    const DIGITS: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
    const LEN: u32 = 26;

    pub fn write(id: u128, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        (0..LEN).rev().try_for_each(|place| {
            let digit = (id >> (place * 5)) & 31;
            let byte = usize::try_from(digit)
                .ok()
                .and_then(|at| DIGITS.get(at))
                .copied()
                .unwrap_or(b'0');
            fmt::Write::write_char(out, char::from(byte))
        })
    }

    /// The id `text` writes, only as [`write`] writes it.
    pub fn parse(text: &str) -> Option<u128> {
        let bytes = text.as_bytes();
        if u32::try_from(bytes.len()).ok() != Some(LEN)
            || bytes.first().is_none_or(|first| *first > b'7')
        {
            return None;
        }
        bytes.iter().try_fold(0u128, |id, byte| {
            let digit = DIGITS.iter().position(|known| known == byte)?;
            Some((id << 5) | u128::try_from(digit).ok()?)
        })
    }

    struct Text(u128);

    impl fmt::Display for Text {
        fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
            write(self.0, out)
        }
    }

    pub fn serialize<S: Serializer>(
        id: &u128,
        out: S,
    ) -> Result<S::Ok, S::Error> {
        if out.is_human_readable() {
            out.collect_str(&Text(*id))
        } else {
            id.serialize(out)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        input: D,
    ) -> Result<u128, D::Error> {
        if input.is_human_readable() {
            input.deserialize_str(Ulid)
        } else {
            <u128 as serde::Deserialize>::deserialize(input)
        }
    }

    struct Ulid;

    impl Visitor<'_> for Ulid {
        type Value = u128;

        fn expecting(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
            out.write_str("26 lower-case Crockford base 32 digits")
        }

        fn visit_str<E: de::Error>(self, text: &str) -> Result<u128, E> {
            parse(text).ok_or_else(|| {
                E::invalid_value(de::Unexpected::Str(text), &self)
            })
        }

        fn visit_string<E: de::Error>(self, text: String) -> Result<u128, E> {
            self.visit_str(&text)
        }
    }
}

/// A `u64` as its decimal digits.
pub mod decimal {
    use super::{
        Deserializer, Serialize, Serializer, String, Visitor, de, fmt,
    };

    pub fn serialize<S, T>(value: &T, out: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        T: fmt::Display + Serialize,
    {
        if out.is_human_readable() {
            out.collect_str(value)
        } else {
            value.serialize(out)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        input: D,
    ) -> Result<u64, D::Error> {
        if input.is_human_readable() {
            input.deserialize_str(Decimal)
        } else {
            <u64 as serde::Deserialize>::deserialize(input)
        }
    }

    struct Decimal;

    impl Visitor<'_> for Decimal {
        type Value = u64;

        fn expecting(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
            out.write_str("a u64 as decimal digits")
        }

        fn visit_str<E: de::Error>(self, text: &str) -> Result<u64, E> {
            text.bytes()
                .all(|byte| byte.is_ascii_digit())
                .then(|| text.parse().ok())
                .flatten()
                .ok_or_else(|| {
                    E::invalid_value(de::Unexpected::Str(text), &self)
                })
        }

        fn visit_string<E: de::Error>(self, text: String) -> Result<u64, E> {
            self.visit_str(&text)
        }
    }
}

/// A map as a list of `[key, value]` pairs, in key order. postcard writes
/// a map and a list of pairs alike, so its bytes are a map's.
pub mod pairs {
    use super::{BTreeMap, Deserialize, Deserializer, Serializer, Vec, de};

    pub fn serialize<S, K, V>(
        map: &BTreeMap<K, V>,
        out: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        K: serde::Serialize,
        V: serde::Serialize,
    {
        out.collect_seq(map)
    }

    /// Refuses a key given twice.
    pub fn deserialize<'de, D, K, V>(
        input: D,
    ) -> Result<BTreeMap<K, V>, D::Error>
    where
        D: Deserializer<'de>,
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
    {
        let pairs = Vec::<(K, V)>::deserialize(input)?;
        let len = pairs.len();
        let map = pairs.into_iter().collect::<BTreeMap<K, V>>();
        if map.len() == len {
            Ok(map)
        } else {
            Err(de::Error::custom("a key is given twice"))
        }
    }
}
