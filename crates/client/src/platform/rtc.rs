//! The browser's half of the WebRTC upgrade (§5.4), through `web-sys`.
//!
//! [`negotiate`] sends an SDP offer on a new stream of the authenticated
//! iroh connection and trickles candidates both ways on it. The host's answer
//! carries its DTLS fingerprint and the offer carries ours, so the data
//! channels can only reach the host this iroh connection authenticated. The
//! stream stays open for the life of the WebRTC connection; closing either
//! ends both.
//!
//! The browser's callbacks only forward their events into a channel. The
//! [`Signaling`] actor receives them, decides what they mean, and owns the
//! signaling stream.

use std::convert::Infallible;
use std::rc::Rc;
use std::time::Duration;

use iroh::endpoint::Connection;
use js_sys::{Reflect, Uint8Array};
use n0_future::boxed::BoxStream;
use n0_future::time::{Instant, sleep};
use n0_future::{StreamExt, task};
use proto::{CandidateType, FrameBuffer, Open, Reassembly, Remainder, RtcOffer, Signal};
use tokio::sync::{mpsc, oneshot, watch};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
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

/// Whether the peer connection can still carry traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Peer {
    Usable,
    Lost,
}

/// A WebRTC connection to the host. Cheap to clone; the connection closes
/// when the last clone is dropped.
#[derive(Debug, Clone)]
pub(crate) struct Session {
    inner: Rc<Inner>,
}

struct Inner {
    pc: RtcPeerConnection,
    peer: watch::Receiver<Peer>,
    pair: Option<CandidatePair>,
    /// Open for as long as the connection is usable; it carries nothing.
    liveness: Channel,
    /// Dropping the last sender stops the signaling actor.
    _signaling: mpsc::UnboundedSender<Message>,
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
    pub(crate) fn pair(&self) -> Option<CandidatePair> {
        self.inner.pair
    }

    /// Resolves when the connection fails, the host ends it, or the host
    /// closes every channel.
    pub(crate) async fn closed(&self) {
        let mut peer = self.inner.peer.clone();
        let mut liveness = self.inner.liveness.state.clone();
        tokio::select! {
            _ = peer.wait_for(|peer| Peer::Lost.eq(peer)) => {}
            _ = liveness.wait_for(|state| ChannelState::Closed.eq(state)) => {}
        }
    }

    /// A new ordered, reliable data channel, once it is open.
    pub(crate) async fn open_channel(&self) -> Result<(ChannelSend, ChannelRecv), String> {
        let (channel, events) = Channel::create(&self.inner.pc, "stream")?;
        channel.opened().await?;
        let channel = Rc::new(channel);
        Ok((
            ChannelSend {
                channel: channel.clone(),
            },
            ChannelRecv {
                _channel: channel,
                events,
                buffer: FrameBuffer::empty(),
            },
        ))
    }
}

/// Offers a WebRTC connection over `conn` and waits until a data channel
/// is open. On failure the reason is also reported to the host for its log.
pub(crate) async fn negotiate(
    conn: Connection,
    stun_servers: Vec<String>,
) -> Result<Session, String> {
    let started = Instant::now();
    let pc = peer_connection(&stun_servers)?;
    let (inbox, messages) = mpsc::unbounded_channel();
    let state_pc = pc.clone();
    let state_inbox = inbox.clone();
    let on_state = Closure::<dyn FnMut()>::new(move || {
        let _ = state_inbox.send(Message::State(state_pc.connection_state()));
    });
    pc.set_onconnectionstatechange(Some(on_state.as_ref().unchecked_ref()));
    let candidate_inbox = inbox.clone();
    let on_candidate = Closure::<dyn FnMut(RtcPeerConnectionIceEvent)>::new(move |event| {
        let _ = candidate_inbox.send(Message::Candidate(event));
    });
    pc.set_onicecandidate(Some(on_candidate.as_ref().unchecked_ref()));

    let (liveness, _) = Channel::create(&pc, "conductor")?;
    let (send, recv) = conn
        .open_bi()
        .await
        .map_err(|err| format!("could not open the signaling stream: {err}"))?;
    let offer = JsFuture::from(pc.create_offer()).await.map_err(js)?;
    let sdp = Reflect::get(&offer, &JsValue::from("sdp"))
        .ok()
        .and_then(|sdp| sdp.as_string())
        .ok_or("the browser's offer has no SDP")?;
    let description = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
    description.set_sdp(&sdp);
    JsFuture::from(pc.set_local_description(&description))
        .await
        .map_err(js)?;
    let send = SendHalf::Iroh(send)
        .write(&Open::Rtc(RtcOffer { sdp }))
        .await
        .map_err(|err| err.to_string())?;
    tracing::info!(stun = ?stun_servers, "webrtc offer sent");

    let (peer_tx, peer) = watch::channel(Peer::Usable);
    let signaling = Signaling {
        pc: pc.clone(),
        messages,
        peer: peer_tx,
        gathered: Vec::new(),
        remote: Vec::new(),
        host: Host::Up {
            send,
            incoming: signals(RecvHalf::Iroh(recv)),
        },
    };
    task::spawn(signaling.run());

    let mut lost = peer.clone();
    let outcome = tokio::select! {
        opened = liveness.opened() => opened,
        _ = lost.wait_for(|peer| Peer::Lost.eq(peer)) => Err(String::from("the connection failed")),
        () = sleep(UPGRADE_TIMEOUT) => Err(String::from("timed out")),
    };
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match outcome {
        Ok(()) => {
            let pair = selected_pair(&pc).await;
            tracing::info!(elapsed_ms, ?pair, "webrtc data channel open");
            let _ = inbox.send(Message::Opened(pair));
            Ok(Session {
                inner: Rc::new(Inner {
                    pc,
                    peer,
                    pair,
                    liveness,
                    _signaling: inbox,
                    _on_state: on_state,
                    _on_candidate: on_candidate,
                }),
            })
        }
        Err(why) => {
            let (reply, reason) = oneshot::channel();
            let _ = inbox.send(Message::Failed {
                why,
                elapsed_ms,
                reply,
            });
            let reason = reason
                .await
                .unwrap_or_else(|_| String::from("the signaling actor stopped"));
            pc.close();
            Err(reason)
        }
    }
}

fn peer_connection(stun_servers: &[String]) -> Result<RtcPeerConnection, String> {
    let urls = stun_servers
        .iter()
        .map(JsValue::from)
        .collect::<js_sys::Array>();
    let server = RtcIceServer::new();
    server.set_urls(&urls);
    let config = RtcConfiguration::new();
    config.set_ice_servers(&js_sys::Array::of1(&server));
    RtcPeerConnection::new_with_configuration(&config).map_err(js)
}

/// What the browser's callbacks and [`negotiate`] tell the signaling actor.
enum Message {
    State(RtcPeerConnectionState),
    Candidate(RtcPeerConnectionIceEvent),
    /// The data channel opened, over this pair if the browser named it.
    Opened(Option<CandidatePair>),
    /// The upgrade failed for `why`. The reply is the reason in full.
    Failed {
        why: String,
        elapsed_ms: u64,
        reply: oneshot::Sender<String>,
    },
}

/// The signaling stream's state.
enum Host {
    Up {
        send: SendHalf,
        incoming: BoxStream<Result<Option<Signal>, WireError>>,
    },
    /// The host closed the stream, or it failed.
    Gone,
}

enum Input {
    Message(Option<Message>),
    Signal(Option<Result<Option<Signal>, WireError>>),
}

/// Every sender was dropped: the session or the failed negotiation is gone.
#[derive(Debug)]
struct Stopped {}

/// Carries candidates and reports both ways until the host closes the
/// stream, which ends the connection, or the session is dropped. It keeps
/// what both sides gathered, for the reason of a failed upgrade.
struct Signaling {
    pc: RtcPeerConnection,
    messages: mpsc::UnboundedReceiver<Message>,
    peer: watch::Sender<Peer>,
    gathered: Vec<CandidateType>,
    remote: Vec<Option<CandidateType>>,
    host: Host,
}

impl Signaling {
    async fn run(mut self) -> Result<Infallible, Stopped> {
        loop {
            let input = self.next_input().await;
            self.on(input).await?;
        }
    }

    async fn next_input(&mut self) -> Input {
        match &mut self.host {
            Host::Up { incoming, .. } => tokio::select! {
                message = self.messages.recv() => Input::Message(message),
                signal = incoming.next() => Input::Signal(signal),
            },
            Host::Gone => Input::Message(self.messages.recv().await),
        }
    }

    async fn on(&mut self, input: Input) -> Result<(), Stopped> {
        let host = std::mem::replace(&mut self.host, Host::Gone);
        self.host = self.step(host, input).await?;
        Ok(())
    }

    async fn step(&mut self, host: Host, input: Input) -> Result<Host, Stopped> {
        match input {
            Input::Message(Some(Message::State(state))) => {
                self.on_state(state);
                Ok(host)
            }
            Input::Message(Some(Message::Candidate(event))) => {
                Ok(self.on_candidate(host, event).await)
            }
            Input::Message(Some(Message::Opened(Some(pair)))) => {
                let selected = Signal::Selected {
                    local: pair.local,
                    remote: pair.remote,
                };
                Ok(self.send(host, selected).await)
            }
            Input::Message(Some(Message::Opened(None))) => Ok(host),
            Input::Message(Some(Message::Failed {
                why,
                elapsed_ms,
                reply,
            })) => {
                let reason = format!(
                    "{why} after {elapsed_ms} ms: ice {:?}, connection {:?}, gathering {:?}; \
                     browser gathered {:?}; host sent {:?}",
                    self.pc.ice_connection_state(),
                    self.pc.connection_state(),
                    self.pc.ice_gathering_state(),
                    self.gathered,
                    self.remote,
                );
                tracing::warn!(reason, "webrtc upgrade failed");
                let _ = reply.send(reason.clone());
                Ok(self.send(host, Signal::Failed { reason }).await)
            }
            Input::Message(None) => Err(stop(host)),
            Input::Signal(Some(Ok(Some(signal)))) => match self.apply(signal).await {
                Ok(()) => Ok(host),
                Err(reason) => Ok(self.ended(reason)),
            },
            Input::Signal(Some(Ok(None)) | None) => {
                Ok(self.ended(String::from("the host closed the signaling stream")))
            }
            Input::Signal(Some(Err(err))) => Ok(self.ended(err.to_string())),
        }
    }

    fn on_state(&self, state: RtcPeerConnectionState) {
        tracing::info!(?state, "webrtc connection state");
        if let RtcPeerConnectionState::Failed
        | RtcPeerConnectionState::Closed
        | RtcPeerConnectionState::Disconnected = state
        {
            self.peer.send_replace(Peer::Lost);
        }
    }

    async fn on_candidate(&mut self, host: Host, event: RtcPeerConnectionIceEvent) -> Host {
        match event.candidate().map(|candidate| candidate.candidate()) {
            Some(text) => match text.as_str() {
                "" => host,
                _ => {
                    let kind = CandidateType::of_candidate(&text);
                    tracing::info!(?kind, "webrtc local candidate");
                    self.gathered.extend(kind);
                    self.send(host, Signal::Candidate { candidate: text }).await
                }
            },
            None => host,
        }
    }

    async fn send(&self, host: Host, signal: Signal) -> Host {
        match host {
            Host::Up { send, incoming } => match send.write(&signal).await {
                Ok(send) => Host::Up { send, incoming },
                Err(err) => self.ended(err.to_string()),
            },
            Host::Gone => Host::Gone,
        }
    }

    fn ended(&self, reason: String) -> Host {
        tracing::info!(reason, "webrtc signaling ended");
        self.peer.send_replace(Peer::Lost);
        Host::Gone
    }

    async fn apply(&mut self, signal: Signal) -> Result<(), String> {
        match signal {
            Signal::Answer { sdp } => {
                let description = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
                description.set_sdp(&sdp);
                JsFuture::from(self.pc.set_remote_description(&description))
                    .await
                    .map_err(js)?;
                self.remote.extend(
                    sdp.lines()
                        .filter_map(|line| line.strip_prefix("a="))
                        .filter_map(|attribute| {
                            attribute
                                .strip_prefix("candidate:")
                                .map(|_| CandidateType::of_candidate(attribute))
                        }),
                );
                Ok(())
            }
            Signal::Candidate { candidate } => {
                let kind = CandidateType::of_candidate(&candidate);
                tracing::info!(?kind, "webrtc remote candidate");
                self.remote.push(kind);
                let init = RtcIceCandidateInit::new(&candidate);
                init.set_sdp_m_line_index(Some(0));
                JsFuture::from(
                    self.pc
                        .add_ice_candidate_with_opt_rtc_ice_candidate_init(Some(&init)),
                )
                .await
                .map_err(js)?;
                Ok(())
            }
            Signal::Selected { .. } | Signal::Failed { .. } => Ok(()),
        }
    }
}

/// Ends the signaling stream once nobody needs it.
fn stop(host: Host) -> Stopped {
    match host {
        Host::Up { send, .. } => {
            let _ = send.finish();
            Stopped {}
        }
        Host::Gone => Stopped {},
    }
}

fn signals(recv: RecvHalf) -> BoxStream<Result<Option<Signal>, WireError>> {
    Box::pin(n0_future::stream::unfold(recv, |recv| async move {
        let (recv, signal) = recv.read::<Signal>().await;
        Some((signal, recv))
    }))
}

/// Reads the selected candidate pair from `getStats`: the transport's
/// `selectedCandidatePairId` where the browser sets it, else the last
/// selected, or nominated and succeeded, pair.
async fn selected_pair(pc: &RtcPeerConnection) -> Option<CandidatePair> {
    let report = JsFuture::from(pc.get_stats())
        .await
        .ok()?
        .unchecked_into::<js_sys::Map>();
    let stats = report
        .values()
        .into_iter()
        .filter_map(Result::ok)
        .collect::<Vec<JsValue>>();
    let selected = stats
        .iter()
        .find_map(|stat| match text(stat, "type").as_deref() {
            Some("transport") => text(stat, "selectedCandidatePairId"),
            _ => None,
        });
    let nominated = stats
        .iter()
        .rev()
        .find(|stat| {
            matches!(
                (
                    text(stat, "type").as_deref(),
                    flag(stat, "selected"),
                    flag(stat, "nominated"),
                    text(stat, "state").as_deref(),
                ),
                (Some("candidate-pair"), Some(Flag::True), _, _)
                    | (
                        Some("candidate-pair"),
                        _,
                        Some(Flag::True),
                        Some("succeeded")
                    )
            )
        })
        .and_then(|stat| text(stat, "id"));
    let pair = report.get(&JsValue::from(selected.or(nominated)?));
    Some(CandidatePair {
        local: candidate_type(&report, &pair, "localCandidateId")?,
        remote: candidate_type(&report, &pair, "remoteCandidateId")?,
    })
}

/// A JS field whose value is `true`.
enum Flag {
    True,
}

fn candidate_type(report: &js_sys::Map, pair: &JsValue, id_field: &str) -> Option<CandidateType> {
    let candidate = report.get(&JsValue::from(text(pair, id_field)?));
    CandidateType::parse(&text(&candidate, "candidateType")?)
}

fn field(value: &JsValue, name: &str) -> Option<JsValue> {
    Reflect::get(value, &JsValue::from(name)).ok()
}

fn text(value: &JsValue, name: &str) -> Option<String> {
    field(value, name)?.as_string()
}

fn flag(value: &JsValue, name: &str) -> Option<Flag> {
    field(value, name)?.as_bool()?.then_some(Flag::True)
}

fn js(err: JsValue) -> String {
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

impl ChannelState {
    fn opened(&self) -> Result<(), String> {
        match self {
            ChannelState::Open => Ok(()),
            ChannelState::Connecting | ChannelState::Closed => {
                Err(String::from("the data channel closed before it opened"))
            }
        }
    }
}

/// What a data channel's callbacks tell its reader.
enum ChannelEvent {
    Message(MessageEvent),
    Closed,
}

/// A data channel and its callbacks, which live exactly as long as it does.
struct Channel {
    dc: RtcDataChannel,
    state: watch::Receiver<ChannelState>,
    _on_open: Closure<dyn FnMut()>,
    _on_close: Closure<dyn FnMut()>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
}

impl Channel {
    fn create(
        pc: &RtcPeerConnection,
        label: &str,
    ) -> Result<(Channel, mpsc::UnboundedReceiver<ChannelEvent>), String> {
        let init = RtcDataChannelInit::new();
        init.set_ordered(true);
        let dc = pc.create_data_channel_with_data_channel_dict(label, &init);
        dc.set_binary_type(RtcDataChannelType::Arraybuffer);
        let (state_tx, state) = watch::channel(ChannelState::Connecting);
        let state_tx = Rc::new(state_tx);
        let (events_tx, events) = mpsc::unbounded_channel();
        let open_state = state_tx.clone();
        let on_open = Closure::<dyn FnMut()>::new(move || {
            open_state.send_replace(ChannelState::Open);
        });
        let close_events = events_tx.clone();
        let on_close = Closure::<dyn FnMut()>::new(move || {
            let _ = close_events.send(ChannelEvent::Closed);
            state_tx.send_replace(ChannelState::Closed);
        });
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event| {
            let _ = events_tx.send(ChannelEvent::Message(event));
        });
        dc.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        dc.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        dc.set_onerror(Some(on_close.as_ref().unchecked_ref()));
        dc.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        let channel = Channel {
            dc,
            state,
            _on_open: on_open,
            _on_close: on_close,
            _on_message: on_message,
        };
        Ok((channel, events))
    }

    async fn opened(&self) -> Result<(), String> {
        let mut state = self.state.clone();
        match state
            .wait_for(|state| ChannelState::Connecting.ne(state))
            .await
        {
            Ok(settled) => settled.opened(),
            Err(_) => Err(String::from("the data channel closed before it opened")),
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

pub(crate) struct ChannelSend {
    channel: Rc<Channel>,
}

impl ChannelSend {
    pub(crate) fn write(&self, frame: &[u8]) -> Result<(), String> {
        match self.channel.dc.ready_state() {
            RtcDataChannelState::Open => frame
                .chunks(proto::CHUNK_LEN)
                .try_for_each(|chunk| self.channel.dc.send_with_u8_array(chunk).map_err(js)),
            _ => Err(String::from("the data channel closed")),
        }
    }
}

pub(crate) struct ChannelRecv {
    _channel: Rc<Channel>,
    events: mpsc::UnboundedReceiver<ChannelEvent>,
    buffer: FrameBuffer,
}

/// One step of reading a frame from a data channel.
enum ReadStep {
    /// A frame's body, the channel's clean end, or an error.
    Done(ChannelRecv, Result<Option<Vec<u8>>, WireError>),
    /// A message arrived without completing a frame.
    More(ChannelRecv),
}

impl ChannelRecv {
    pub(crate) async fn read_body(self) -> (ChannelRecv, Result<Option<Vec<u8>>, WireError>) {
        let mut recv = self;
        loop {
            match recv.step().await {
                ReadStep::Done(done, body) => break (done, body),
                ReadStep::More(more) => recv = more,
            }
        }
    }

    async fn step(self) -> ReadStep {
        match self.buffer.next_body() {
            Ok(Reassembly::Frame { body, rest }) => ReadStep::Done(
                ChannelRecv {
                    buffer: rest,
                    ..self
                },
                Ok(Some(body)),
            ),
            Ok(Reassembly::Incomplete) => self.receive().await,
            Err(err) => ReadStep::Done(self, Err(err.into())),
        }
    }

    async fn receive(self) -> ReadStep {
        let mut events = self.events;
        match events.recv().await {
            Some(ChannelEvent::Message(message)) => ReadStep::More(ChannelRecv {
                buffer: self.buffer.push(&Uint8Array::new(&message.data()).to_vec()),
                events,
                ..self
            }),
            Some(ChannelEvent::Closed) | None => {
                events.close();
                let body = match self.buffer.remainder() {
                    Remainder::Nothing => Ok(None),
                    Remainder::PartialFrame => Err(WireError::Channel(String::from(
                        "closed in the middle of a frame",
                    ))),
                };
                ReadStep::Done(ChannelRecv { events, ..self }, body)
            }
        }
    }
}
