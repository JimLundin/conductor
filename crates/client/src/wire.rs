//! Frame I/O over iroh streams, using `proto`'s codec.

use iroh::endpoint::{ReadExactError, RecvStream, SendStream, WriteError};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("stream read failed: {0}")]
    Read(#[from] ReadExactError),
    #[error("stream write failed: {0}")]
    Write(#[from] WriteError),
    #[error(transparent)]
    Frame(#[from] proto::FrameError),
}

/// `Ok(None)` when the peer finished the stream between frames.
pub async fn read<T: DeserializeOwned>(recv: &mut RecvStream) -> Result<Option<T>, WireError> {
    let mut header = [0u8; proto::HEADER_LEN];
    match recv.read_exact(&mut header).await {
        Ok(()) => {}
        Err(ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(err) => return Err(err.into()),
    }
    let mut body = vec![0u8; proto::body_len(header)?];
    recv.read_exact(&mut body).await?;
    Ok(Some(proto::decode(&body)?))
}

pub async fn write<T: Serialize>(send: &mut SendStream, msg: &T) -> Result<(), WireError> {
    send.write_all(&proto::encode(msg)?).await?;
    Ok(())
}
