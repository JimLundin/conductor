//! Frame I/O over either transport, using `proto`'s codec. Protocol code
//! opens streams on a [`Transport`] and reads and writes frames through
//! [`SendHalf`] and [`RecvHalf`], without knowing which transport it is on.

use iroh::endpoint::{
    ClosedStream, Connection, ReadExactError, RecvStream, SendStream, WriteError,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::PathKind;
#[cfg(target_family = "wasm")]
use crate::platform::rtc;

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
    #[error("data channel: {0}")]
    Channel(String),
    #[error(transparent)]
    Frame(#[from] proto::FrameError),
}

/// What carries the protocol's streams: the iroh connection, or a WebRTC
/// connection negotiated over it.
#[derive(Debug, Clone)]
pub enum Transport {
    Iroh(Connection),
    #[cfg(target_family = "wasm")]
    WebRtc(rtc::Session),
}

impl Transport {
    /// A QUIC bi-stream, or a new ordered, reliable data channel.
    pub async fn open_bi(&self) -> Result<(SendHalf, RecvHalf), WireError> {
        match self {
            Self::Iroh(conn) => {
                let (send, recv) = conn.open_bi().await?;
                Ok((SendHalf::Iroh(send), RecvHalf::Iroh(recv)))
            }
            #[cfg(target_family = "wasm")]
            Self::WebRtc(session) => {
                let (send, recv) = session.open_channel().await.map_err(WireError::Channel)?;
                Ok((SendHalf::WebRtc(send), RecvHalf::WebRtc(recv)))
            }
        }
    }

    /// Resolves when a WebRTC connection fails or closes. An iroh connection
    /// reports its end through its streams instead, so this never resolves.
    pub async fn dropped(&self) {
        match self {
            Self::Iroh(_) => std::future::pending().await,
            #[cfg(target_family = "wasm")]
            Self::WebRtc(session) => session.closed().await,
        }
    }

    pub fn is_webrtc(&self) -> bool {
        !matches!(self, Self::Iroh(_))
    }

    /// The path to show, given the iroh connection's selected path.
    pub fn path(&self, iroh: PathKind) -> PathKind {
        match self {
            Self::Iroh(_) => iroh,
            #[cfg(target_family = "wasm")]
            Self::WebRtc(session) => PathKind::WebRtc(session.pair()),
        }
    }
}

pub enum SendHalf {
    Iroh(SendStream),
    #[cfg(target_family = "wasm")]
    WebRtc(rtc::ChannelSend),
}

pub enum RecvHalf {
    Iroh(RecvStream),
    #[cfg(target_family = "wasm")]
    WebRtc(rtc::ChannelRecv),
}

impl SendHalf {
    pub async fn write<T: Serialize>(&mut self, msg: &T) -> Result<(), WireError> {
        let frame = proto::encode(msg)?;
        match self {
            Self::Iroh(send) => send.write_all(&frame).await?,
            #[cfg(target_family = "wasm")]
            Self::WebRtc(send) => send.write(&frame).map_err(WireError::Channel)?,
        }
        Ok(())
    }

    /// Ends this direction. A data channel can't half-close, so the host
    /// closes it once it has replied.
    pub fn finish(&mut self) -> Result<(), WireError> {
        match self {
            Self::Iroh(send) => send.finish()?,
            #[cfg(target_family = "wasm")]
            Self::WebRtc(_) => {}
        }
        Ok(())
    }
}

impl RecvHalf {
    /// `Ok(None)` when the peer finished the stream between frames.
    pub async fn read<T: DeserializeOwned>(&mut self) -> Result<Option<T>, WireError> {
        let body = match self {
            Self::Iroh(recv) => read_iroh(recv).await?,
            #[cfg(target_family = "wasm")]
            Self::WebRtc(recv) => recv.read_body().await?,
        };
        Ok(body.map(|body| proto::decode(&body)).transpose()?)
    }
}

async fn read_iroh(recv: &mut RecvStream) -> Result<Option<Vec<u8>>, WireError> {
    let mut header = [0u8; proto::HEADER_LEN];
    match recv.read_exact(&mut header).await {
        Ok(()) => {}
        Err(ReadExactError::FinishedEarly(0)) => return Ok(None),
        Err(err) => return Err(err.into()),
    }
    let mut body = vec![0u8; proto::body_len(header)?];
    recv.read_exact(&mut body).await?;
    Ok(Some(body))
}
