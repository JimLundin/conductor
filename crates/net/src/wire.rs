//! Frame I/O on iroh streams: the client link's frames ([`link::frame`]),
//! and the reads of a length-prefixed frame that another framing composes
//! ([`read_header`], [`read_body`]).

use iroh::endpoint::{
    ClosedStream, ReadExactError, RecvStream, SendStream, WriteError,
};
use link::frame::Frame;

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("stream read failed: {0}")]
    Read(#[from] ReadExactError),
    #[error("stream write failed: {0}")]
    Write(#[from] WriteError),
    #[error("could not finish the stream: {0}")]
    Finish(#[from] ClosedStream),
    #[error("could not open a stream: {0}")]
    Open(#[from] iroh::endpoint::ConnectionError),
    #[error("frame of {length} bytes exceeds the {limit}-byte stream limit")]
    Limit { length: usize, limit: usize },
    #[error("{0}")]
    Frame(#[from] link::frame::FrameError),
}

/// Writes `msg` as one frame.
///
/// # Errors
///
/// [`WireError::Frame`] when `msg` doesn't encode, and
/// [`WireError::Write`] when the stream fails.
pub async fn write<T: Frame + Sync>(
    send: &mut SendStream,
    msg: &T,
) -> Result<(), WireError> {
    Ok(send.write_all(&link::frame::encode(msg)?).await?)
}

/// Reads one frame: `Ok(None)` when the peer finished the stream between
/// frames.
///
/// # Errors
///
/// [`WireError::Read`] when the stream fails or ends inside a frame, and
/// [`WireError::Frame`] when the frame doesn't decode.
pub async fn read<T: Frame>(
    recv: &mut RecvStream,
) -> Result<Option<T>, WireError> {
    read_bounded(recv, usize::MAX).await
}

/// Reads a frame with a smaller body limit for this stream.
///
/// # Errors
///
/// Returns the read or decoding error, or rejects an oversized frame before
/// allocating its body.
pub async fn read_bounded<T: Frame>(
    recv: &mut RecvStream,
    limit: usize,
) -> Result<Option<T>, WireError> {
    let Some(header) = read_header::<{ link::frame::HEADER_LEN }>(recv).await?
    else {
        return Ok(None);
    };
    let body = read_body(recv, link::frame::body_len(header)?, limit).await?;
    Ok(Some(link::frame::decode(&body)?))
}

/// Reads a frame's header of `N` bytes: `Ok(None)` when the peer finished
/// the stream between frames.
///
/// # Errors
///
/// [`WireError::Read`] when the stream fails or ends inside the header.
pub async fn read_header<const N: usize>(
    recv: &mut RecvStream,
) -> Result<Option<[u8; N]>, WireError> {
    let mut header = [0u8; N];
    match recv.read_exact(&mut header).await {
        Ok(()) => Ok(Some(header)),
        Err(ReadExactError::FinishedEarly(0)) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Reads the `length` bytes of a body its header announced.
///
/// # Errors
///
/// [`WireError::Limit`] when `length` exceeds `limit`, before allocating
/// the body, and [`WireError::Read`] when the stream fails or ends inside
/// it.
pub async fn read_body(
    recv: &mut RecvStream,
    length: usize,
    limit: usize,
) -> Result<Vec<u8>, WireError> {
    if length > limit {
        return Err(WireError::Limit { length, limit });
    }
    let mut body = std::iter::repeat_n(0u8, length).collect::<Vec<u8>>();
    recv.read_exact(&mut body).await?;
    Ok(body)
}
