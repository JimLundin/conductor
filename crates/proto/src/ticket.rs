//! What a client needs to reach and authenticate to a host, as text that fits
//! a URL fragment (`#join=<ticket>`).
//!
//! Binary layout, then base64url without padding:
//! `version (1 byte) | host endpoint id (32) | token (16) | relay url (utf-8, rest)`.
//!
//! This checks only the layout; `net::parse` also checks that the id is a
//! valid key and the relay a valid URL.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use data_encoding::BASE64URL_NOPAD;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

const VERSION: u8 = 0;
const ID_LEN: usize = 32;
const TOKEN_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    pub host: [u8; ID_LEN],
    pub relay: String,
    pub token: AuthToken,
}

/// S3 placeholder for the fleet certificates of §4: a random secret the host
/// prints in its ticket and requires in every `Hello`.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthToken([u8; TOKEN_LEN]);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TicketError {
    #[error("ticket is not base64url")]
    Encoding,
    #[error("ticket is truncated")]
    Truncated,
    #[error("ticket version {0} is not supported")]
    Version(u8),
    #[error("ticket host id is not a valid key")]
    HostId,
    #[error("ticket relay url is invalid")]
    RelayUrl,
}

/// A `Hello` whose token is not the host's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("wrong token")]
pub struct WrongToken {}

impl AuthToken {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; TOKEN_LEN]) -> Self {
        Self(bytes)
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

/// Never print the secret in logs.
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

    /// # Errors
    ///
    /// The first way `text` departs from the layout.
    pub fn parse(text: &str) -> Result<Self, TicketError> {
        let bytes = BASE64URL_NOPAD
            .decode(text.trim().as_bytes())
            .map_err(|_| TicketError::Encoding)?;
        let (version, rest) =
            bytes.split_first().ok_or(TicketError::Truncated)?;
        match *version {
            VERSION => Self::parse_fields(rest),
            other => Err(TicketError::Version(other)),
        }
    }

    /// Everything after the version byte.
    fn parse_fields(bytes: &[u8]) -> Result<Self, TicketError> {
        let (id, rest) = bytes
            .split_first_chunk::<ID_LEN>()
            .ok_or(TicketError::Truncated)?;
        let (token, relay) = rest
            .split_first_chunk::<TOKEN_LEN>()
            .ok_or(TicketError::Truncated)?;
        let text =
            core::str::from_utf8(relay).map_err(|_| TicketError::RelayUrl)?;
        Ok(Self {
            host: *id,
            relay: String::from(text),
            token: AuthToken(*token),
        })
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;

    use data_encoding::BASE64URL_NOPAD;

    use super::{AuthToken, Ticket, TicketError};

    const TOKEN: AuthToken = AuthToken::from_bytes([1; 16]);

    fn ticket() -> Ticket {
        Ticket {
            host: core::array::from_fn(|i| if i == 0 { 0x58 } else { 0x66 }),
            relay: String::from("https://r.example/"),
            token: TOKEN,
        }
    }

    #[test]
    fn ticket_text_is_fragment_safe_base64url() {
        let text = ticket().to_text();
        assert_eq!(
            text,
            "AFhmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmAQEBAQEBAQEBAQEBAQEBAWh0dHBzOi8vci5leGFtcGxlLw"
        );
        assert_eq!(Ticket::parse(&text), Ok(ticket()));
    }

    #[test]
    fn malformed_tickets_are_refused() -> Result<(), data_encoding::DecodeError>
    {
        let text = ticket().to_text();
        assert_eq!(Ticket::parse("not base64!"), Err(TicketError::Encoding));
        assert_eq!(
            Ticket::parse(
                text.split_at_checked(20).map_or("", |(head, _)| head)
            ),
            Err(TicketError::Truncated)
        );
        let bytes = BASE64URL_NOPAD.decode(text.as_bytes())?;
        let v1 = core::iter::once(1)
            .chain(bytes.into_iter().skip(1))
            .collect::<Vec<u8>>();
        assert_eq!(
            Ticket::parse(&BASE64URL_NOPAD.encode(&v1)),
            Err(TicketError::Version(1))
        );
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
