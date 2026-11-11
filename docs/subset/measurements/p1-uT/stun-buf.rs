//! Asks public STUN servers what public address each WebRTC socket maps to.
//!
//! str0m does no STUN of its own, and its parser only accepts ICE messages,
//! which carry MESSAGE-INTEGRITY. Public STUN servers never send that, so the
//! plain RFC 8489 binding exchange is encoded here.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::ops::BitXor;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Buf;
use n0_future::StreamExt;
use tokio::net::UdpSocket;

use crate::rtc::Socket;

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(2);
/// When to send each binding request, from the start of the session.
const SCHEDULE: &[Duration] = &[
    Duration::ZERO,
    Duration::from_millis(250),
    Duration::from_millis(750),
    Duration::from_millis(1750),
];
const GIVE_UP: Duration = Duration::from_secs(3);

const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const MAGIC_COOKIE: u32 = 0x2112_a442;
/// The cookie's high half, which masks XOR-MAPPED-ADDRESS's port.
const PORT_MASK: u16 = 0x2112;
const MAPPED_ADDRESS: u16 = 0x0001;
const XOR_MAPPED_ADDRESS: u16 = 0x0020;
const FAMILY_IPV4: u16 = 0x0001;
/// Reserved byte, family, port and IPv4 address.
const IPV4_ADDRESS_LEN: usize = 8;
const HEADER_LEN: usize = 20;
const ATTRIBUTE_HEADER_LEN: usize = 4;

/// The binding requests of one session: one probe per server and socket,
/// by transaction ID.
pub(crate) struct Stun {
    probes: HashMap<[u8; 12], Probe>,
    names: Vec<String>,
}

/// A STUN server's answer: what it saw one of our sockets as.
pub(crate) struct Answer {
    pub(crate) server: String,
    pub(crate) mapped: SocketAddr,
}

struct Server {
    name: String,
    addr: SocketAddr,
}

#[derive(Clone)]
struct Probe {
    server: String,
    addr: SocketAddr,
    socket: Arc<UdpSocket>,
    local: SocketAddr,
    trans: [u8; 12],
    progress: Progress,
}

#[derive(Debug, Clone, Copy)]
enum Progress {
    /// This many requests are out, and no answer yet.
    Sent(usize),
    /// Answered, or given up on.
    Done,
}

impl Stun {
    /// Resolves each `stun:host:port` URL, one after the other.
    pub(crate) async fn resolve(servers: &[String], sockets: &[Socket]) -> Stun {
        let resolved = n0_future::stream::iter(servers.to_vec())
            .then(lookup)
            .collect::<Vec<Option<Server>>>()
            .await
            .into_iter()
            .flatten()
            .collect::<Vec<Server>>();
        Stun {
            probes: resolved
                .iter()
                .flat_map(|server| sockets.iter().map(|socket| Probe::new(server, socket)))
                .map(|probe| (probe.trans, probe))
                .collect(),
            names: resolved
                .iter()
                .map(|server| format!("{} ({})", server.name, server.addr))
                .collect(),
        }
    }

    pub(crate) fn server_names(&self) -> Vec<String> {
        self.names.clone()
    }

    /// When the next request is due, or when to give up on a silent server.
    pub(crate) fn next_due(&self, started: Instant) -> Option<Instant> {
        self.probes
            .values()
            .filter_map(Probe::due)
            .filter_map(|due| started.checked_add(due))
            .min()
    }

    /// Sends every request that is due, and gives up on servers that stayed
    /// silent through the whole schedule.
    pub(crate) async fn send_due(&mut self, started: Instant) {
        let elapsed = started.elapsed();
        let probes = std::mem::take(&mut self.probes);
        self.probes = n0_future::stream::iter(probes.into_values())
            .then(|probe| probe.tick(elapsed))
            .map(|probe| (probe.trans, probe))
            .collect()
            .await;
    }

    /// The mapped address, if `message` from `source` answers one of our
    /// requests.
    pub(crate) fn answer(&mut self, source: SocketAddr, message: &[u8]) -> Option<Answer> {
        let binding = binding_response(message)?;
        let probe = self
            .probes
            .get(&binding.trans)
            .filter(|probe| probe.addr == source)?;
        let answer = Answer {
            server: probe.server.clone(),
            mapped: binding.mapped,
        };
        let done = Probe {
            progress: Progress::Done,
            ..probe.clone()
        };
        let _ = self.probes.insert(binding.trans, done);
        Some(answer)
    }
}

async fn lookup(server: String) -> Option<Server> {
    let host = server.strip_prefix("stun:").unwrap_or(&server).to_owned();
    let lookup = tokio::time::timeout(RESOLVE_TIMEOUT, tokio::net::lookup_host(host)).await;
    let addr = match lookup {
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
    addr.map(|addr| Server { name: server, addr })
}

impl Probe {
    fn new(server: &Server, socket: &Socket) -> Probe {
        Probe {
            server: server.name.clone(),
            addr: server.addr,
            socket: socket.udp.clone(),
            local: socket.addr,
            trans: crate::random(),
            progress: Progress::Sent(0),
        }
    }

    fn due(&self) -> Option<Duration> {
        match self.progress {
            Progress::Sent(sent) => Some(SCHEDULE.get(sent).copied().unwrap_or(GIVE_UP)),
            Progress::Done => None,
        }
    }

    async fn tick(self, elapsed: Duration) -> Probe {
        match (self.progress, self.due().filter(|due| due <= &elapsed)) {
            (Progress::Sent(sent), Some(_)) => self.send(sent).await,
            (Progress::Sent(_), None) | (Progress::Done, _) => self,
        }
    }

    async fn send(self, sent: usize) -> Probe {
        match SCHEDULE.get(sent) {
            Some(_) => {
                let request = binding_request(self.trans);
                if let Err(err) = self.socket.send_to(&request, self.addr).await {
                    tracing::debug!(server = self.server, %err, "webrtc STUN send failed");
                }
                Probe {
                    progress: Progress::Sent(sent.saturating_add(1)),
                    ..self
                }
            }
            None => {
                tracing::warn!(
                    server = self.server,
                    local = ?Some(self.local),
                    "webrtc STUN server did not answer: no srflx candidate from it"
                );
                Probe {
                    progress: Progress::Done,
                    ..self
                }
            }
        }
    }
}

/// An RFC 8489 Binding request with no attributes.
fn binding_request(trans: [u8; 12]) -> Vec<u8> {
    BINDING_REQUEST
        .to_be_bytes()
        .into_iter()
        .chain(0u16.to_be_bytes())
        .chain(MAGIC_COOKIE.to_be_bytes())
        .chain(trans)
        .collect()
}

/// A Binding success response: which request it answers, and the address
/// the server saw.
struct Binding {
    trans: [u8; 12],
    mapped: SocketAddr,
}

fn binding_response(message: &[u8]) -> Option<Binding> {
    let mut header = message;
    match (
        header.try_get_u16().ok()?,
        header.try_get_u16().ok()?,
        header.try_get_u32().ok()?,
    ) {
        (BINDING_SUCCESS, _, MAGIC_COOKIE) => {
            let (trans, attributes) = header.split_first_chunk()?;
            Some(Binding {
                trans: trans.to_owned(),
                mapped: mapped_address(attributes, None)?,
            })
        }
        _ => None,
    }
}

/// Reads the attributes in order, IPv4 only. The first XOR-MAPPED-ADDRESS
/// wins, else the last MAPPED-ADDRESS. A value cut short before either
/// rejects the whole response.
fn mapped_address(mut attributes: &[u8], plain: Option<SocketAddr>) -> Option<SocketAddr> {
    match (attributes.try_get_u16(), attributes.try_get_u16()) {
        (Ok(kind), Ok(len)) => {
            let len = usize::from(len);
            let (value, _) = attributes.split_at_checked(len)?;
            let (_, rest) = attributes
                .split_at_checked(len.next_multiple_of(4))
                .unwrap_or_default();
            match (kind, ipv4(value)) {
                (XOR_MAPPED_ADDRESS, Some(xored)) => Some(unxor(xored)),
                (MAPPED_ADDRESS, Some(addr)) => mapped_address(rest, Some(SocketAddr::V4(addr))),
                _ => mapped_address(rest, plain),
            }
        }
        _ => plain,
    }
}

/// An address value, if it is IPv4.
fn ipv4(mut value: &[u8]) -> Option<SocketAddrV4> {
    match (
        value.len(),
        value.try_get_u16(),
        value.try_get_u16(),
        value.try_get_u32(),
    ) {
        (IPV4_ADDRESS_LEN, Ok(FAMILY_IPV4), Ok(port), Ok(ip)) => {
            Some(SocketAddrV4::new(Ipv4Addr::from_bits(ip), port))
        }
        _ => None,
    }
}

fn unxor(xored: SocketAddrV4) -> SocketAddr {
    let ip = Ipv4Addr::from_bits(xored.ip().to_bits().bitxor(MAGIC_COOKIE));
    SocketAddr::V4(SocketAddrV4::new(ip, xored.port().bitxor(PORT_MASK)))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::{binding_request, binding_response};

    const HEADER: [u8; 20] = [
        1, 1, 0, 12, 0x21, 0x12, 0xa4, 0x42, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
    ];
    const NOT_SUCCESS: [u8; 20] = [
        1, 0x11, 0, 12, 0x21, 0x12, 0xa4, 0x42, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9,
    ];
    /// XOR-MAPPED-ADDRESS for 203.0.113.9:51000.
    const XOR_MAPPED: [u8; 12] = [0, 0x20, 0, 8, 0, 1, 0xe6, 0x2a, 0xea, 0x12, 0xd5, 0x4b];
    /// MAPPED-ADDRESS for 203.0.113.10:51000.
    const MAPPED: [u8; 12] = [0, 0x01, 0, 8, 0, 1, 0xc7, 0x38, 203, 0, 113, 10];
    /// An XOR-MAPPED-ADDRESS whose value is cut short.
    const CUT_SHORT: [u8; 5] = [0, 0x20, 0, 8, 0];

    fn mapped(header: [u8; 20], first: &[u8], second: &[u8]) -> Option<SocketAddr> {
        let message = header
            .iter()
            .chain(first)
            .chain(second)
            .copied()
            .collect::<Vec<u8>>();
        binding_response(&message).map(|binding| binding.mapped)
    }

    #[test]
    fn binding_request_is_a_bare_rfc8489_header() {
        assert_eq!(
            binding_request([7; 12]),
            [
                0, 1, 0, 0, 0x21, 0x12, 0xa4, 0x42, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7
            ]
        );
    }

    #[test]
    fn binding_response_reads_xor_mapped_then_plain() {
        let xor = Some("203.0.113.9:51000".parse().unwrap());
        let plain = Some("203.0.113.10:51000".parse().unwrap());
        assert_eq!(mapped(HEADER, &XOR_MAPPED, &[]), xor);
        assert_eq!(mapped(HEADER, &MAPPED, &[]), plain);
        assert_eq!(mapped(HEADER, &MAPPED, &XOR_MAPPED), xor);
        assert_eq!(mapped(NOT_SUCCESS, &XOR_MAPPED, &[]), None);
    }

    #[test]
    fn a_value_cut_short_rejects_the_response_unless_xor_mapped_came_first() {
        let xor = Some("203.0.113.9:51000".parse().unwrap());
        assert_eq!(mapped(HEADER, &MAPPED, &CUT_SHORT), None);
        assert_eq!(mapped(HEADER, &XOR_MAPPED, &CUT_SHORT), xor);
    }
}
