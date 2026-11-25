//! The fleet protocol's frames, between hosts only.
//!
//! A frame's body is its postcard, and only the fleet's own frames
//! ([`Frame`]) travel in it. `net::wire` frames the body.

use alloc::vec::Vec;

use super::{Reply, Request, SignedConfiguration};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// A message that opens or answers a stream between hosts.
pub trait Frame: Serialize + DeserializeOwned {}

impl Frame for Request {}

impl Frame for Reply {}

impl Frame for SignedConfiguration {}

/// The frame body of `msg`.
///
/// # Errors
///
/// When `msg` doesn't serialize.
pub fn encode<T: Frame>(msg: &T) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(msg)
}

/// The value a frame body holds.
///
/// # Errors
///
/// When the body is not a `T`.
pub fn decode<T: Frame>(body: &[u8]) -> Result<T, postcard::Error> {
    postcard::from_bytes(body)
}

#[cfg(test)]
mod tests {
    use crate::fleet::Reply;

    use super::{decode, encode};

    #[test]
    fn fleet_traffic_is_postcard() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(encode(&Reply::Refused)?, b"\x04");
        assert_eq!(decode::<Reply>(b"\x04")?, Reply::Refused);
        Ok(())
    }
}
