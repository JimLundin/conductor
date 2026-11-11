//! Frame I/O over either transport, using `proto`'s codec. Protocol code
//! reads and writes frames through [`SendHalf`] and [`RecvHalf`] and never
//! sees which transport carries them.

use iroh::endpoint::{ClosedStream, ReadExactError, RecvStream, SendStream, WriteError};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::rtc;

#[derive(Debug, thiserror::Error)]
pub(crate) enum WireError {
    #[error("stream read failed: {0}")]
    Read(#[from] ReadExactError),
    #[error("stream write failed: {0}")]
    Write(#[from] WriteError),
    #[error("could not finish the stream: {0}")]
    Finish(#[from] ClosedStream),
    #[error("{0}")]
    Channel(#[from] rtc::ChannelError),
    #[error("{0}")]
    Frame(#[from] proto::FrameError),
}

/// One side of a QUIC bi-stream or of an ordered, reliable data channel.
pub(crate) enum SendHalf {
    Iroh(SendStream),
    WebRtc(rtc::ChannelSend),
}

pub(crate) enum RecvHalf {
    Iroh(RecvStream),
    WebRtc(rtc::ChannelRecv),
}

impl SendHalf {
    pub(crate) async fn write<T: Serialize>(&mut self, msg: &T) -> Result<(), WireError> {
        let frame = proto::encode(msg)?;
        match self {
            Self::Iroh(send) => send.write_all(&frame).await?,
            Self::WebRtc(send) => send.write(frame).await?,
        }
        Ok(())
    }

    /// Ends this direction. A data channel can't half-close, so this closes it.
    pub(crate) fn finish(&mut self) -> Result<(), WireError> {
        match self {
            Self::Iroh(send) => send.finish()?,
            Self::WebRtc(send) => send.close(),
        }
        Ok(())
    }

    /// Waits until the peer has read everything or stopped reading.
    pub(crate) async fn stopped(&mut self) {
        if let Self::Iroh(send) = self {
            let _ = send.stopped().await;
        }
    }
}

impl RecvHalf {
    /// `Ok(None)` when the peer finished the stream between frames.
    pub(crate) async fn read<T: DeserializeOwned>(&mut self) -> Result<Option<T>, WireError> {
        let body = match self {
            Self::Iroh(recv) => read_iroh(recv).await?,
            Self::WebRtc(recv) => recv.read_body().await?,
        };
        Ok(body.map(|body| proto::decode(&body)).transpose()?)
    }
}

async fn read_iroh(recv: &mut RecvStream) -> Result<Option<Vec<u8>>, WireError> {
    let mut header = [0u8; proto::HEADER_LEN];
    match recv.read_exact(&mut header).await {
        Ok(()) => {
            let mut body = std::iter::repeat_n(0u8, proto::body_len(header)?).collect::<Vec<u8>>();
            recv.read_exact(&mut body).await?;
            Ok(Some(body))
        }
        Err(ReadExactError::FinishedEarly(0)) => Ok(None),
        Err(err) => Err(err.into()),
    }
}
