//! The host's end of the browser's WebRTC upgrade (architecture §5.4).
//!
//! A browser that is already authenticated over iroh sends an SDP offer on an
//! iroh stream. [`accept`] answers it with a str0m [`Rtc`] that runs on its
//! own ephemeral UDP sockets, separate from iroh's, and trickles candidates
//! back on the same stream. Each data channel the browser opens is handed to
//! the caller as a [`Stream`] carrying the same length-prefixed frames as an
//! iroh bi-stream.
//!
//! str0m is sans-IO and does no STUN of its own, so [`Stun`] asks public STUN
//! servers for the sockets' public addresses and adds them as
//! server-reflexive candidates. Nothing relays: there is no TURN.

use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroU16;
use std::ops::{ControlFlow, RangeInclusive};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use iroh::endpoint::{RecvStream, SendStream};
use proto::{CandidateType, FrameBuffer, Reassembly, Remainder, RtcOffer, Signal};
use str0m::change::SdpOffer;
use str0m::channel::ChannelId;
use str0m::net::{DatagramRecv, Protocol, Receive, Transmit};
use str0m::{Candidate, Event, IceConnectionState, Output, Rtc, RtcError};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

use crate::stun::{Answer, Stun};
use crate::wire::{RecvHalf, SendHalf, WireError};

/// ICE and DTLS must finish within this, or the host gives up on the offer.
/// The browser gives up sooner and reports why.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_DATAGRAM: usize = 2048;

/// How the host runs WebRTC upgrades, shared by every session.
#[derive(Debug)]
pub struct RtcOptions {
    /// `stun:host:port` URLs, as in `RTCIceServer.urls`.
    stun_servers: Vec<String>,
    blackhole: Option<Blackhole>,
    /// Test knob: end the first WebRTC session after it has sent this many
    /// frames, as if the connection had dropped.
    drop_after: Option<Count>,
    /// Whether the drop knob has fired yet, in any session.
    drop_knob: watch::Sender<Knob>,
    ports: RtcPorts,
}

/// Which UDP ports the host's WebRTC sockets bind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtcPorts {
    /// Any port the OS picks, a new one for each socket.
    Ephemeral,
    /// The first free port in the range, for each socket.
    Range(PortRange),
}

/// An inclusive range of UDP ports, `first` no higher than `last`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRange {
    first: NonZeroU16,
    last: NonZeroU16,
}

#[derive(Debug, thiserror::Error)]
pub enum PortRangeError {
    #[error("expected <first>-<last>, as in 50000-50099")]
    NoDash,
    #[error("{0:?} is not a port from 1 to 65535")]
    NotAPort(String),
    #[error("the first port, {first}, is above the last, {last}")]
    Reversed { first: u16, last: u16 },
}

impl PortRange {
    /// Parses `<first>-<last>`, as in `50000-50099`.
    pub fn parse(text: &str) -> Result<PortRange, PortRangeError> {
        let (first, last) = text.split_once('-').ok_or(PortRangeError::NoDash)?;
        match (port(first)?, port(last)?) {
            (first, last) if first > last => Err(PortRangeError::Reversed {
                first: first.get(),
                last: last.get(),
            }),
            (first, last) => Ok(PortRange { first, last }),
        }
    }
}

fn port(text: &str) -> Result<NonZeroU16, PortRangeError> {
    text.parse::<NonZeroU16>()
        .map_err(|_| PortRangeError::NotAPort(String::from(text)))
}

impl RtcPorts {
    /// The ports to try for one socket, in order. Port 0 asks the OS.
    fn candidates(self) -> Vec<u16> {
        match self {
            RtcPorts::Ephemeral => Vec::from([0]),
            RtcPorts::Range(PortRange { first, last }) => {
                RangeInclusive::new(first.get(), last.get()).collect()
            }
        }
    }

    fn exhausted(self) -> Unbound {
        match self {
            RtcPorts::Ephemeral => Unbound::NoSocket,
            RtcPorts::Range(PortRange { first, last }) => Unbound::RangeFull {
                first: first.get(),
                last: last.get(),
            },
        }
    }
}

/// Why the host has no socket to answer an offer with.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Unbound {
    #[error("no usable IPv4 interface for WebRTC")]
    NoInterface,
    #[error("no interface would bind a UDP socket")]
    NoSocket,
    #[error("every UDP port in --rtc-ports {first}-{last} is taken")]
    RangeFull { first: u16, last: u16 },
}

/// Test knob: drop every datagram that arrives, so ICE never connects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blackhole {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Knob {
    Armed,
    Spent,
}

/// What a session does after a frame goes out.
enum Fate {
    Live,
    Drop,
}

impl RtcOptions {
    pub fn new(
        stun_servers: Vec<String>,
        blackhole: Option<Blackhole>,
        drop_after: Option<u64>,
        ports: RtcPorts,
    ) -> Self {
        let (drop_knob, _) = watch::channel(Knob::Armed);
        Self {
            stun_servers,
            blackhole,
            drop_after: drop_after.map(Count),
            drop_knob,
            ports,
        }
    }

    pub(crate) fn ports(&self) -> RtcPorts {
        self.ports
    }

    /// Fires the drop knob once per host, in the first session to reach it.
    fn fate(&self, frames_out: Count) -> Fate {
        match self.drop_after {
            Some(after) if after == frames_out => match self.drop_knob.send_replace(Knob::Spent) {
                Knob::Armed => Fate::Drop,
                Knob::Spent => Fate::Live,
            },
            Some(_) | None => Fate::Live,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Count(u64);

impl Count {
    fn next(self) -> Count {
        Count(self.0.saturating_add(1))
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ChannelError {
    #[error("the data channel closed")]
    Closed,
    #[error("the data channel closed in the middle of a frame")]
    Truncated,
}

/// A live WebRTC connection. Cheap to clone.
#[derive(Debug, Clone)]
pub(crate) struct Session {
    /// Never sent on: its sender drops when the driver ends.
    ended: watch::Receiver<()>,
}

impl Session {
    /// Resolves once the connection is gone.
    pub(crate) async fn closed(&self) {
        let mut ended = self.ended.clone();
        let _ = ended.changed().await;
    }
}

/// The sending side of one data channel.
pub(crate) struct ChannelSend {
    id: ChannelId,
    /// Its sender lives while the channel is open for writes.
    open: watch::Receiver<()>,
    out: mpsc::Sender<Out>,
}

impl ChannelSend {
    pub(crate) async fn write(&self, frame: Vec<u8>) -> Result<(), ChannelError> {
        let _ = self.open.has_changed().map_err(|_| ChannelError::Closed)?;
        self.out
            .send(Out::Frame(self.id, frame))
            .await
            .map_err(|_| ChannelError::Closed)
    }

    pub(crate) fn close(&self) {
        let _ = self.out.try_send(Out::Close(self.id));
    }
}

impl Drop for ChannelSend {
    fn drop(&mut self) {
        self.close();
    }
}

/// The receiving side of one data channel.
pub(crate) struct ChannelRecv {
    messages: mpsc::UnboundedReceiver<Vec<u8>>,
    buffer: FrameBuffer,
}

/// One step of reading a frame from a data channel.
enum ReadStep {
    /// A frame's body, the channel's clean end, or an error.
    Done(Result<Option<Vec<u8>>, WireError>),
    /// A message arrived without completing a frame.
    More,
}

impl ChannelRecv {
    /// Cancel-safe: a partly received frame stays in the buffer.
    pub(crate) async fn read_body(&mut self) -> Result<Option<Vec<u8>>, WireError> {
        loop {
            match self.step().await {
                ReadStep::Done(body) => break body,
                ReadStep::More => {}
            }
        }
    }

    async fn step(&mut self) -> ReadStep {
        match self.buffer.next_body() {
            Ok(Reassembly::Frame { body, rest }) => {
                self.buffer = rest;
                ReadStep::Done(Ok(Some(body)))
            }
            Ok(Reassembly::Incomplete) => self.receive().await,
            Err(err) => ReadStep::Done(Err(err.into())),
        }
    }

    async fn receive(&mut self) -> ReadStep {
        match self.messages.recv().await {
            Some(message) => {
                let buffer = std::mem::replace(&mut self.buffer, FrameBuffer::empty());
                self.buffer = buffer.push(&message);
                ReadStep::More
            }
            None => ReadStep::Done(match self.buffer.remainder() {
                Remainder::Nothing => Ok(None),
                Remainder::PartialFrame => Err(ChannelError::Truncated.into()),
            }),
        }
    }
}

/// A data channel the browser opened, ready for the protocol.
pub(crate) struct Stream {
    pub(crate) send: SendHalf,
    pub(crate) recv: RecvHalf,
}

/// What the browser negotiated: the connection, and each data channel it
/// opens.
pub(crate) struct Upgrade {
    pub(crate) session: Session,
    pub(crate) channels: mpsc::Receiver<Stream>,
}

/// What a [`ChannelSend`] asks of the driver.
enum Out {
    Frame(ChannelId, Vec<u8>),
    Close(ChannelId),
}

/// A UDP socket of ours, bound to one interface.
#[derive(Clone)]
pub(crate) struct Socket {
    pub(crate) udp: Arc<UdpSocket>,
    pub(crate) addr: SocketAddr,
}

/// Answers `offer` on `sockets` and starts driving the connection. The
/// signaling stream stays open for trickled candidates and reports; when it
/// closes, the connection ends.
pub(crate) async fn accept(
    offer: RtcOffer,
    sockets: Vec<Socket>,
    options: Arc<RtcOptions>,
    client: String,
    signal_send: SendStream,
    signal_recv: RecvStream,
) -> anyhow::Result<Upgrade> {
    let started = Instant::now();
    let stun = Stun::resolve(&options.stun_servers, &sockets).await;
    let mut rtc = Rtc::builder()
        .set_crypto_provider(Arc::new(str0m::crypto::from_feature_flags()))
        // The browser's fingerprint came over the authenticated iroh stream,
        // so checking it binds this DTLS session to that peer.
        .set_fingerprint_verification(true)
        .build(started);
    let candidates = sockets
        .iter()
        .map(|socket| Candidate::host(socket.addr, "udp"))
        .collect::<Result<Vec<Candidate>, str0m::error::IceError>>()?;
    for candidate in candidates {
        let _ = rtc.add_local_candidate(candidate);
    }
    let offer = SdpOffer::from_sdp_string(&offer.sdp).context("parsing the browser's offer")?;
    let answer = rtc.sdp_api().accept_offer(offer)?;
    tracing::info!(
        client,
        host_candidates = ?sockets.iter().map(|socket| socket.addr).collect::<Vec<SocketAddr>>(),
        stun = ?stun.server_names(),
        blackhole = ?options.blackhole,
        "webrtc offer accepted"
    );
    let mut signal_send = SendHalf::Iroh(signal_send);
    signal_send
        .write(&Signal::Answer {
            sdp: answer.to_sdp_string(),
        })
        .await?;

    let (packets_tx, packets) = mpsc::channel(256);
    let mut readers = JoinSet::new();
    for socket in sockets.iter().cloned() {
        let _ = readers.spawn(read_socket(socket, packets_tx.clone()));
    }
    let (signals_tx, signals) = mpsc::channel(16);
    tokio::spawn(read_signals(RecvHalf::Iroh(signal_recv), signals_tx));
    let (out_tx, out) = mpsc::channel(256);
    let (channels_tx, channels) = mpsc::channel(16);
    let (ended_tx, ended) = watch::channel(());
    let driver = Driver {
        rtc,
        client,
        sockets,
        packets,
        readers,
        signals,
        signal_send,
        stun,
        out,
        out_tx,
        channels: channels_tx,
        inbound: HashMap::new(),
        writable: HashMap::new(),
        pending: VecDeque::new(),
        options,
        phase: Phase::Running,
        started,
        connected_at: None,
        remote: None,
        mapped: HashMap::new(),
        frames_out: Count(0),
        datagrams_in: Count(0),
        datagrams_out: Count(0),
        _ended: ended_tx,
    };
    tokio::spawn(driver.run());
    Ok(Upgrade {
        session: Session { ended },
        channels,
    })
}

/// One socket per usable IPv4 interface, so the destination of every
/// datagram is known without `IP_PKTINFO`. Each takes the first port of
/// `ports` that is free on its interface, so several sessions and several
/// hosts on one machine never collide with each other or with iroh.
pub(crate) fn bind_sockets(ports: RtcPorts) -> Result<Vec<Socket>, Unbound> {
    let ips = interface_ips();
    let _ = ips.first().ok_or(Unbound::NoInterface)?;
    let sockets = ips
        .into_iter()
        .filter_map(|ip| bind(ip, ports))
        .collect::<Vec<Socket>>();
    let _ = sockets.first().ok_or_else(|| ports.exhausted())?;
    Ok(sockets)
}

fn bind(ip: IpAddr, ports: RtcPorts) -> Option<Socket> {
    let bound = ports
        .candidates()
        .into_iter()
        .find_map(|port| bind_port(SocketAddr::new(ip, port)));
    match bound {
        Some(socket) => Some(socket),
        None => {
            tracing::warn!(%ip, ?ports, "webrtc could not bind a socket");
            None
        }
    }
}

/// Synchronous, since a UDP bind never waits on the network. tokio needs
/// the socket in non-blocking mode.
fn bind_port(addr: SocketAddr) -> Option<Socket> {
    let bound = std::net::UdpSocket::bind(addr).and_then(|udp| {
        udp.set_nonblocking(true)?;
        let udp = UdpSocket::from_std(udp)?;
        Ok(Socket {
            addr: udp.local_addr()?,
            udp: Arc::new(udp),
        })
    });
    match bound {
        Ok(socket) => Some(socket),
        Err(err) => {
            tracing::debug!(%addr, %err, "webrtc could not bind a port");
            None
        }
    }
}

fn interface_ips() -> Vec<IpAddr> {
    netdev::get_interfaces()
        .into_iter()
        .filter(|i| i.is_up() && !i.is_loopback())
        .flat_map(|i| i.ipv4.into_iter().map(|net| net.addr()))
        .filter(|ip| !ip.is_link_local() && !ip.is_unspecified())
        .map(IpAddr::V4)
        .collect()
}

/// A datagram that arrived on one of our sockets.
struct Packet {
    source: SocketAddr,
    destination: SocketAddr,
    bytes: Vec<u8>,
}

/// The receiving end of a reader task's channel closed.
#[derive(Debug)]
struct Stopped {}

impl Socket {
    async fn receive(&self) -> std::io::Result<Packet> {
        let mut bytes = Vec::with_capacity(MAX_DATAGRAM);
        let (_, source) = self.udp.recv_buf_from(&mut bytes).await?;
        Ok(Packet {
            source,
            destination: self.addr,
            bytes,
        })
    }
}

async fn read_socket(socket: Socket, packets: mpsc::Sender<Packet>) -> Result<Infallible, Stopped> {
    loop {
        match socket.receive().await {
            Ok(packet) => packets.send(packet).await.map_err(|_| Stopped {})?,
            Err(err) => tracing::debug!(%err, "webrtc socket read failed"),
        }
    }
}

async fn read_signals(
    mut recv: RecvHalf,
    signals: mpsc::Sender<Signal>,
) -> Result<Infallible, Stopped> {
    loop {
        match recv.read::<Signal>().await {
            Ok(Some(signal)) => signals.send(signal).await.map_err(|_| Stopped {})?,
            Ok(None) => break Err(Stopped {}),
            Err(err) => {
                tracing::debug!(%err, "webrtc signaling stream ended");
                break Err(Stopped {});
            }
        }
    }
}

/// Why a WebRTC session ended.
#[derive(Debug, thiserror::Error)]
enum Ended {
    #[error("str0m failed: {0}")]
    Str0m(#[from] RtcError),
    #[error("the connection closed")]
    Closed,
    #[error(
        "ICE did not connect within {}s (remote address seen: {remote:?})",
        CONNECT_TIMEOUT.as_secs()
    )]
    NotConnected { remote: Option<SocketAddr> },
    #[error("the sockets closed")]
    SocketsClosed,
    #[error("the client closed the signaling stream")]
    SignalingClosed,
    #[error("test knob: dropped")]
    Dropped,
}

/// What arrived while str0m waited.
enum Input {
    Packet(Option<Packet>),
    Timer,
    Out(Option<Out>),
    Signal(Option<Signal>),
}

/// Whether the session runs, or drains str0m after the drop knob fired.
#[derive(Debug, Clone, Copy)]
enum Phase {
    Running,
    Dropping,
}

/// A write the SCTP send buffer had no room for yet.
struct Write {
    id: ChannelId,
    bytes: Vec<u8>,
}

/// Where a flush stopped: how many writes it took off the queue, and the
/// error that stopped it, if one did.
struct Stop {
    done: usize,
    error: Option<RtcError>,
}

struct Driver {
    rtc: Rtc,
    client: String,
    sockets: Vec<Socket>,
    packets: mpsc::Receiver<Packet>,
    /// The tasks reading `sockets`. Each holds its socket open.
    readers: JoinSet<Result<Infallible, Stopped>>,
    signals: mpsc::Receiver<Signal>,
    stun: Stun,
    out: mpsc::Receiver<Out>,
    out_tx: mpsc::Sender<Out>,
    channels: mpsc::Sender<Stream>,
    /// Every channel from when it opens until it closes.
    inbound: HashMap<ChannelId, mpsc::UnboundedSender<Vec<u8>>>,
    /// The channels still open for writes. Dropping one fails its writes.
    writable: HashMap<ChannelId, watch::Sender<()>>,
    pending: VecDeque<Write>,
    options: Arc<RtcOptions>,
    phase: Phase,
    started: Instant,
    connected_at: Option<Instant>,
    /// Where the browser's datagrams last came from.
    remote: Option<SocketAddr>,
    /// What STUN said each socket maps to.
    mapped: HashMap<SocketAddr, SocketAddr>,
    frames_out: Count,
    datagrams_in: Count,
    datagrams_out: Count,
    /// After every field that holds a socket, so the ports are free by the
    /// time dropping it closes the signaling stream.
    signal_send: SendHalf,
    /// Dropped when the driver ends, which resolves [`Session::closed`].
    _ended: watch::Sender<()>,
}

impl Driver {
    async fn run(mut self) {
        loop {
            match self.turn().await {
                Ok(()) => {}
                Err(ended) => break self.end(ended).await,
            }
        }
    }

    /// Frees the sockets' ports before the signaling stream closes, so a
    /// client that retries at once finds them free.
    async fn end(&mut self, ended: Ended) {
        self.writable.clear();
        self.readers.shutdown().await;
        tracing::info!(
            client = self.client,
            reason = ended.to_string(),
            lasted_ms = self.started.elapsed().as_millis(),
            frames_out = self.frames_out.0,
            datagrams_in = self.datagrams_in.0,
            datagrams_out = self.datagrams_out.0,
            connected = self.connected_at.is_some(),
            last_remote = ?self.remote,
            "webrtc session ended"
        );
    }

    /// Handles str0m's next output, or, once str0m has only a timeout left,
    /// what arrives while it waits.
    async fn turn(&mut self) -> Result<(), Ended> {
        match (self.rtc.poll_output(), self.phase) {
            (Ok(Output::Transmit(transmit)), _) => {
                self.transmit(transmit).await;
                Ok(())
            }
            (Ok(Output::Event(event)), _) => {
                self.event(event);
                Ok(())
            }
            (Ok(Output::Timeout(deadline)), Phase::Running) => {
                let input = self.wait(deadline).await?;
                self.input(input).await?;
                self.timeout(deadline)
            }
            (Ok(Output::Timeout(_)) | Err(_), Phase::Dropping) => Err(Ended::Dropped),
            (Err(err), Phase::Running) => Err(Ended::Str0m(err)),
        }
    }

    async fn wait(&mut self, deadline: Instant) -> Result<Input, Ended> {
        self.rtc.is_alive().then_some(()).ok_or(Ended::Closed)?;
        self.connecting_in_time()?;
        let wake = self
            .stun
            .next_due(self.started)
            .map_or(deadline, |due| due.min(deadline));
        let input = tokio::select! {
            packet = self.packets.recv() => Input::Packet(packet),
            () = tokio::time::sleep_until(wake.into()) => Input::Timer,
            out = self.out.recv() => Input::Out(out),
            signal = self.signals.recv() => Input::Signal(signal),
        };
        Ok(input)
    }

    fn connecting_in_time(&self) -> Result<(), Ended> {
        match self.connected_at {
            Some(_) => Ok(()),
            None => match self.started.elapsed().cmp(&CONNECT_TIMEOUT) {
                Ordering::Greater => Err(Ended::NotConnected {
                    remote: self.remote,
                }),
                Ordering::Less | Ordering::Equal => Ok(()),
            },
        }
    }

    async fn input(&mut self, input: Input) -> Result<(), Ended> {
        match input {
            Input::Packet(Some(packet)) => Ok(self.packet(packet).await?),
            Input::Packet(None) => Err(Ended::SocketsClosed),
            Input::Timer => {
                self.stun.send_due(self.started).await;
                Ok(())
            }
            Input::Out(Some(Out::Frame(id, frame))) => self.frame(id, frame),
            Input::Out(Some(Out::Close(id))) => {
                self.close(id);
                Ok(())
            }
            Input::Out(None) => Ok(()),
            Input::Signal(Some(signal)) => {
                self.signal(signal);
                Ok(())
            }
            Input::Signal(None) => Err(Ended::SignalingClosed),
        }
    }

    fn timeout(&mut self, deadline: Instant) -> Result<(), Ended> {
        let now = Instant::now();
        match now.cmp(&deadline) {
            Ordering::Less => Ok(()),
            Ordering::Equal | Ordering::Greater => {
                Ok(self.rtc.handle_input(str0m::Input::Timeout(now))?)
            }
        }
    }

    async fn transmit(&mut self, transmit: Transmit) {
        let socket = self
            .sockets
            .iter()
            .find(|socket| socket.addr == transmit.source);
        if let Some(socket) = socket {
            self.datagrams_out = self.datagrams_out.next();
            let sent = socket
                .udp
                .send_to(&transmit.contents, transmit.destination)
                .await;
            if let Err(err) = sent {
                tracing::debug!(%err, to = %transmit.destination, "webrtc send failed");
            }
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::IceConnectionStateChange(state) => {
                tracing::info!(client = self.client, ?state, "webrtc ice state");
                if let IceConnectionState::Disconnected = state {
                    self.rtc.disconnect();
                }
            }
            Event::Connected => {
                let now = Instant::now();
                self.connected_at = Some(now);
                tracing::info!(
                    client = self.client,
                    after_ms = now.duration_since(self.started).as_millis(),
                    remote = ?self.remote,
                    "webrtc connected: DTLS is up"
                );
            }
            Event::ChannelOpen(id, label) => self.open_channel(id, &label),
            Event::ChannelData(data) => {
                if let Some(messages) = self.inbound.get(&data.id) {
                    let _ = messages.send(data.data);
                }
            }
            Event::ChannelClose(id) => {
                let _ = self.inbound.remove(&id);
                let _ = self.writable.remove(&id);
                self.pending.retain(|write| write.id != id);
            }
            Event::ChannelBufferedAmountLow(_) => {
                if let Err(err) = self.flush() {
                    tracing::debug!(%err, "webrtc flush failed");
                }
            }
            _ => {}
        }
    }

    fn open_channel(&mut self, id: ChannelId, label: &str) {
        let (messages_tx, messages) = mpsc::unbounded_channel();
        let (open_tx, open) = watch::channel(());
        let _ = self.inbound.insert(id, messages_tx);
        let _ = self.writable.insert(id, open_tx);
        let stream = Stream {
            send: SendHalf::WebRtc(ChannelSend {
                id,
                open,
                out: self.out_tx.clone(),
            }),
            recv: RecvHalf::WebRtc(ChannelRecv {
                messages,
                buffer: FrameBuffer::empty(),
            }),
        };
        tracing::debug!(label, "webrtc data channel opened");
        if let Err(err) = self.channels.try_send(stream) {
            tracing::warn!(%err, "webrtc data channel refused: too many opening at once");
        }
    }

    /// Closes our side once; later closes of the same channel do nothing.
    fn close(&mut self, id: ChannelId) {
        if let Some(open) = self.writable.remove(&id) {
            drop(open);
            self.rtc.direct_api().close_data_channel(id);
        }
    }

    async fn packet(&mut self, packet: Packet) -> Result<(), RtcError> {
        match self.options.blackhole {
            Some(Blackhole {}) => Ok(()),
            None => match self.stun.answer(packet.source, &packet.bytes) {
                Some(answer) => {
                    self.add_srflx(answer, packet.destination).await;
                    Ok(())
                }
                None => self.datagram(&packet),
            },
        }
    }

    fn datagram(&mut self, packet: &Packet) -> Result<(), RtcError> {
        match DatagramRecv::try_from(packet.bytes.as_slice()) {
            Ok(contents) => self.receive(packet, contents),
            Err(_) => Ok(()),
        }
    }

    fn receive(&mut self, packet: &Packet, contents: DatagramRecv) -> Result<(), RtcError> {
        let input = str0m::Input::Receive(
            Instant::now(),
            Receive {
                proto: Protocol::Udp,
                source: packet.source,
                destination: packet.destination,
                contents,
            },
        );
        match self.rtc.accepts(&input).then_some(()) {
            Some(()) => {
                if let Count(0) = self.datagrams_in {
                    tracing::info!(client = self.client, from = %packet.source, "webrtc datagrams arriving");
                }
                self.datagrams_in = self.datagrams_in.next();
                self.remote = Some(packet.source);
                self.rtc.handle_input(input)
            }
            None => Ok(()),
        }
    }

    async fn add_srflx(&mut self, answer: Answer, base: SocketAddr) {
        let Answer { server, mapped } = answer;
        tracing::info!(client = self.client, server, %mapped, %base, "webrtc STUN answer");
        let earlier = self
            .mapped
            .insert(base, mapped)
            .filter(|earlier| earlier != &mapped);
        if let Some(earlier) = earlier {
            tracing::warn!(
                client = self.client,
                %base,
                %earlier,
                %mapped,
                "webrtc STUN servers saw different public addresses for one socket: \
                 the NAT maps per destination, so a peer on another network can't use \
                 the srflx candidate"
            );
        }
        if let Ok(candidate) = Candidate::server_reflexive(mapped, base, "udp") {
            self.trickle(candidate, mapped, base).await;
        }
    }

    async fn trickle(&mut self, candidate: Candidate, mapped: SocketAddr, base: SocketAddr) {
        let text = candidate.to_sdp_string();
        if let Some(Candidate { .. }) = self.rtc.add_local_candidate(candidate) {
            tracing::info!(client = self.client, %mapped, %base, "webrtc local candidate srflx");
            let signal = Signal::Candidate { candidate: text };
            if let Err(err) = self.signal_send.write(&signal).await {
                tracing::debug!(%err, "webrtc could not trickle a candidate");
            }
        }
    }

    fn signal(&mut self, signal: Signal) {
        match signal {
            Signal::Candidate { candidate } => self.remote_candidate(&candidate),
            Signal::Selected { local, remote } => tracing::info!(
                client = self.client,
                browser_side = local.as_str(),
                host_side = remote.as_str(),
                "webrtc selected candidate pair, as the browser reports it"
            ),
            Signal::Failed { reason } => {
                tracing::warn!(client = self.client, reason, "webrtc client gave up");
            }
            Signal::Answer { .. } => {}
        }
    }

    fn remote_candidate(&mut self, candidate: &str) {
        let text = candidate.strip_prefix("a=").unwrap_or(candidate);
        let kind = CandidateType::of_candidate(text);
        match Candidate::from_sdp_string(text) {
            Ok(parsed) => {
                tracing::info!(client = self.client, kind = ?kind, addr = %parsed.addr(), "webrtc remote candidate");
                self.rtc.add_remote_candidate(parsed);
            }
            // Browsers hide interface addresses behind mDNS names. Their
            // checks still arrive and str0m learns them as prflx.
            Err(err) => {
                tracing::info!(client = self.client, kind = ?kind, %err, candidate = text, "webrtc remote candidate skipped");
            }
        }
    }

    fn frame(&mut self, id: ChannelId, frame: Vec<u8>) -> Result<(), Ended> {
        self.pending
            .extend(frame.chunks(proto::CHUNK_LEN).map(|chunk| Write {
                id,
                bytes: chunk.to_vec(),
            }));
        self.flush()?;
        self.frames_out = self.frames_out.next();
        if let Fate::Drop = self.options.fate(self.frames_out) {
            self.kill();
        }
        Ok(())
    }

    /// Test knob: resets every channel so the browser sees the drop at once
    /// rather than when ICE consent expires, then drains str0m and ends.
    fn kill(&mut self) {
        tracing::warn!(
            frames = self.frames_out.0,
            "webrtc test knob: dropping the connection"
        );
        let ids = self.inbound.keys().copied().collect::<Vec<ChannelId>>();
        for id in ids {
            self.rtc.direct_api().close_data_channel(id);
        }
        self.phase = Phase::Dropping;
    }

    /// Writes pending chunks in order until str0m's send buffer is full. A
    /// chunk for a channel that has gone is dropped.
    fn flush(&mut self) -> Result<(), RtcError> {
        let flow = self.pending.iter().try_fold(0, |done, write| {
            let written = self
                .rtc
                .channel(write.id)
                .map(|mut channel| channel.write(true, &write.bytes))
                .map(|whole| whole.map(|whole| whole.then_some(())));
            flushed(done, written)
        });
        let stop = match flow {
            ControlFlow::Continue(done) => Stop { done, error: None },
            ControlFlow::Break(stop) => stop,
        };
        self.pending = self.pending.split_off(stop.done);
        stop.error.map_or(Ok(()), Err)
    }
}

/// Counts a chunk that str0m took whole, or whose channel has gone. Stops at
/// one that doesn't fit, or at an error.
fn flushed(done: usize, written: Option<Result<Option<()>, RtcError>>) -> ControlFlow<Stop, usize> {
    match written {
        None | Some(Ok(Some(()))) => ControlFlow::Continue(done.saturating_add(1)),
        Some(Ok(None)) => ControlFlow::Break(Stop { done, error: None }),
        Some(Err(error)) => ControlFlow::Break(Stop {
            done,
            error: Some(error),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::PortRange;

    fn parsed(text: &str) -> String {
        match PortRange::parse(text) {
            Ok(PortRange { first, last }) => format!("{first}..={last}"),
            Err(err) => err.to_string(),
        }
    }

    #[test]
    fn a_port_range_is_two_ports_in_order_with_no_zero() {
        assert_eq!(
            [
                "50000-50099",
                "7-7",
                "50099-50000",
                "0-10",
                "1-65536",
                "50000",
                "a-b"
            ]
            .into_iter()
            .map(parsed)
            .collect::<Vec<String>>(),
            Vec::from([
                String::from("50000..=50099"),
                String::from("7..=7"),
                String::from("the first port, 50099, is above the last, 50000"),
                String::from("\"0\" is not a port from 1 to 65535"),
                String::from("\"65536\" is not a port from 1 to 65535"),
                String::from("expected <first>-<last>, as in 50000-50099"),
                String::from("\"a\" is not a port from 1 to 65535"),
            ])
        );
    }
}
