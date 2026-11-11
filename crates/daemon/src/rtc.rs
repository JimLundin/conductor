//! The host's end of the browser's WebRTC upgrade (architecture §5.4).
//!
//! A browser that is already authenticated over iroh sends an SDP offer on an
//! iroh stream. [`accept`] answers it with a str0m [`Rtc`] that runs on its
//! own ephemeral UDP sockets, separate from iroh's, and trickles candidates
//! back on the same stream. Each data channel the browser opens is handed to
//! the caller as a [`ChannelSend`] and [`ChannelRecv`] pair carrying the same
//! length-prefixed frames as an iroh bi-stream.
//!
//! str0m is sans-IO and does no STUN of its own, so [`Stun`] asks public STUN
//! servers for the sockets' public addresses and adds them as
//! server-reflexive candidates. Nothing relays: there is no TURN. str0m's
//! STUN parser only accepts ICE messages, which carry MESSAGE-INTEGRITY, so
//! the plain RFC 8489 binding exchange is encoded here.

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use iroh::endpoint::{RecvStream, SendStream};
use proto::{CandidateType, RtcOffer, Signal};
use str0m::change::SdpOffer;
use str0m::channel::ChannelId;
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, watch};

use crate::wire::{RecvHalf, SendHalf};

/// ICE and DTLS must finish within this, or the host gives up on the offer.
/// The browser gives up sooner and reports why.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const STUN_RESOLVE_TIMEOUT: Duration = Duration::from_secs(2);
/// When to send each STUN binding request, from the start of the session.
const STUN_SCHEDULE: [Duration; 4] = [
    Duration::ZERO,
    Duration::from_millis(250),
    Duration::from_millis(750),
    Duration::from_millis(1750),
];
const STUN_GIVE_UP: Duration = Duration::from_secs(3);
const MAX_DATAGRAM: usize = 2048;

#[derive(Debug, Default)]
pub struct RtcOptions {
    /// `stun:host:port` URLs, as in `RTCIceServer.urls`.
    pub stun_servers: Vec<String>,
    /// Test knob: drop every datagram that arrives, so ICE never connects.
    pub test_blackhole: bool,
    /// Test knob: end the first WebRTC session after it has sent this many
    /// frames, as if the connection had dropped.
    pub test_drop_after: Option<u64>,
    dropped_once: AtomicBool,
}

impl RtcOptions {
    pub fn new(stun_servers: Vec<String>) -> Self {
        Self {
            stun_servers,
            ..Self::default()
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("the data channel closed")]
    Closed,
    #[error("the data channel closed in the middle of a frame")]
    Truncated,
}

/// A live WebRTC connection. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Session {
    ended: watch::Receiver<()>,
}

impl Session {
    /// Resolves once the connection is gone.
    pub async fn closed(&self) {
        let mut ended = self.ended.clone();
        while ended.changed().await.is_ok() {}
    }
}

pub struct ChannelSend {
    id: ChannelId,
    open: Arc<AtomicBool>,
    out: mpsc::Sender<Out>,
    session: Arc<Counters>,
}

impl ChannelSend {
    pub async fn write(&mut self, frame: Vec<u8>) -> Result<(), ChannelError> {
        if !self.open.load(Ordering::Relaxed) {
            return Err(ChannelError::Closed);
        }
        for chunk in frame.chunks(proto::CHUNK_LEN) {
            self.out
                .send(Out::Write(self.id, chunk.to_vec()))
                .await
                .map_err(|_| ChannelError::Closed)?;
        }
        self.session.frame_sent(&self.out);
        Ok(())
    }

    pub fn close(&mut self) {
        if self.open.swap(false, Ordering::Relaxed) {
            self.out.try_send(Out::Close(self.id)).ok();
        }
    }
}

impl Drop for ChannelSend {
    fn drop(&mut self) {
        self.close();
    }
}

pub struct ChannelRecv {
    messages: mpsc::UnboundedReceiver<Vec<u8>>,
    buffer: proto::FrameBuffer,
}

impl ChannelRecv {
    /// Cancel-safe: a partly received frame stays in the buffer.
    pub async fn read_body(&mut self) -> Result<Option<Vec<u8>>, crate::wire::WireError> {
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
                None => return Err(ChannelError::Truncated.into()),
            }
        }
    }
}

/// What the browser negotiated: the connection, and each data channel it
/// opens, as a stream ready for the protocol.
pub struct Upgrade {
    pub session: Session,
    pub channels: mpsc::Receiver<(SendHalf, RecvHalf)>,
}

enum Out {
    Write(ChannelId, Vec<u8>),
    Close(ChannelId),
    /// Test knob: drop the whole connection.
    Kill,
}

struct Counters {
    options: Arc<RtcOptions>,
    frames_out: AtomicU64,
}

impl Counters {
    fn frame_sent(&self, out: &mpsc::Sender<Out>) {
        let sent = self.frames_out.fetch_add(1, Ordering::Relaxed) + 1;
        if self.options.test_drop_after == Some(sent)
            && !self.options.dropped_once.swap(true, Ordering::Relaxed)
        {
            tracing::warn!(frames = sent, "webrtc test knob: dropping the connection");
            out.try_send(Out::Kill).ok();
        }
    }
}

/// Answers `offer` and starts driving the connection. The signaling stream
/// stays open for trickled candidates and reports; when it closes, the
/// connection ends.
pub async fn accept(
    offer: RtcOffer,
    options: Arc<RtcOptions>,
    client: String,
    signal_send: SendStream,
    signal_recv: RecvStream,
) -> anyhow::Result<Upgrade> {
    let started = Instant::now();
    let sockets = bind_sockets().await?;
    let stun = Stun::resolve(&options.stun_servers, sockets.len()).await;
    let mut rtc = Rtc::builder()
        .set_crypto_provider(Arc::new(str0m::crypto::from_feature_flags()))
        // The browser's fingerprint came over the authenticated iroh stream,
        // so checking it binds this DTLS session to that peer.
        .set_fingerprint_verification(true)
        .build(started);
    for socket in &sockets {
        let addr = socket.local_addr()?;
        rtc.add_local_candidate(Candidate::host(addr, "udp")?);
    }
    let offer = SdpOffer::from_sdp_string(&offer.sdp).context("parsing the browser's offer")?;
    let answer = rtc.sdp_api().accept_offer(offer)?;
    let local: Vec<SocketAddr> = sockets.iter().filter_map(|s| s.local_addr().ok()).collect();
    tracing::info!(
        client,
        host_candidates = ?local,
        stun = ?stun.server_names(),
        blackhole = options.test_blackhole,
        "webrtc offer accepted"
    );
    let mut signal_send = SendHalf::Iroh(signal_send);
    signal_send
        .write(&Signal::Answer {
            sdp: answer.to_sdp_string(),
        })
        .await?;

    let (packets_tx, packets) = mpsc::channel(256);
    for socket in &sockets {
        tokio::spawn(read_socket(socket.clone(), packets_tx.clone()));
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
        signals,
        signal_send,
        stun,
        out,
        out_tx,
        channels: channels_tx,
        inbound: HashMap::new(),
        open: HashMap::new(),
        pending: VecDeque::new(),
        counters: Arc::new(Counters {
            options,
            frames_out: AtomicU64::new(0),
        }),
        started,
        connected_at: None,
        remote: None,
        mapped: HashMap::new(),
        datagrams_in: 0,
        datagrams_out: 0,
        _ended: ended_tx,
    };
    tokio::spawn(driver.run());
    Ok(Upgrade {
        session: Session { ended },
        channels,
    })
}

/// One socket per usable IPv4 interface, each on an ephemeral port, so the
/// destination of every datagram is known without `IP_PKTINFO`, and several
/// hosts on one machine never collide with each other or with iroh.
async fn bind_sockets() -> anyhow::Result<Vec<Arc<UdpSocket>>> {
    let mut sockets = Vec::new();
    for ip in interface_ips() {
        match UdpSocket::bind(SocketAddr::new(ip, 0)).await {
            Ok(socket) => sockets.push(Arc::new(socket)),
            Err(err) => tracing::warn!(%ip, %err, "webrtc could not bind a socket"),
        }
    }
    anyhow::ensure!(!sockets.is_empty(), "no usable IPv4 interface for WebRTC");
    Ok(sockets)
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

struct Packet {
    source: SocketAddr,
    destination: SocketAddr,
    bytes: Vec<u8>,
}

async fn read_socket(socket: Arc<UdpSocket>, packets: mpsc::Sender<Packet>) {
    let Ok(destination) = socket.local_addr() else {
        return;
    };
    let mut buf = vec![0u8; MAX_DATAGRAM];
    loop {
        let (len, source) = match socket.recv_from(&mut buf).await {
            Ok(received) => received,
            Err(err) => {
                tracing::debug!(%err, "webrtc socket read failed");
                continue;
            }
        };
        let packet = Packet {
            source,
            destination,
            bytes: buf[..len].to_vec(),
        };
        if packets.send(packet).await.is_err() {
            return;
        }
    }
}

async fn read_signals(mut recv: RecvHalf, signals: mpsc::Sender<Signal>) {
    loop {
        match recv.read::<Signal>().await {
            Ok(Some(signal)) => {
                if signals.send(signal).await.is_err() {
                    return;
                }
            }
            Ok(None) => return,
            Err(err) => {
                tracing::debug!(%err, "webrtc signaling stream ended");
                return;
            }
        }
    }
}

struct Driver {
    rtc: Rtc,
    client: String,
    sockets: Vec<Arc<UdpSocket>>,
    packets: mpsc::Receiver<Packet>,
    signals: mpsc::Receiver<Signal>,
    signal_send: SendHalf,
    stun: Stun,
    out: mpsc::Receiver<Out>,
    out_tx: mpsc::Sender<Out>,
    channels: mpsc::Sender<(SendHalf, RecvHalf)>,
    inbound: HashMap<ChannelId, mpsc::UnboundedSender<Vec<u8>>>,
    open: HashMap<ChannelId, Arc<AtomicBool>>,
    /// Writes the SCTP send buffer had no room for yet.
    pending: VecDeque<(ChannelId, Vec<u8>)>,
    counters: Arc<Counters>,
    started: Instant,
    connected_at: Option<Instant>,
    /// Where the browser's datagrams last came from.
    remote: Option<SocketAddr>,
    /// What STUN said each socket maps to.
    mapped: HashMap<SocketAddr, SocketAddr>,
    datagrams_in: u64,
    datagrams_out: u64,
    /// Dropped when the driver ends, which resolves [`Session::closed`].
    _ended: watch::Sender<()>,
}

impl Driver {
    async fn run(mut self) {
        let reason = self.drive().await;
        for open in self.open.values() {
            open.store(false, Ordering::Relaxed);
        }
        let now = Instant::now();
        tracing::info!(
            client = self.client,
            reason,
            lasted_ms = now.duration_since(self.started).as_millis() as u64,
            frames_out = self.counters.frames_out.load(Ordering::Relaxed),
            datagrams_in = self.datagrams_in,
            datagrams_out = self.datagrams_out,
            connected = self.connected_at.is_some(),
            last_remote = ?self.remote,
            "webrtc session ended"
        );
    }

    async fn drive(&mut self) -> String {
        loop {
            let deadline = match self.poll().await {
                Ok(deadline) => deadline,
                Err(err) => return format!("str0m failed: {err}"),
            };
            if !self.rtc.is_alive() {
                return "the connection closed".into();
            }
            if self.connected_at.is_none() && self.started.elapsed() > CONNECT_TIMEOUT {
                return format!(
                    "ICE did not connect within {}s (remote address seen: {:?})",
                    CONNECT_TIMEOUT.as_secs(),
                    self.remote
                );
            }
            let stun_due = self.stun.next_due(self.started);
            let wake = stun_due.map_or(deadline, |due| due.min(deadline));
            let input = tokio::select! {
                packet = self.packets.recv() => {
                    let Some(packet) = packet else { return "the sockets closed".into() };
                    self.packet(packet).await
                }
                () = tokio::time::sleep_until(wake.into()) => {
                    self.stun.send_due(self.started, &self.sockets).await;
                    Ok(())
                }
                out = self.out.recv() => match out {
                    Some(Out::Write(id, bytes)) => {
                        self.pending.push_back((id, bytes));
                        self.flush()
                    }
                    Some(Out::Close(id)) => {
                        self.rtc.direct_api().close_data_channel(id);
                        Ok(())
                    }
                    Some(Out::Kill) => {
                        // Reset every channel so the browser sees the drop at
                        // once rather than when ICE consent expires.
                        let ids: Vec<ChannelId> = self.open.keys().copied().collect();
                        for id in ids {
                            self.rtc.direct_api().close_data_channel(id);
                        }
                        self.poll().await.ok();
                        return "test knob: dropped".into();
                    }
                    None => Ok(()),
                },
                signal = self.signals.recv() => match signal {
                    Some(signal) => self.signal(signal),
                    None => return "the client closed the signaling stream".into(),
                },
            };
            if let Err(err) = input {
                return format!("str0m failed: {err}");
            }
            if Instant::now() >= deadline
                && let Err(err) = self.rtc.handle_input(Input::Timeout(Instant::now()))
            {
                return format!("str0m failed: {err}");
            }
        }
    }

    /// Drains str0m until it only has a timeout left, which it returns.
    async fn poll(&mut self) -> Result<Instant, str0m::RtcError> {
        loop {
            match self.rtc.poll_output()? {
                Output::Timeout(deadline) => return Ok(deadline),
                Output::Transmit(transmit) => {
                    if let Some(socket) = self
                        .sockets
                        .iter()
                        .find(|s| s.local_addr().ok() == Some(transmit.source))
                    {
                        self.datagrams_out += 1;
                        if let Err(err) = socket
                            .send_to(&transmit.contents, transmit.destination)
                            .await
                        {
                            tracing::debug!(%err, to = %transmit.destination, "webrtc send failed");
                        }
                    }
                }
                Output::Event(event) => self.event(event).await,
            }
        }
    }

    async fn event(&mut self, event: Event) {
        match event {
            Event::IceConnectionStateChange(state) => {
                tracing::info!(client = self.client, ?state, "webrtc ice state");
                if state == IceConnectionState::Disconnected {
                    self.rtc.disconnect();
                }
            }
            Event::Connected => {
                let now = Instant::now();
                self.connected_at = Some(now);
                tracing::info!(
                    client = self.client,
                    after_ms = now.duration_since(self.started).as_millis() as u64,
                    remote = ?self.remote,
                    "webrtc connected: DTLS is up"
                );
            }
            Event::ChannelOpen(id, label) => {
                let (messages_tx, messages) = mpsc::unbounded_channel();
                let open = Arc::new(AtomicBool::new(true));
                self.inbound.insert(id, messages_tx);
                self.open.insert(id, open.clone());
                let send = ChannelSend {
                    id,
                    open,
                    out: self.out_tx.clone(),
                    session: self.counters.clone(),
                };
                let recv = ChannelRecv {
                    messages,
                    buffer: proto::FrameBuffer::empty(),
                };
                tracing::debug!(label, "webrtc data channel opened");
                if self
                    .channels
                    .try_send((SendHalf::WebRtc(send), RecvHalf::WebRtc(recv)))
                    .is_err()
                {
                    tracing::warn!("webrtc data channel refused: too many opening at once");
                }
            }
            Event::ChannelData(data) => {
                if let Some(messages) = self.inbound.get(&data.id) {
                    messages.send(data.data).ok();
                }
            }
            Event::ChannelClose(id) => {
                self.inbound.remove(&id);
                if let Some(open) = self.open.remove(&id) {
                    open.store(false, Ordering::Relaxed);
                }
                self.pending.retain(|(pending, _)| *pending != id);
            }
            Event::ChannelBufferedAmountLow(_) => {
                if let Err(err) = self.flush() {
                    tracing::debug!(%err, "webrtc flush failed");
                }
            }
            _ => {}
        }
    }

    async fn packet(&mut self, packet: Packet) -> Result<(), str0m::RtcError> {
        if self.counters.options.test_blackhole {
            return Ok(());
        }
        if let Some((server, mapped)) = self.stun.answer(&packet) {
            self.add_srflx(server, mapped, packet.destination).await;
            return Ok(());
        }
        let Ok(contents) = packet.bytes.as_slice().try_into() else {
            return Ok(());
        };
        let input = Input::Receive(
            Instant::now(),
            Receive {
                proto: Protocol::Udp,
                source: packet.source,
                destination: packet.destination,
                contents,
            },
        );
        if !self.rtc.accepts(&input) {
            return Ok(());
        }
        self.datagrams_in += 1;
        if self.remote.is_none() {
            tracing::info!(client = self.client, from = %packet.source, "webrtc datagrams arriving");
        }
        self.remote = Some(packet.source);
        self.rtc.handle_input(input)
    }

    async fn add_srflx(&mut self, server: String, mapped: SocketAddr, base: SocketAddr) {
        tracing::info!(client = self.client, server, %mapped, %base, "webrtc STUN answer");
        match self.mapped.insert(base, mapped) {
            Some(earlier) if earlier != mapped => tracing::warn!(
                client = self.client,
                %base,
                %earlier,
                %mapped,
                "webrtc STUN servers saw different public addresses for one socket: \
                 the NAT maps per destination, so a peer on another network can't use \
                 the srflx candidate"
            ),
            _ => {}
        }
        let Ok(candidate) = Candidate::server_reflexive(mapped, base, "udp") else {
            return;
        };
        let text = candidate.to_sdp_string();
        if self.rtc.add_local_candidate(candidate).is_none() {
            return;
        }
        tracing::info!(client = self.client, %mapped, %base, "webrtc local candidate srflx");
        let signal = Signal::Candidate { candidate: text };
        if let Err(err) = self.signal_send.write(&signal).await {
            tracing::debug!(%err, "webrtc could not trickle a candidate");
        }
    }

    fn signal(&mut self, signal: Signal) -> Result<(), str0m::RtcError> {
        match signal {
            Signal::Candidate { candidate } => {
                let text = candidate.strip_prefix("a=").unwrap_or(&candidate);
                let kind = CandidateType::of_candidate(text);
                match Candidate::from_sdp_string(text) {
                    Ok(parsed) => {
                        tracing::info!(client = self.client, kind = ?kind, addr = %parsed.addr(), "webrtc remote candidate");
                        self.rtc.add_remote_candidate(parsed);
                    }
                    // Browsers hide interface addresses behind mDNS names. Their
                    // checks still arrive and str0m learns them as prflx.
                    Err(err) => {
                        tracing::info!(client = self.client, kind = ?kind, %err, candidate = text, "webrtc remote candidate skipped")
                    }
                }
            }
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
        Ok(())
    }

    fn flush(&mut self) -> Result<(), str0m::RtcError> {
        while let Some((id, bytes)) = self.pending.front() {
            let Some(mut channel) = self.rtc.channel(*id) else {
                self.pending.pop_front();
                continue;
            };
            if !channel.write(true, bytes)? {
                return Ok(());
            }
            self.pending.pop_front();
        }
        Ok(())
    }
}

/// Asks STUN servers what public address each socket maps to.
struct Stun {
    probes: Vec<Probe>,
}

struct Probe {
    server: String,
    addr: SocketAddr,
    socket: usize,
    trans: [u8; 12],
    sent: usize,
    done: bool,
}

impl Stun {
    async fn resolve(servers: &[String], sockets: usize) -> Self {
        let mut probes = Vec::new();
        for server in servers {
            let host = server.strip_prefix("stun:").unwrap_or(server);
            let lookup = tokio::time::timeout(STUN_RESOLVE_TIMEOUT, tokio::net::lookup_host(host));
            let addr = match lookup.await {
                Ok(Ok(mut addrs)) => addrs.find(SocketAddr::is_ipv4),
                Ok(Err(err)) => {
                    tracing::warn!(server, %err, "webrtc could not resolve a STUN server");
                    None
                }
                Err(_) => {
                    tracing::warn!(server, "webrtc STUN server lookup timed out");
                    None
                }
            };
            let Some(addr) = addr else { continue };
            for socket in 0..sockets {
                probes.push(Probe {
                    server: server.clone(),
                    addr,
                    socket,
                    trans: crate::random(),
                    sent: 0,
                    done: false,
                });
            }
        }
        Self { probes }
    }

    fn server_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .probes
            .iter()
            .map(|p| format!("{} ({})", p.server, p.addr))
            .collect();
        names.dedup();
        names
    }

    /// When the next request is due, or when to give up on a silent server.
    fn next_due(&self, started: Instant) -> Option<Instant> {
        self.probes
            .iter()
            .filter(|p| !p.done)
            .map(|p| started + p.due())
            .min()
    }

    async fn send_due(&mut self, started: Instant, sockets: &[Arc<UdpSocket>]) {
        let elapsed = started.elapsed();
        for probe in self.probes.iter_mut().filter(|p| !p.done) {
            if probe.due() > elapsed {
                continue;
            }
            if probe.sent == STUN_SCHEDULE.len() {
                probe.done = true;
                tracing::warn!(
                    server = probe.server,
                    local = ?sockets[probe.socket].local_addr().ok(),
                    "webrtc STUN server did not answer: no srflx candidate from it"
                );
                continue;
            }
            probe.sent += 1;
            let request = binding_request(probe.trans);
            if let Err(err) = sockets[probe.socket].send_to(&request, probe.addr).await {
                tracing::debug!(server = probe.server, %err, "webrtc STUN send failed");
            }
        }
    }

    /// The mapped address, if `packet` answers one of our requests.
    fn answer(&mut self, packet: &Packet) -> Option<(String, SocketAddr)> {
        let probe = self
            .probes
            .iter_mut()
            .find(|p| p.addr == packet.source && packet.bytes.get(8..20) == Some(&p.trans))?;
        let mapped = mapped_address(&packet.bytes)?;
        probe.done = true;
        Some((probe.server.clone(), mapped))
    }
}

impl Probe {
    fn due(&self) -> Duration {
        STUN_SCHEDULE
            .get(self.sent)
            .copied()
            .unwrap_or(STUN_GIVE_UP)
    }
}

const STUN_MAGIC: [u8; 4] = [0x21, 0x12, 0xa4, 0x42];

/// An RFC 8489 Binding request with no attributes.
fn binding_request(trans: [u8; 12]) -> [u8; 20] {
    let mut request = [0u8; 20];
    request[1] = 0x01;
    request[4..8].copy_from_slice(&STUN_MAGIC);
    request[8..20].copy_from_slice(&trans);
    request
}

/// XOR-MAPPED-ADDRESS (or the legacy MAPPED-ADDRESS) of a Binding success
/// response, IPv4 only.
fn mapped_address(response: &[u8]) -> Option<SocketAddr> {
    if response.get(0..2)? != [0x01, 0x01] || response.get(4..8)? != STUN_MAGIC {
        return None;
    }
    let mut attrs = response.get(20..)?;
    let mut plain = None;
    while attrs.len() >= 4 {
        let kind = u16::from_be_bytes([attrs[0], attrs[1]]);
        let len = u16::from_be_bytes([attrs[2], attrs[3]]) as usize;
        let value = attrs.get(4..4 + len)?;
        if len == 8 && value[1] == 0x01 {
            let port = u16::from_be_bytes([value[2], value[3]]);
            let ip = [value[4], value[5], value[6], value[7]];
            match kind {
                0x0020 => {
                    let port = port ^ 0x2112;
                    let ip: [u8; 4] = std::array::from_fn(|i| ip[i] ^ STUN_MAGIC[i]);
                    return Some(SocketAddr::from((ip, port)));
                }
                0x0001 => plain = Some(SocketAddr::from((ip, port))),
                _ => {}
            }
        }
        attrs = attrs.get(4 + len.next_multiple_of(4)..).unwrap_or_default();
    }
    plain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_request_is_a_bare_rfc8489_header() {
        let request = binding_request([7; 12]);
        assert_eq!(
            request,
            [
                0, 1, 0, 0, 0x21, 0x12, 0xa4, 0x42, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7
            ]
        );
    }

    #[test]
    fn mapped_address_reads_xor_mapped_then_plain() {
        let mut response = vec![1, 1, 0, 12, 0x21, 0x12, 0xa4, 0x42];
        response.extend([9; 12]);
        // XOR-MAPPED-ADDRESS for 203.0.113.9:51000.
        response.extend([0, 0x20, 0, 8, 0, 1, 0xe6, 0x2a, 0xea, 0x12, 0xd5, 0x4b]);
        assert_eq!(
            mapped_address(&response),
            Some("203.0.113.9:51000".parse().unwrap())
        );
        let mut legacy = response[..20].to_vec();
        legacy.extend([0, 0x01, 0, 8, 0, 1, 0xc7, 0x38, 203, 0, 113, 10]);
        assert_eq!(
            mapped_address(&legacy),
            Some("203.0.113.10:51000".parse().unwrap())
        );
        response[1] = 0x11;
        assert_eq!(mapped_address(&response), None);
    }
}
