use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

const TOKEN_LEN: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnrollmentToken([u8; TOKEN_LEN]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("wrong enrollment token")]
pub struct WrongEnrollmentToken {}

impl EnrollmentToken {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; TOKEN_LEN]) -> Self {
        Self(bytes)
    }

    pub fn check(&self, expected: &Self) -> Result<(), WrongEnrollmentToken> {
        match self.0.as_slice().ct_eq(expected.0.as_slice()).unwrap_u8() {
            1 => Ok(()),
            _ => Err(WrongEnrollmentToken {}),
        }
    }
}

impl fmt::Debug for EnrollmentToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("EnrollmentToken(..)")
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;

    use super::EnrollmentToken;

    const TOKEN: EnrollmentToken = EnrollmentToken::from_bytes([1; 16]);

    #[test]
    fn only_the_same_enrollment_token_checks() {
        assert!(TOKEN.check(&EnrollmentToken::from_bytes([1; 16])).is_ok());
        assert!(TOKEN.check(&EnrollmentToken::from_bytes([2; 16])).is_err());
    }

    #[test]
    fn enrollment_token_hides_in_debug() {
        assert_eq!(format!("{TOKEN:?}"), "EnrollmentToken(..)");
    }

    #[test]
    fn enrollment_token_is_its_sixteen_bytes_on_the_fleet_protocol()
    -> Result<(), postcard::Error> {
        assert_eq!(postcard::to_allocvec(&TOKEN)?, [1; 16]);
        Ok(())
    }
}
