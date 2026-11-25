//! What a client needs to reach and be admitted to a host, as text that
//! fits a URL fragment (`#join=<ticket>`).
//!
//! Binary layout, then base64url without padding:
//! `version (1 byte) | host endpoint id (32) | token (16) | relay url (utf-8, rest)`.
//!
//! Only the web app reads it, in `web/src/ticket.ts`.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use data_encoding::BASE64URL_NOPAD;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

const VERSION: u8 = 0;
const ID_LEN: usize = 32;
const TOKEN_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Ticket {
    pub host: [u8; ID_LEN],
    pub relay: String,
    pub token: AuthToken,
}

/// S3 placeholder for the fleet certificates of §4: a random secret the host
/// prints in its ticket and requires in every `Hello`.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthToken([u8; TOKEN_LEN]);

/// A `Hello` whose token is not the host's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("wrong token")]
pub struct WrongToken {}

impl AuthToken {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; TOKEN_LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn to_bytes(self) -> [u8; TOKEN_LEN] {
        self.0
    }

    /// Checks `self` against the host's token in constant time, so the
    /// time a refusal takes says nothing about the token.
    ///
    /// # Errors
    ///
    /// [`WrongToken`] when the tokens differ.
    pub fn check(&self, expected: &Self) -> Result<(), WrongToken> {
        match self.0.as_slice().ct_eq(expected.0.as_slice()).unwrap_u8() {
            1 => Ok(()),
            _ => Err(WrongToken {}),
        }
    }
}

impl fmt::Debug for AuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthToken(..)")
    }
}

impl Ticket {
    /// The ticket as fragment-safe text.
    #[must_use]
    pub fn to_text(&self) -> String {
        let bytes = core::iter::once(VERSION)
            .chain(self.host)
            .chain(self.token.0)
            .chain(self.relay.bytes())
            .collect::<Vec<u8>>();
        BASE64URL_NOPAD.encode(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::string::String;
    use std::path::PathBuf;

    use serde::Serialize;
    use serde_json::Value;

    use super::{AuthToken, Ticket};

    const TOKEN: AuthToken = AuthToken::from_bytes([1; 16]);

    fn ticket() -> Ticket {
        Ticket {
            host: core::array::from_fn(|i| if i == 0 { 0x58 } else { 0x66 }),
            relay: String::from("https://r.example/"),
            token: TOKEN,
        }
    }

    #[derive(Serialize)]
    struct Example {
        ticket: Ticket,
        text: String,
    }

    #[test]
    fn ticket_text_is_fragment_safe_base64url() {
        assert_eq!(
            ticket().to_text(),
            "AFhmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmAQEBAQEBAQEBAQEBAQEBAWh0dHBzOi8vci5leGFtcGxlLw"
        );
    }

    #[test]
    fn the_worked_example_is_published_beside_link()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("ticket.example.json");
        let example = serde_json::to_value(Example {
            ticket: ticket(),
            text: ticket().to_text(),
        })?;
        let published =
            serde_json::from_slice::<Value>(&std::fs::read(&path)?)?;
        assert_eq!(published, example);
        Ok(())
    }

    #[test]
    fn token_hides_in_debug() {
        assert_eq!(format!("{:?}", AuthToken([1; 16])), "AuthToken(..)");
    }
    #[test]
    fn only_the_same_token_checks() {
        assert!(TOKEN.check(&AuthToken::from_bytes([1; 16])).is_ok());
        assert!(TOKEN.check(&AuthToken::from_bytes([2; 16])).is_err());
    }
}
