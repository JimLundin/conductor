//! A command's id on the client link, which the app mints so a retried
//! command answers from its receipt.

use alloc::string::String;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A client-generated, fleet-unique 128-bit random id for idempotent
/// retries, whose text is a ULID's as the `ulid` crate writes it.
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
    #[serde(with = "home::ulid_text")]
    #[schemars(with = "String")]
    pub u128,
);

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
    fn only_26_crockford_digits_are_a_command_id() {
        for text in [
            r#""""#,
            r#""0000000000000000000000000""#,
            r#""0000000000000000000000000U""#,
        ] {
            assert!(serde_json::from_str::<CommandId>(text).is_err(), "{text}");
        }
    }
}
