//! Frame I/O over either transport, using `proto`'s codec. Protocol code
//! opens streams on a [`Transport`] and reads and writes frames through
//! [`SendHalf`] and [`RecvHalf`], without knowing which transport it is on.
//!
//! Each read and write takes its half and hands it back. A read in progress
//! owns its half, so a caller that must not lose bytes to a `select!` keeps
//! the read inside a stream.

use iroh::endpoint::{
    ClosedStream, Connection, ReadExactError, RecvStream, SendStream, WriteError,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::platform::{ChannelRecv, ChannelSend, Session};

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
    #[error("{0}")]
    Frame(#[from] proto::FrameError),
}

/// What carries the protocol's streams: the iroh connection, or a WebRTC
/// connection negotiated over it. Only the browser builds the second.
#[derive(Debug, Clone)]
pub enum Transport {
    Iroh(Connection),
    WebRtc(Session),
}

impl Transport {
    /// A QUIC bi-stream, or a new ordered, reliable data channel.
    pub async fn open_bi(&self) -> Result<(SendHalf, RecvHalf), WireError> {
        match self {
            Self::Iroh(conn) => {
                let (send, recv) = conn.open_bi().await?;
                Ok((SendHalf::Iroh(send), RecvHalf::Iroh(recv)))
            }
            Self::WebRtc(session) => {
                let (send, recv) = session.open_channel().await.map_err(WireError::Channel)?;
                Ok((SendHalf::WebRtc(send), RecvHalf::WebRtc(recv)))
            }
        }
    }
}

pub enum SendHalf {
    Iroh(SendStream),
    WebRtc(ChannelSend),
}

pub enum RecvHalf {
    Iroh(RecvStream),
    WebRtc(ChannelRecv),
}

impl SendHalf {
    pub async fn write<T: Serialize>(self, msg: &T) -> Result<SendHalf, WireError> {
        let frame = proto::encode(msg)?;
        match self {
            Self::Iroh(mut send) => {
                send.write_all(&frame).await?;
                Ok(Self::Iroh(send))
            }
            Self::WebRtc(send) => {
                send.write(&frame).map_err(WireError::Channel)?;
                Ok(Self::WebRtc(send))
            }
        }
    }

    /// Ends this direction. A data channel can't half-close, so the host
    /// closes it once it has replied.
    pub fn finish(self) -> Result<SendHalf, WireError> {
        match self {
            Self::Iroh(mut send) => {
                send.finish()?;
                Ok(Self::Iroh(send))
            }
            Self::WebRtc(send) => Ok(Self::WebRtc(send)),
        }
    }
}

impl RecvHalf {
    /// `Ok(None)` when the peer finished the stream between frames.
    pub async fn read<T: DeserializeOwned>(self) -> (RecvHalf, Result<Option<T>, WireError>) {
        let (recv, body) = match self {
            Self::Iroh(recv) => {
                let (recv, body) = read_iroh(recv).await;
                (Self::Iroh(recv), body)
            }
            Self::WebRtc(recv) => {
                let (recv, body) = recv.read_body().await;
                (Self::WebRtc(recv), body)
            }
        };
        (recv, body.and_then(decode))
    }
}

fn decode<T: DeserializeOwned>(body: Option<Vec<u8>>) -> Result<Option<T>, WireError> {
    Ok(body.map(|body| proto::decode(&body)).transpose()?)
}

async fn read_iroh(mut recv: RecvStream) -> (RecvStream, Result<Option<Vec<u8>>, WireError>) {
    let mut header = [0u8; proto::HEADER_LEN];
    let read = recv.read_exact(&mut header).await;
    match read {
        Ok(()) => read_iroh_body(recv, header).await,
        Err(ReadExactError::FinishedEarly(0)) => (recv, Ok(None)),
        Err(err) => (recv, Err(err.into())),
    }
}

async fn read_iroh_body(
    mut recv: RecvStream,
    header: [u8; proto::HEADER_LEN],
) -> (RecvStream, Result<Option<Vec<u8>>, WireError>) {
    match proto::body_len(header) {
        Ok(len) => {
            let mut body = std::iter::repeat_n(0u8, len).collect::<Vec<u8>>();
            let read = recv.read_exact(&mut body).await;
            (recv, read.map(|()| Some(body)).map_err(WireError::from))
        }
        Err(err) => (recv, Err(err.into())),
    }
}
