//! iroh in the browser as a byte pipe: a connection to one endpoint through
//! its home relay, with bidirectional streams of frames ([`net::wire`]). A
//! frame is one body; what a body means is the caller's.

use iroh::endpoint::{
    BindError, ClosedStream, ConnectError, ConnectionError, RecvStream,
    SendStream, presets,
};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode, RelayUrl};
use net::wire::{self, WireError};
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
    #[error("could not finish the stream: {0}")]
    Finish(#[from] ClosedStream),
    #[error("{0}")]
    Wire(#[from] WireError),
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
        Ok(wire::write(&mut *self.send.lock().await, &frame).await?)
    }

    /// The next frame's body, or none once the peer finished the stream
    /// between frames.
    ///
    /// # Errors
    ///
    /// When the stream fails or ends inside a frame, or a header announces
    /// more than the frame limit.
    pub async fn recv(&self) -> Result<Option<Vec<u8>>, PipeError> {
        Ok(
            wire::read(&mut *self.recv.lock().await, wire::MAX_FRAME_LEN)
                .await?,
        )
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
