//! What a client needs to reach and authenticate to a host, as text that fits
//! a URL fragment (`#join=<ticket>`).
//!
//! Binary layout, then base64url without padding:
//! `version (1 byte) | host endpoint id (32) | token (16) | relay url (utf-8, rest)`.

use std::fmt;
use std::str::FromStr;

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

#[derive(Debug, Clone, thiserror::Error)]
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
    pub fn from_bytes(bytes: [u8; TOKEN_LEN]) -> Self {
        Self(bytes)
    }
}

impl fmt::Display for AuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&HEXLOWER.encode(&self.0))
    }
}

/// Never print the secret in logs.
impl fmt::Debug for AuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthToken(..)")
    }
}

impl FromStr for AuthToken {
    type Err = TicketError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        HEXLOWER
            .decode(s.as_bytes())
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .map(Self)
            .ok_or(TicketError::Token)
    }
}

impl Serialize for AuthToken {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for AuthToken {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for Ticket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let relay = self.relay.to_string();
        let mut bytes = Vec::with_capacity(1 + ID_LEN + TOKEN_LEN + relay.len());
        bytes.push(VERSION);
        bytes.extend_from_slice(self.host.as_bytes());
        bytes.extend_from_slice(&self.token.0);
        bytes.extend_from_slice(relay.as_bytes());
        f.write_str(&BASE64URL_NOPAD.encode(&bytes))
    }
}

impl FromStr for Ticket {
    type Err = TicketError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = BASE64URL_NOPAD
            .decode(s.trim().as_bytes())
            .map_err(|_| TicketError::Encoding)?;
        let (&version, rest) = bytes.split_first().ok_or(TicketError::Truncated)?;
        if version != VERSION {
            return Err(TicketError::Version(version));
        }
        let (id, rest) = rest
            .split_first_chunk::<ID_LEN>()
            .ok_or(TicketError::Truncated)?;
        let (token, relay) = rest
            .split_first_chunk::<TOKEN_LEN>()
            .ok_or(TicketError::Truncated)?;
        let relay = std::str::from_utf8(relay).map_err(|_| TicketError::RelayUrl)?;
        Ok(Self {
            host: EndpointId::from_bytes(id).map_err(|_| TicketError::HostId)?,
            relay: relay.parse().map_err(|_| TicketError::RelayUrl)?,
            token: AuthToken(*token),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket() -> Ticket {
        // The ed25519 base point: a fixed, valid public key.
        let mut id = [0x66; 32];
        id[0] = 0x58;
        Ticket {
            host: EndpointId::from_bytes(&id).unwrap(),
            relay: "https://r.example/".parse().unwrap(),
            token: AuthToken([1; 16]),
        }
    }

    #[test]
    fn ticket_text_is_fragment_safe_base64url() {
        let text = ticket().to_string();
        assert_eq!(
            text,
            "AFhmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmZmAQEBAQEBAQEBAQEBAQEBAWh0dHBzOi8vci5leGFtcGxlLw"
        );
        assert_eq!(text.parse::<Ticket>().unwrap(), ticket());
    }

    #[test]
    fn malformed_tickets_are_refused() {
        let text = ticket().to_string();
        assert!(matches!(
            "not base64!".parse::<Ticket>(),
            Err(TicketError::Encoding)
        ));
        assert!(matches!(
            text[..20].parse::<Ticket>(),
            Err(TicketError::Truncated)
        ));
        let mut v1 = BASE64URL_NOPAD.decode(text.as_bytes()).unwrap();
        v1[0] = 1;
        assert!(matches!(
            BASE64URL_NOPAD.encode(&v1).parse::<Ticket>(),
            Err(TicketError::Version(1))
        ));
    }

    #[test]
    fn token_round_trips_as_hex_and_hides_in_debug() {
        let token: AuthToken = "000102030405060708090a0b0c0d0e0f".parse().unwrap();
        assert_eq!(
            token.0,
            [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
        );
        assert_eq!(format!("{token:?}"), "AuthToken(..)");
        assert!("00".parse::<AuthToken>().is_err());
    }
}
