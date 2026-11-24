//! The types that have yet to move to the crates that own them (ADR 0003):
//! the client link, `link`, composes them into its frames. It is a pure
//! core, `no_std`.
//!
//! The client link is JSON, so a change to a type it reaches bumps
//! `link::ALPN`.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A client-generated, fleet-unique 128-bit random id for idempotent retries.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
)]
pub struct CommandId(
    #[serde(with = "ulid")]
    #[schemars(with = "String")]
    pub u128,
);

/// A command id's JSON is its ULID text, as the host writes its own ids.
mod ulid {
    use super::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        id: &u128,
        out: S,
    ) -> Result<S::Ok, S::Error> {
        host::Ulid(*id).serialize(out)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        input: D,
    ) -> Result<u128, D::Error> {
        host::Ulid::deserialize(input).map(|id| id.0)
    }
}
