//! The browser's half of the WebRTC upgrade (§5.4), through `web-sys`.
//!
//! [`negotiate`] sends an SDP offer on a new stream of the authenticated
//! iroh connection and trickles candidates both ways on it. The host's answer
//! carries its DTLS fingerprint and the offer carries ours, so the data
//! channels can only reach the host this iroh connection authenticated. The
//! stream stays open for the life of the WebRTC connection; closing either
//! ends both.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use iroh::endpoint::Connection;
use js_sys::{Reflect, Uint8Array};
use n0_future::time::sleep;
use n0_future::{StreamExt, task};
use proto::{CandidateType, Open, RtcOffer, Signal};
use tokio::sync::{mpsc, oneshot, watch};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    MessageEvent, RtcConfiguration, RtcDataChannel, RtcDataChannelInit, RtcDataChannelState,
    RtcDataChannelType, RtcIceCandidateInit, RtcIceServer, RtcPeerConnection,
    RtcPeerConnectionIceEvent, RtcPeerConnectionState, RtcSdpType, RtcSessionDescriptionInit,
};

use crate::CandidatePair;
use crate::wire::{RecvHalf, SendHalf, WireError};

/// ICE, DTLS and SCTP must be up within this, or the upgrade fails and the
/// link stays relayed.
const UPGRADE_TIMEOUT: Duration = Duration::from_secs(15);

/// A WebRTC connection to the host. Cheap to clone; the connection closes
/// when the last clone is dropped.
#[derive(Debug, Clone)]
pub struct Session {
    inner: Rc<Inner>,
}

struct Inner {
    pc: RtcPeerConnection,
    closed: watch::Receiver<bool>,
    pair: Option<CandidatePair>,
    /// Open for as long as the connection is usable; it carries nothing.
    liveness: Channel,
    _signaling: oneshot::Sender<()>,
    _on_state: Closure<dyn FnMut()>,
    _on_candidate: Closure<dyn FnMut(RtcPeerConnectionIceEvent)>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").field("pair", &self.pair).finish()
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.pc.set_onconnectionstatechange(None);
        self.pc.set_onicecandidate(None);
        self.pc.close();
    }
}

impl Session {
    /// The selected candidate pair, if the browser's `getStats` named it.
    pub fn pair(&self) -> Option<CandidatePair> {
        self.inner.pair
    }

    /// Resolves when the connection fails, the host ends it, or the host
    /// closes every channel.
    pub async fn closed(&self) {
        let mut closed = self.inner.closed.clone();
        let mut liveness = self.inner.liveness.state.clone();
        tokio::select! {
            _ = closed.wait_for(|closed| *closed) => {}
            _ = liveness.wait_for(|state| *state == ChannelState::Closed) => {}
        }
    }

    /// A new ordered, reliable data channel, once it is open.
    pub async fn open_channel(&self) -> Result<(ChannelSend, ChannelRecv), String> {
        let mut channel = Channel::create(&self.inner.pc, "stream")?;
        channel.opened().await?;
        let messages = channel.messages.take().expect("taken once");
        let channel = Rc::new(channel);
        Ok((
            ChannelSend {
                channel: channel.clone(),
            },
            ChannelRecv {
                _channel: channel,
                messages,
                buffer: proto::FrameBuffer::empty(),
            },
        ))
    }
}

/// Offers a WebRTC connection over `conn` and waits until a data channel
/// is open. On failure the reason is also reported to the host for its log.
pub async fn negotiate(conn: Connection, stun_servers: Vec<String>) -> Result<Session, String> {
    let started = js_sys::Date::now();
    let pc = peer_connection(&stun_servers)?;
    let (closed_tx, closed) = watch::channel(false);
    let closed_tx = Rc::new(closed_tx);
    let on_state = Closure::<dyn FnMut()>::new({
        let pc = pc.clone();
        let closed_tx = closed_tx.clone();
        move || {
            let state = pc.connection_state();
            tracing::info!(?state, "webrtc connection state");
            if matches!(
                state,
                RtcPeerConnectionState::Failed
                    | RtcPeerConnectionState::Closed
                    | RtcPeerConnectionState::Disconnected
            ) {
                closed_tx.send_replace(true);
            }
        }
    });
    pc.set_onconnectionstatechange(Some(on_state.as_ref().unchecked_ref()));

    let gathered = Rc::new(RefCell::new(Vec::<CandidateType>::new()));
    let (local_tx, local) = mpsc::unbounded_channel::<String>();
    let on_candidate = Closure::<dyn FnMut(RtcPeerConnectionIceEvent)>::new({
        let gathered = gathered.clone();
        move |event: RtcPeerConnectionIceEvent| {
            let Some(candidate) = event.candidate() else {
                return;
            };
            let text = candidate.candidate();
            if text.is_empty() {
                return;
            }
            let kind = CandidateType::of_candidate(&text);
            tracing::info!(?kind, "webrtc local candidate");
            gathered.borrow_mut().extend(kind);
            local_tx.send(text).ok();
        }
    });
    pc.set_onicecandidate(Some(on_candidate.as_ref().unchecked_ref()));

    let mut liveness = Channel::create(&pc, "conductor")?;
    let (send, recv) = conn
        .open_bi()
        .await
        .map_err(|err| format!("could not open the signaling stream: {err}"))?;
    let mut send = SendHalf::Iroh(send);
    let offer = JsFuture::from(pc.create_offer()).await.map_err(js)?;
    let sdp = Reflect::get(&offer, &"sdp".into())
        .ok()
        .and_then(|sdp| sdp.as_string())
        .ok_or("the browser's offer has no SDP")?;
    let description = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
    description.set_sdp(&sdp);
    JsFuture::from(pc.set_local_description(&description))
        .await
        .map_err(js)?;
    send.write(&Open::Rtc(RtcOffer { sdp }))
        .await
        .map_err(|err| err.to_string())?;
    tracing::info!(stun = ?stun_servers, "webrtc offer sent");

    let (reports, reports_rx) = mpsc::unbounded_channel();
    let (stop, stop_rx) = oneshot::channel();
    let remote = Rc::new(RefCell::new(Vec::<Option<CandidateType>>::new()));
    task::spawn(signaling(
        pc.clone(),
        send,
        RecvHalf::Iroh(recv),
        local,
        reports_rx,
        stop_rx,
        closed_tx,
        remote.clone(),
    ));

    let mut closed_rx = closed.clone();
    let outcome = tokio::select! {
        opened = liveness.opened() => opened,
        _ = closed_rx.wait_for(|closed| *closed) => Err("the connection failed".to_string()),
        () = sleep(UPGRADE_TIMEOUT) => Err("timed out".to_string()),
    };
    let elapsed_ms = (js_sys::Date::now() - started) as u64;
    if let Err(why) = outcome {
        let reason = format!(
            "{why} after {elapsed_ms} ms: ice {:?}, connection {:?}, gathering {:?}; \
             browser gathered {:?}; host sent {:?}",
            pc.ice_connection_state(),
            pc.connection_state(),
            pc.ice_gathering_state(),
            gathered.borrow(),
            remote.borrow(),
        );
        tracing::warn!(reason, "webrtc upgrade failed");
        reports
            .send(Signal::Failed {
                reason: reason.clone(),
            })
            .ok();
        stop.send(()).ok();
        pc.close();
        return Err(reason);
    }
    let pair = selected_pair(&pc).await;
    tracing::info!(elapsed_ms, ?pair, "webrtc data channel open");
    if let Some(pair) = pair {
        reports
            .send(Signal::Selected {
                local: pair.local,
                remote: pair.remote,
            })
            .ok();
    }
    Ok(Session {
        inner: Rc::new(Inner {
            pc,
            closed,
            pair,
            liveness,
            _signaling: stop,
            _on_state: on_state,
            _on_candidate: on_candidate,
        }),
    })
}

fn peer_connection(stun_servers: &[String]) -> Result<RtcPeerConnection, String> {
    let urls = js_sys::Array::new();
    for url in stun_servers {
        urls.push(&url.into());
    }
    let server = RtcIceServer::new();
    server.set_urls(&urls);
    let config = RtcConfiguration::new();
    config.set_ice_servers(&js_sys::Array::of1(&server));
    RtcPeerConnection::new_with_configuration(&config).map_err(js)
}

/// Carries candidates and reports both ways until the host closes the
/// stream, which ends the connection, or the session is dropped.
#[allow(clippy::too_many_arguments)]
async fn signaling(
    pc: RtcPeerConnection,
    mut send: SendHalf,
    recv: RecvHalf,
    mut local: mpsc::UnboundedReceiver<String>,
    mut reports: mpsc::UnboundedReceiver<Signal>,
    mut stop: oneshot::Receiver<()>,
    closed: Rc<watch::Sender<bool>>,
    remote: Rc<RefCell<Vec<Option<CandidateType>>>>,
) {
    let incoming = n0_future::stream::unfold(recv, |mut recv| async move {
        let signal = recv.read::<Signal>().await;
        Some((signal, recv))
    });
    let mut incoming = std::pin::pin!(incoming);
    loop {
        let result: Result<(), String> = tokio::select! {
            Some(candidate) = local.recv() => {
                send.write(&Signal::Candidate { candidate }).await.map_err(|e| e.to_string())
            }
            Some(report) = reports.recv() => send.write(&report).await.map_err(|e| e.to_string()),
            signal = incoming.next() => match signal {
                Some(Ok(Some(signal))) => apply(&pc, signal, &remote).await,
                Some(Ok(None)) | None => Err("the host closed the signaling stream".into()),
                Some(Err(err)) => Err(err.to_string()),
            },
            _ = &mut stop => {
                while let Ok(report) = reports.try_recv() {
                    send.write(&report).await.ok();
                }
                send.finish().ok();
                return;
            }
        };
        if let Err(reason) = result {
            tracing::info!(reason, "webrtc signaling ended");
            closed.send_replace(true);
            return;
        }
    }
}

async fn apply(
    pc: &RtcPeerConnection,
    signal: Signal,
    remote: &RefCell<Vec<Option<CandidateType>>>,
) -> Result<(), String> {
    match signal {
        Signal::Answer { sdp } => {
            let description = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
            description.set_sdp(&sdp);
            JsFuture::from(pc.set_remote_description(&description))
                .await
                .map_err(js)?;
            for line in sdp.lines() {
                if let Some(candidate) = line.strip_prefix("a=")
                    && candidate.starts_with("candidate:")
                {
                    remote
                        .borrow_mut()
                        .push(CandidateType::of_candidate(candidate));
                }
            }
        }
        Signal::Candidate { candidate } => {
            let kind = CandidateType::of_candidate(&candidate);
            tracing::info!(?kind, "webrtc remote candidate");
            remote.borrow_mut().push(kind);
            let init = RtcIceCandidateInit::new(&candidate);
            init.set_sdp_m_line_index(Some(0));
            JsFuture::from(pc.add_ice_candidate_with_opt_rtc_ice_candidate_init(Some(&init)))
                .await
                .map_err(js)?;
        }
        Signal::Selected { .. } | Signal::Failed { .. } => {}
    }
    Ok(())
}

/// Reads the selected candidate pair from `getStats`: the transport's
/// `selectedCandidatePairId` where the browser sets it, else the nominated,
/// succeeded pair.
async fn selected_pair(pc: &RtcPeerConnection) -> Option<CandidatePair> {
    let report: js_sys::Map = JsFuture::from(pc.get_stats()).await.ok()?.unchecked_into();
    let field = |value: &wasm_bindgen::JsValue, name: &str| Reflect::get(value, &name.into()).ok();
    let text = |value: &wasm_bindgen::JsValue, name: &str| field(value, name)?.as_string();
    let mut selected = None;
    let mut nominated = None;
    report.for_each(&mut |stat, _| match text(&stat, "type").as_deref() {
        Some("transport") => {
            selected = selected
                .take()
                .or_else(|| text(&stat, "selectedCandidatePairId"))
        }
        Some("candidate-pair")
            if field(&stat, "selected").and_then(|v| v.as_bool()) == Some(true)
                || (field(&stat, "nominated").and_then(|v| v.as_bool()) == Some(true)
                    && text(&stat, "state").as_deref() == Some("succeeded")) =>
        {
            nominated = text(&stat, "id");
        }
        _ => {}
    });
    let pair = report.get(&selected.or(nominated)?.into());
    let kind = |id_field: &str| {
        let candidate = report.get(&text(&pair, id_field)?.into());
        CandidateType::parse(&text(&candidate, "candidateType")?)
    };
    Some(CandidatePair {
        local: kind("localCandidateId")?,
        remote: kind("remoteCandidateId")?,
    })
}

fn js(err: wasm_bindgen::JsValue) -> String {
    err.as_string()
        .or_else(|| {
            err.dyn_ref::<js_sys::Error>()
                .map(|e| String::from(e.message()))
        })
        .unwrap_or_else(|| format!("{err:?}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelState {
    Connecting,
    Open,
    Closed,
}

/// A data channel and its callbacks, which live exactly as long as it does.
struct Channel {
    dc: RtcDataChannel,
    state: watch::Receiver<ChannelState>,
    messages: Option<mpsc::UnboundedReceiver<Vec<u8>>>,
    _on_open: Closure<dyn FnMut()>,
    _on_close: Closure<dyn FnMut()>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
}

impl Channel {
    fn create(pc: &RtcPeerConnection, label: &str) -> Result<Self, String> {
        let init = RtcDataChannelInit::new();
        init.set_ordered(true);
        let dc = pc.create_data_channel_with_data_channel_dict(label, &init);
        dc.set_binary_type(RtcDataChannelType::Arraybuffer);
        let (state_tx, state) = watch::channel(ChannelState::Connecting);
        let state_tx = Rc::new(state_tx);
        let (messages_tx, messages) = mpsc::unbounded_channel();
        let messages_tx = Rc::new(RefCell::new(Some(messages_tx)));
        let on_open = Closure::<dyn FnMut()>::new({
            let state_tx = state_tx.clone();
            move || {
                state_tx.send_replace(ChannelState::Open);
            }
        });
        let on_close = Closure::<dyn FnMut()>::new({
            let messages_tx = messages_tx.clone();
            move || {
                messages_tx.borrow_mut().take();
                state_tx.send_replace(ChannelState::Closed);
            }
        });
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            if let Some(tx) = messages_tx.borrow().as_ref() {
                tx.send(Uint8Array::new(&event.data()).to_vec()).ok();
            }
        });
        dc.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        dc.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        dc.set_onerror(Some(on_close.as_ref().unchecked_ref()));
        dc.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        Ok(Self {
            dc,
            state,
            messages: Some(messages),
            _on_open: on_open,
            _on_close: on_close,
            _on_message: on_message,
        })
    }

    async fn opened(&mut self) -> Result<(), String> {
        match self
            .state
            .wait_for(|s| *s != ChannelState::Connecting)
            .await
        {
            Ok(state) if *state == ChannelState::Open => Ok(()),
            _ => Err("the data channel closed before it opened".into()),
        }
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.dc.set_onopen(None);
        self.dc.set_onclose(None);
        self.dc.set_onerror(None);
        self.dc.set_onmessage(None);
        self.dc.close();
    }
}

pub struct ChannelSend {
    channel: Rc<Channel>,
}

impl ChannelSend {
    pub fn write(&mut self, frame: &[u8]) -> Result<(), String> {
        if self.channel.dc.ready_state() != RtcDataChannelState::Open {
            return Err("the data channel closed".into());
        }
        for chunk in frame.chunks(proto::CHUNK_LEN) {
            self.channel.dc.send_with_u8_array(chunk).map_err(js)?;
        }
        Ok(())
    }
}

pub struct ChannelRecv {
    _channel: Rc<Channel>,
    messages: mpsc::UnboundedReceiver<Vec<u8>>,
    buffer: proto::FrameBuffer,
}

impl ChannelRecv {
    /// Cancel-safe: a partly received frame stays in the buffer.
    pub async fn read_body(&mut self) -> Result<Option<Vec<u8>>, WireError> {
        loop {
            if let proto::Reassembly::Frame { body, rest } = self.buffer.next_body()? {
                self.buffer = rest;
                return Ok(Some(body));
            }
            match self.messages.recv().await {
                Some(message) => {
                    let buffer = std::mem::replace(&mut self.buffer, proto::FrameBuffer::empty());
                    self.buffer = buffer.push(&message);
                }
                None if self.buffer.remainder() == proto::Remainder::Nothing => return Ok(None),
                None => {
                    return Err(WireError::Channel("closed in the middle of a frame".into()));
                }
            }
        }
    }
}
