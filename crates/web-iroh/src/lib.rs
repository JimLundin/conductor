//! iroh in the browser as a byte pipe: a connection to one endpoint through
//! its home relay, with bidirectional streams of frames. A frame is one body,
//! which the pipe sends behind its header ([`proto::header`]); what a body
//! means is the caller's.

use iroh::endpoint::{
    BindError, ClosedStream, ConnectError, ConnectionError, ReadExactError,
    RecvStream, SendStream, WriteError, presets,
};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl};
use tokio::sync::Mutex;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsError, JsValue};

#[derive(Debug, thiserror::Error)]
pub enum PipeError {
    #[error("an endpoint id is 32 bytes, not {0}")]
    EndpointLength(usize),
    #[error("not an endpoint id: {0}")]
    EndpointId(#[from] iroh::KeyParsingError),
    #[error("not a relay URL: {0}")]
    Relay(#[from] iroh::RelayUrlParseError),
    #[error("could not start iroh: {0}")]
    Bind(#[from] BindError),
    #[error("could not connect: {0}")]
    Connect(#[from] ConnectError),
    #[error("could not open a stream: {0}")]
    Open(#[from] ConnectionError),
    #[error("stream read failed: {0}")]
    Read(#[from] ReadExactError),
    #[error("stream write failed: {0}")]
    Write(#[from] WriteError),
    #[error("could not finish the stream: {0}")]
    Finish(#[from] ClosedStream),
    #[error("{0}")]
    Frame(#[from] proto::FrameError),
}

impl From<PipeError> for JsValue {
    fn from(error: PipeError) -> Self {
        JsError::new(&error.to_string()).into()
    }
}

/// Connects to `endpoint`, a 32-byte endpoint id, through `relay`, its home
/// relay, speaking `alpn`.
///
/// # Errors
///
/// When the id or the relay doesn't parse, iroh can't start, or the
/// connection fails.
#[wasm_bindgen]
pub async fn connect(
    endpoint: Vec<u8>,
    relay: String,
    alpn: Vec<u8>,
) -> Result<Connection, PipeError> {
    let id = <[u8; 32]>::try_from(endpoint.as_slice())
        .map_err(|_| PipeError::EndpointLength(endpoint.len()))?;
    let addr = EndpointAddr::new(EndpointId::from_bytes(&id)?);
    let home = relay.parse::<RelayUrl>()?;
    let local = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Custom(home.clone().into()))
        .bind()
        .await?;
    Connection::dial(local, addr.with_relay_url(home), &alpn).await
}

/// A connection to one endpoint, from a local endpoint of its own.
#[wasm_bindgen]
#[derive(Debug)]
pub struct Connection {
    endpoint: Endpoint,
    connection: iroh::endpoint::Connection,
}

impl Connection {
    /// Connects from `endpoint`, which the connection then owns, to `addr`.
    ///
    /// # Errors
    ///
    /// When the connection fails, which closes `endpoint`.
    pub async fn dial(
        endpoint: Endpoint,
        addr: EndpointAddr,
        alpn: &[u8],
    ) -> Result<Self, PipeError> {
        match endpoint.connect(addr, alpn).await {
            Ok(connection) => Ok(Self {
                endpoint,
                connection,
            }),
            Err(error) => {
                endpoint.close().await;
                Err(error.into())
            }
        }
    }
}

#[wasm_bindgen]
impl Connection {
    /// # Errors
    ///
    /// When the connection has closed.
    #[wasm_bindgen(js_name = openBi)]
    pub async fn open_bi(&self) -> Result<Stream, PipeError> {
        let (send, recv) = self.connection.open_bi().await?;
        Ok(Stream {
            send: Mutex::new(send),
            recv: Mutex::new(recv),
        })
    }

    /// Resolves once the connection has closed, with why.
    pub async fn closed(&self) -> String {
        self.connection.closed().await.to_string()
    }

    /// Closes the connection and its local endpoint.
    pub async fn close(&self) {
        self.connection.close(0u32.into(), b"");
        self.endpoint.close().await;
    }
}

/// A bidirectional stream of frames. Sending and receiving may run at once.
#[wasm_bindgen]
#[derive(Debug)]
pub struct Stream {
    send: Mutex<SendStream>,
    recv: Mutex<RecvStream>,
}

#[wasm_bindgen]
impl Stream {
    /// Sends `frame` as one frame.
    ///
    /// # Errors
    ///
    /// When it exceeds the frame limit or the stream fails.
    pub async fn send(&self, frame: Vec<u8>) -> Result<(), PipeError> {
        let header = proto::header(frame.len())?;
        let mut send = self.send.lock().await;
        send.write_all(&header).await?;
        Ok(send.write_all(&frame).await?)
    }

    /// The next frame's body, or none once the peer finished the stream
    /// between frames.
    ///
    /// # Errors
    ///
    /// When the stream fails or ends inside a frame, or a header announces
    /// more than the frame limit.
    pub async fn recv(&self) -> Result<Option<Vec<u8>>, PipeError> {
        read_frame(&mut *self.recv.lock().await).await
    }

    /// Ends the sending side once what was sent is delivered.
    ///
    /// # Errors
    ///
    /// When the stream was already finished or reset.
    pub async fn finish(&self) -> Result<(), PipeError> {
        Ok(self.send.lock().await.finish()?)
    }
}

async fn read_frame(
    recv: &mut RecvStream,
) -> Result<Option<Vec<u8>>, PipeError> {
    let mut header = [0u8; proto::HEADER_LEN];
    match recv.read_exact(&mut header).await {
        Ok(()) => {
            let mut body = std::iter::repeat_n(0u8, proto::body_len(header)?)
                .collect::<Vec<u8>>();
            recv.read_exact(&mut body).await?;
            Ok(Some(body))
        }
        Err(ReadExactError::FinishedEarly(0)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}
