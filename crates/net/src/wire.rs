//! Frames on iroh streams: a frame is a big-endian `u32` body length and
//! its body. What a body holds is its link's codec (`link::frame`,
//! `host::fleet::frame`).

use iroh::endpoint::{ReadExactError, RecvStream, SendStream, WriteError};

const HEADER_LEN: usize = 4;

/// Larger frames are refused, a received one before its body is read.
/// 16 MiB.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("stream read failed: {0}")]
    Read(#[from] ReadExactError),
    #[error("stream write failed: {0}")]
    Write(#[from] WriteError),
    #[error("frame of {length} bytes exceeds the {limit}-byte limit")]
    TooLarge { length: usize, limit: usize },
}

/// Writes `body` as one frame.
///
/// # Errors
///
/// [`WireError::TooLarge`] when `body` exceeds [`MAX_FRAME_LEN`], and
/// [`WireError::Write`] when the stream fails.
pub async fn write(
    send: &mut SendStream,
    body: &[u8],
) -> Result<(), WireError> {
    send.write_all(&header(body.len())?).await?;
    Ok(send.write_all(body).await?)
}

/// Reads one frame's body: `Ok(None)` when the peer finished the stream
/// between frames.
///
/// # Errors
///
/// [`WireError::TooLarge`] when the header announces more than `limit` or
/// [`MAX_FRAME_LEN`], before reading the body, and [`WireError::Read`]
/// when the stream fails or ends inside a frame.
pub async fn read(
    recv: &mut RecvStream,
    limit: usize,
) -> Result<Option<Vec<u8>>, WireError> {
    let mut header = [0u8; HEADER_LEN];
    match recv.read_exact(&mut header).await {
        Ok(()) => {}
        Err(ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(err) => return Err(err.into()),
    }
    let mut body =
        std::iter::repeat_n(0u8, body_len(header, limit)?).collect::<Vec<u8>>();
    recv.read_exact(&mut body).await?;
    Ok(Some(body))
}

fn header(length: usize) -> Result<[u8; HEADER_LEN], WireError> {
    u32::try_from(length)
        .ok()
        .filter(|_| length <= MAX_FRAME_LEN)
        .map(u32::to_be_bytes)
        .ok_or(WireError::TooLarge {
            length,
            limit: MAX_FRAME_LEN,
        })
}

fn body_len(
    header: [u8; HEADER_LEN],
    bound: usize,
) -> Result<usize, WireError> {
    let limit = bound.min(MAX_FRAME_LEN);
    let length =
        usize::try_from(u32::from_be_bytes(header)).unwrap_or(usize::MAX);
    if length > limit {
        return Err(WireError::TooLarge { length, limit });
    }
    Ok(length)
}

#[cfg(test)]
mod tests {
    use super::{MAX_FRAME_LEN, body_len, header};

    #[test]
    fn a_header_announces_its_body_length_big_endian()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(header(0x0001_0203)?, [0x00, 0x01, 0x02, 0x03]);
        assert_eq!(
            body_len([0x00, 0x01, 0x02, 0x03], MAX_FRAME_LEN)?,
            0x0001_0203
        );
        Ok(())
    }

    #[test]
    fn a_frame_over_16_mib_is_refused_both_ways()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(header(16_777_216)?, [0x01, 0x00, 0x00, 0x00]);
        assert_eq!(body_len([0x01, 0x00, 0x00, 0x00], usize::MAX)?, 16_777_216);
        let refused = "frame of 16777217 bytes exceeds the 16777216-byte limit";
        assert_eq!(
            header(16_777_217).map_err(|error| error.to_string()),
            Err(String::from(refused))
        );
        assert_eq!(
            body_len([0x01, 0x00, 0x00, 0x01], usize::MAX)
                .map_err(|error| error.to_string()),
            Err(String::from(refused))
        );
        Ok(())
    }

    #[test]
    fn a_frame_over_the_readers_limit_is_refused() {
        assert_eq!(
            body_len([0x00, 0x00, 0x10, 0x01], 4096)
                .map_err(|error| error.to_string()),
            Err(String::from(
                "frame of 4097 bytes exceeds the 4096-byte limit"
            ))
        );
    }
}
