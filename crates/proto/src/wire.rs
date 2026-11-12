//! Frame I/O on iroh streams, with this crate's codec.

use iroh::endpoint::{ClosedStream, ReadExactError, RecvStream, SendStream, WriteError};
use serde::Serialize;
use serde::de::DeserializeOwned;

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
    #[error("{0}")]
    Frame(#[from] crate::FrameError),
}

pub async fn write<T: Serialize>(send: &mut SendStream, msg: &T) -> Result<(), WireError> {
    Ok(send.write_all(&crate::encode(msg)?).await?)
}

/// `Ok(None)` when the peer finished the stream between frames.
pub async fn read<T: DeserializeOwned>(recv: &mut RecvStream) -> Result<Option<T>, WireError> {
    let mut header = [0u8; crate::HEADER_LEN];
    match recv.read_exact(&mut header).await {
        Ok(()) => {
            let mut body = std::iter::repeat_n(0u8, crate::body_len(header)?).collect::<Vec<u8>>();
            recv.read_exact(&mut body).await?;
            Ok(Some(crate::decode(&body)?))
        }
        Err(ReadExactError::FinishedEarly(0)) => Ok(None),
        Err(err) => Err(err.into()),
    }
}
