//! A 128-bit id as the `ulid` crate's text, for `#[serde(with)]`.
//!
//! Only where a format is human-readable, since JSON's numbers are exact
//! only to 2^53; elsewhere the number, so postcard's bytes are a `u128`'s.
//! The crate's own serde support needs `std`, which these cores don't have.

use alloc::string::String;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use ulid::Ulid;

/// # Errors
///
/// When the format fails to write.
pub fn serialize<S: Serializer>(id: &u128, out: S) -> Result<S::Ok, S::Error> {
    if out.is_human_readable() {
        out.collect_str(&Ulid(*id))
    } else {
        id.serialize(out)
    }
}

/// # Errors
///
/// When the input is not a ULID's text, or, in a format that isn't
/// human-readable, a `u128`.
pub fn deserialize<'de, D: Deserializer<'de>>(
    input: D,
) -> Result<u128, D::Error> {
    if input.is_human_readable() {
        let text = String::deserialize(input)?;
        Ulid::from_string(&text)
            .map(u128::from)
            .map_err(de::Error::custom)
    } else {
        u128::deserialize(input)
    }
}
