//! The wire protocol between a client and `conductord` (architecture §5). Pure
//! types and the frame codec; no I/O, so it compiles natively and to wasm.
//!
//! Every bi-stream starts with one [`Open`] frame from the client. The first
//! stream of a connection must be [`Open::Hello`], answered by one
//! [`HelloReply`]. A [`Open::Command`] stream gets one [`CommandResult`] and
//! closes. A [`Open::Subscribe`] stream then carries [`Event`]s until either
//! side closes it. A [`Open::Rtc`] stream negotiates a WebRTC connection that
//! carries the same streams as data channels (§5.4); it then carries
//! [`Signal`]s both ways for as long as that connection lives.

mod frame;
mod relay;
mod ticket;

use serde::{Deserialize, Serialize};

pub use frame::{
    CHUNK_LEN, FrameBuffer, FrameError, HEADER_LEN, Reassembly, Remainder, body_len, decode, encode,
};
pub use relay::without_root_dot;
pub use ticket::{AuthToken, Ticket, TicketError};

/// The ALPN for the S3 protocol. P1 moves to `conductor/1` (§5.1).
pub const ALPN: &[u8] = b"conductor/0";

/// The host's event cursor. Monotonic per [`Epoch`]; the first event is 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostSeq(pub u64);

/// Identifies one incarnation of a host's event log. A cursor from another
/// epoch is meaningless, so the client starts over from seq 0 (§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Epoch(pub u64);

/// Chosen by the client. P1 makes it a ULID for idempotent retries (§5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandId(pub u64);

/// An approval request, assigned by the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub u64);

/// The first frame on every client-opened bi-stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "open", rename_all = "snake_case")]
pub enum Open {
    Hello(Hello),
    Subscribe(Subscribe),
    Command(Command),
    Rtc(RtcOffer),
}

/// Public STUN servers both ends ask for their public address by default.
/// They are stateless and see no traffic; nothing relays (§5.4).
pub const DEFAULT_STUN_SERVERS: [&str; 2] = [
    "stun:stun.l.google.com:19302",
    "stun:stun.cloudflare.com:3478",
];

/// The browser's SDP offer. It carries the browser's DTLS fingerprint, and
/// the answer carries the host's, so only the two ends of this authenticated
/// iroh connection can complete the WebRTC handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RtcOffer {
    pub sdp: String,
}

/// Everything after the offer on an [`Open::Rtc`] stream. Closing the stream
/// ends the WebRTC connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "signal", rename_all = "snake_case")]
pub enum Signal {
    /// Host to client, once, first.
    Answer { sdp: String },
    /// Both ways, trickled as each side learns its addresses. The text is an
    /// SDP `candidate:` attribute value.
    Candidate { candidate: String },
    /// Client to host, so the host log records what the browser chose.
    Selected {
        local: CandidateType,
        remote: CandidateType,
    },
    /// Client to host, so the host log records why an upgrade failed.
    Failed { reason: String },
}

/// How an ICE candidate's address was found (RFC 8445 §5.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateType {
    /// An interface address.
    Host,
    /// The public address a STUN server saw.
    Srflx,
    /// An address the peer saw our checks come from.
    Prflx,
    /// A TURN allocation. Conductor configures no TURN servers.
    Relay,
}

impl CandidateType {
    /// Reads the `typ` field of an SDP `candidate:` attribute value.
    pub fn of_candidate(candidate: &str) -> Option<CandidateType> {
        let words = candidate.split_ascii_whitespace();
        words
            .clone()
            .zip(words.skip(1))
            .find_map(|(key, value)| match key {
                "typ" => Some(value),
                _ => None,
            })
            .and_then(CandidateType::parse)
    }

    /// Parses the names browsers use in `RTCIceCandidateStats.candidateType`.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "host" => Some(Self::Host),
            "srflx" => Some(Self::Srflx),
            "prflx" => Some(Self::Prflx),
            "relay" => Some(Self::Relay),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Host => "host",
            Self::Srflx => "srflx",
            Self::Prflx => "prflx",
            Self::Relay => "relay",
        }
    }
}

/// S3 auth placeholder: the ticket's token stands in for the `DeviceCert`
/// that §4 puts here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// Shown as `by` when this client resolves an approval.
    pub client: String,
    pub token: AuthToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum HelloReply {
    Welcome(Welcome),
    Refused { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    pub host: String,
    pub epoch: Epoch,
    pub head_seq: HostSeq,
}

/// Replays every event after `after_seq`, then streams live ones. A
/// subscription from another epoch is refused by closing the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscribe {
    pub epoch: Epoch,
    pub after_seq: HostSeq,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub seq: HostSeq,
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    TranscriptDelta {
        text: String,
    },
    AssistantMessage {
        text: String,
    },
    ApprovalRequested {
        id: RequestId,
        tool: String,
        detail: String,
    },
    ApprovalResolved {
        id: RequestId,
        decision: Decision,
        by: String,
    },
    TurnFinished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approve,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub id: CommandId,
    pub body: CommandBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandBody {
    Prompt {
        text: String,
    },
    Resolve {
        approval: RequestId,
        decision: Decision,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum CommandResult {
    Accepted,
    Rejected(Rejection),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Rejection {
    TurnInProgress,
    UnknownApproval,
    AlreadyResolved { by: String },
}

impl Rejection {
    /// Why the host refused the command, for a person to read.
    pub fn label(&self) -> String {
        match self {
            Rejection::TurnInProgress => String::from("a turn is already running"),
            Rejection::UnknownApproval => String::from("no approval with that id"),
            Rejection::AlreadyResolved { by } => format!("already resolved by {by}"),
        }
    }
}
