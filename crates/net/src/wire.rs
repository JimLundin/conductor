//! Frame I/O on iroh streams, each message in its link's codec
//! ([`proto::Frame`]).

use iroh::endpoint::{
    ClosedStream, ReadExactError, RecvStream, SendStream, WriteError,
};
use proto::Frame;

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
    Frame(#[from] proto::FrameError),
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
    Ok(send.write_all(&proto::encode(msg)?).await?)
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
    let mut header = [0u8; proto::HEADER_LEN];
    match recv.read_exact(&mut header).await {
        Ok(()) => {
            let length = proto::body_len(header)?;
            if length > limit {
                return Err(WireError::Limit { length, limit });
            }
            let mut body =
                std::iter::repeat_n(0u8, length).collect::<Vec<u8>>();
            recv.read_exact(&mut body).await?;
            Ok(Some(proto::decode(&body)?))
        }
        Err(ReadExactError::FinishedEarly(0)) => Ok(None),
        Err(err) => Err(err.into()),
    }
}
