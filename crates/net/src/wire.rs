//! Frame I/O on iroh streams: the client link's frames ([`link::frame`])
//! and, in [`fleet`], the fleet protocol's ([`host::fleet::frame`]).

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
    #[error("{0}")]
    FleetFrame(#[from] host::fleet::frame::FrameError),
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
    let body =
        read_body(recv, limit, |header| Ok(link::frame::body_len(header)?))
            .await?;
    Ok(body.map(|read| link::frame::decode(&read)).transpose()?)
}

/// Frame I/O between hosts.
pub mod fleet {
    use host::fleet::frame::{self, Frame};
    use iroh::endpoint::{RecvStream, SendStream};

    use super::WireError;

    /// Writes `msg` as one frame.
    ///
    /// # Errors
    ///
    /// [`WireError::FleetFrame`] when `msg` doesn't encode, and
    /// [`WireError::Write`] when the stream fails.
    pub async fn write<T: Frame + Sync>(
        send: &mut SendStream,
        msg: &T,
    ) -> Result<(), WireError> {
        Ok(send.write_all(&frame::encode(msg)?).await?)
    }

    /// Reads a frame of at most `limit` bytes: `Ok(None)` when the peer
    /// finished the stream between frames.
    ///
    /// # Errors
    ///
    /// Returns the read or decoding error, or rejects an oversized frame
    /// before allocating its body.
    pub async fn read_bounded<T: Frame>(
        recv: &mut RecvStream,
        limit: usize,
    ) -> Result<Option<T>, WireError> {
        let body = super::read_body(recv, limit, |header| {
            Ok(frame::body_len(header)?)
        })
        .await?;
        Ok(body.map(|read| frame::decode(&read)).transpose()?)
    }
}

/// Reads one frame's body undecoded, its length announced by a header
/// `body_len` reads: `Ok(None)` when the peer finished the stream between
/// frames.
async fn read_body(
    recv: &mut RecvStream,
    limit: usize,
    body_len: impl FnOnce([u8; link::frame::HEADER_LEN]) -> Result<usize, WireError>,
) -> Result<Option<Vec<u8>>, WireError> {
    let mut header = [0u8; link::frame::HEADER_LEN];
    match recv.read_exact(&mut header).await {
        Ok(()) => {
            let length = body_len(header)?;
            if length > limit {
                return Err(WireError::Limit { length, limit });
            }
            let mut body =
                std::iter::repeat_n(0u8, length).collect::<Vec<u8>>();
            recv.read_exact(&mut body).await?;
            Ok(Some(body))
        }
        Err(ReadExactError::FinishedEarly(0)) => Ok(None),
        Err(err) => Err(err.into()),
    }
}
