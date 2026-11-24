//! The id of a pending issue edit, which the app mints in the issue
//! command: a retry sends the same edit again under a new command, and the
//! home applies it once.

use alloc::string::String;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// 128 random bits, whose text is a ULID's as the `ulid` crate writes it.
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
pub struct EditId(
    #[serde(with = "crate::ulid_text")]
    #[schemars(with = "String")]
    pub u128,
);

#[cfg(test)]
mod tests {
    use super::EditId;

    const MAX: &str = r#""7ZZZZZZZZZZZZZZZZZZZZZZZZZ""#;

    #[test]
    fn an_edit_id_is_written_as_the_ulid_crate_writes_a_ulid() {
        assert_eq!(
            serde_json::to_string(&EditId(u128::MAX)).ok().as_deref(),
            Some(MAX)
        );
    }

    #[test]
    fn an_edit_id_in_lower_case_reads_as_in_upper_case() {
        assert_eq!(
            serde_json::from_str::<EditId>(&MAX.to_ascii_lowercase()).ok(),
            Some(EditId(u128::MAX))
        );
    }

    #[test]
    fn postcard_writes_an_edit_id_as_its_number() {
        let id = 0x0123_4567_89ab_cdef_0123_4567_89ab_cdef_u128;
        assert_eq!(
            postcard::to_allocvec(&EditId(id)).ok(),
            postcard::to_allocvec(&id).ok()
        );
    }

    #[test]
    fn only_26_crockford_digits_are_an_edit_id() {
        for text in [
            r#""""#,
            r#""0000000000000000000000000""#,
            r#""0000000000000000000000000U""#,
        ] {
            assert!(serde_json::from_str::<EditId>(text).is_err(), "{text}");
        }
    }
}
