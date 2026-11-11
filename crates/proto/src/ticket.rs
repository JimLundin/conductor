//! What a client needs to reach and authenticate to a host, as text that fits
//! a URL fragment (`#join=<ticket>`).
//!
//! Binary layout, then base64url without padding:
//! `version (1 byte) | host endpoint id (32) | token (16) | relay url (utf-8, rest)`.

use std::fmt;

use data_encoding::{BASE64URL_NOPAD, HEXLOWER};
use iroh_base::{EndpointId, RelayUrl};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

const VERSION: u8 = 0;
const ID_LEN: usize = 32;
const TOKEN_LEN: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub host: EndpointId,
    pub relay: RelayUrl,
    pub token: AuthToken,
}

/// S3 placeholder for the fleet certificates of §4: a random secret the host
/// prints in its ticket and requires in every `Hello`.
#[derive(Clone, Copy, PartialEq, Eq)]
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
    #[error("token is not 32 lowercase hex digits")]
    Token,
}

impl AuthToken {
    pub fn from_bytes(bytes: [u8; TOKEN_LEN]) -> AuthToken {
        AuthToken(bytes)
    }

    /// The token as 32 lowercase hex digits.
    pub fn to_text(&self) -> String {
        HEXLOWER.encode(&self.0)
    }

    pub fn parse(text: &str) -> Result<AuthToken, TicketError> {
        HEXLOWER
            .decode(text.as_bytes())
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .map(AuthToken)
            .ok_or(TicketError::Token)
    }
}

/// Never print the secret in logs.
impl fmt::Debug for AuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthToken(..)")
    }
}

impl Serialize for AuthToken {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_text())
    }
}

impl<'de> Deserialize<'de> for AuthToken {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        AuthToken::parse(&String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

impl Ticket {
    /// The ticket as fragment-safe text.
    pub fn to_text(&self) -> String {
        let bytes = std::iter::once(VERSION)
            .chain(self.host.as_bytes().iter().copied())
            .chain(self.token.0)
            .chain(self.relay.to_string().into_bytes())
            .collect::<Vec<u8>>();
        BASE64URL_NOPAD.encode(&bytes)
    }

    pub fn parse(text: &str) -> Result<Ticket, TicketError> {
        let bytes = BASE64URL_NOPAD
            .decode(text.trim().as_bytes())
            .map_err(|_| TicketError::Encoding)?;
        let (version, rest) = bytes.split_first().ok_or(TicketError::Truncated)?;
        match version.to_owned() {
            VERSION => Ticket::parse_fields(rest),
            other => Err(TicketError::Version(other)),
        }
    }

    /// Everything after the version byte.
    fn parse_fields(bytes: &[u8]) -> Result<Ticket, TicketError> {
        let (id, rest) = bytes
            .split_first_chunk::<ID_LEN>()
            .ok_or(TicketError::Truncated)?;
        let (token, relay) = rest
            .split_first_chunk::<TOKEN_LEN>()
            .ok_or(TicketError::Truncated)?;
        let relay = std::str::from_utf8(relay).map_err(|_| TicketError::RelayUrl)?;
        Ok(Ticket {
            host: EndpointId::from_bytes(id).map_err(|_| TicketError::HostId)?,
            relay: relay.parse().map_err(|_| TicketError::RelayUrl)?,
            token: AuthToken(token.to_owned()),
        })
    }
}

#[cfg(test)]
mod tests {
    use data_encoding::BASE64URL_NOPAD;
    use iroh_base::EndpointId;

    use super::{AuthToken, Ticket, TicketError};

    fn ticket() -> Ticket {
        // The ed25519 base point: a fixed, valid public key.
        let id = std::iter::once(0x58)
            .chain([0x66; 31])
            .collect::<Vec<u8>>()
            .try_into()
            .unwrap();
        Ticket {
            host: EndpointId::from_bytes(&id).unwrap(),
            relay: "https://r.example/".parse().unwrap(),
            token: AuthToken([1; 16]),
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
    fn malformed_tickets_are_refused() {
        let text = ticket().to_text();
        assert_eq!(Ticket::parse("not base64!"), Err(TicketError::Encoding));
        assert_eq!(
            Ticket::parse(text.split_at(20).0),
            Err(TicketError::Truncated)
        );
        let bytes = BASE64URL_NOPAD.decode(text.as_bytes()).unwrap();
        let v1 = std::iter::once(1)
            .chain(bytes.into_iter().skip(1))
            .collect::<Vec<u8>>();
        assert_eq!(
            Ticket::parse(&BASE64URL_NOPAD.encode(&v1)),
            Err(TicketError::Version(1))
        );
    }

    #[test]
    fn token_round_trips_as_hex_and_hides_in_debug() {
        let token = AuthToken::parse("000102030405060708090a0b0c0d0e0f").unwrap();
        assert_eq!(
            token.0,
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
        );
        assert_eq!(token.to_text(), "000102030405060708090a0b0c0d0e0f");
        assert_eq!(format!("{token:?}"), "AuthToken(..)");
        assert_eq!(AuthToken::parse("00"), Err(TicketError::Token));
    }
}
