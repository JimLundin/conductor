//! The fleet protocol's frames, between hosts only.
//!
//! A frame is a `u32` big-endian body length followed by its postcard
//! body, and only the fleet's own frames ([`Frame`]) travel in it. Callers
//! own the I/O: read [`HEADER_LEN`] bytes, pass them to [`body_len`], read
//! that many bytes, pass them to [`decode`].

use alloc::vec::Vec;

use proto::fleet::{Reply, Request, SignedConfiguration};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub const HEADER_LEN: usize = 4;

/// Larger frames are refused before their body is read. 16 MiB.
pub const MAX_FRAME_LEN: u32 = 16_777_216;

/// A message that opens or answers a stream between hosts.
pub trait Frame: Serialize + DeserializeOwned {}

impl Frame for Request {}

impl Frame for Reply {}

impl Frame for SignedConfiguration {}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame of {0} bytes exceeds the {MAX_FRAME_LEN}-byte limit")]
    TooLarge(u64),
    #[error("malformed frame body: {0}")]
    Postcard(#[from] postcard::Error),
}

/// The frame of `msg`.
///
/// # Errors
///
/// [`FrameError::Postcard`] when `msg` doesn't serialize, and
/// [`FrameError::TooLarge`] when its body exceeds the 16 MiB limit.
pub fn encode<T: Frame>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let body = postcard::to_allocvec(msg)?;
    let len = u32::try_from(body.len())
        .ok()
        .filter(|len| *len <= MAX_FRAME_LEN)
        .ok_or_else(|| {
            FrameError::TooLarge(u64::try_from(body.len()).unwrap_or(u64::MAX))
        })?;
    Ok(len
        .to_be_bytes()
        .into_iter()
        .chain(body)
        .collect::<Vec<u8>>())
}

/// The body length a frame header announces.
///
/// # Errors
///
/// [`FrameError::TooLarge`] when it exceeds the 16 MiB limit.
pub fn body_len(header: [u8; HEADER_LEN]) -> Result<usize, FrameError> {
    let len = u32::from_be_bytes(header);
    match len {
        ..=MAX_FRAME_LEN => usize::try_from(len)
            .map_err(|_| FrameError::TooLarge(u64::from(len))),
        _ => Err(FrameError::TooLarge(u64::from(len))),
    }
}

/// The value a frame body holds.
///
/// # Errors
///
/// [`FrameError::Postcard`] when the body is not a `T`.
pub fn decode<T: Frame>(body: &[u8]) -> Result<T, FrameError> {
    Ok(postcard::from_bytes(body)?)
}

#[cfg(test)]
mod tests {
    use proto::fleet::Reply;

    use super::{body_len, decode, encode};

    #[test]
    fn fleet_traffic_is_length_then_postcard()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(encode(&Reply::Refused)?, b"\x00\x00\x00\x01\x04");
        assert_eq!(decode::<Reply>(b"\x04")?, Reply::Refused);
        Ok(())
    }

    #[test]
    fn oversized_header_is_refused_before_reading_the_body()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            body_len([0x01, 0x00, 0x00, 0x01])
                .map_err(|error| error.to_string()),
            Err(String::from(
                "frame of 16777217 bytes exceeds the 16777216-byte limit"
            ))
        );
        assert_eq!(body_len([0x01, 0x00, 0x00, 0x00])?, 16_777_216);
        Ok(())
    }
}
