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
//!
//! Frame bodies are postcard, which is not self-describing: any breaking
//! change to these types bumps [`ALPN`].

mod frame;
mod relay;
mod ticket;

use serde::{Deserialize, Serialize};

pub use frame::{
    CHUNK_LEN, FrameBuffer, FrameError, HEADER_LEN, Reassembly, Remainder, body_len, decode, encode,
};
pub use relay::without_root_dot;
pub use ticket::{AuthToken, Ticket, TicketError};

/// The ALPN of the protocol. The event and command types change
/// incompatibly between versions, so an old client fails at the handshake.
pub const ALPN: &[u8] = b"conductor/1";

/// The host's event cursor. Monotonic per [`Epoch`]; the first event is 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HostSeq(pub u64);

impl HostSeq {
    pub fn next(self) -> HostSeq {
        HostSeq(self.0.saturating_add(1))
    }
}

/// Identifies one incarnation of a host's event log. A cursor from another
/// epoch is meaningless, so the client starts over from seq 0 (§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Epoch(pub u64);

/// Chosen by the client. P1 makes it a ULID for idempotent retries (§5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommandId(pub u64);

/// An approval request, assigned by the host per session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(pub u64);

/// The first frame on every client-opened bi-stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Open {
    Hello(Hello),
    Subscribe(Subscribe),
    Command(Command),
    Rtc(RtcOffer),
}

/// Public STUN servers both ends ask for their public address by default.
/// They are stateless and see no traffic; nothing relays (§5.4).
pub const DEFAULT_STUN_SERVERS: &[&str] = &[
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
    pub client: ClientName,
    pub token: AuthToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Assigned by the host, from 1, in order of creation. Unique within an
/// epoch, which is all an in-memory log needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(pub u64);

/// A tool call's id as the provider gave it (`toolu_…` or a Codex item id).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolCallId(pub String);

/// A path on the host, as the host's OS spells it. The client never
/// interprets it: a browser on a phone can't know a Windows path's rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostPath(pub String);

/// The name a client gave in `Hello`. Shown as "by" on a resolution and a
/// user message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientName(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    Claude,
    Codex,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub id: CommandId,
    pub body: CommandBody,
}

/// Every command after `StartSession` names the session it acts on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandBody {
    StartSession {
        provider: Provider,
        directory: HostPath,
    },
    SendMessage {
        session: SessionId,
        text: String,
    },
    Interrupt {
        session: SessionId,
    },
    Resolve {
        session: SessionId,
        request: RequestId,
        decision: Decision,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Approve,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandResult {
    Accepted,
    /// The answer to `StartSession`. `SessionCreated` follows in the log.
    Started {
        session: SessionId,
    },
    Rejected(Rejection),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rejection {
    NoSuchDirectory,
    ProviderUnavailable { reason: String },
    UnknownSession,
    SessionClosed,
    TurnInProgress,
    NoTurnRunning,
    UnknownRequest,
    AlreadyResolved { by: ClientName },
}

impl Rejection {
    /// Why the host refused the command, for a person to read.
    pub fn label(&self) -> String {
        match self {
            Rejection::NoSuchDirectory => String::from("no directory at that path on the host"),
            Rejection::ProviderUnavailable { reason } => {
                format!("that provider is unavailable: {reason}")
            }
            Rejection::UnknownSession => String::from("no session with that id"),
            Rejection::SessionClosed => String::from("the session has closed"),
            Rejection::TurnInProgress => String::from("a turn is already running"),
            Rejection::NoTurnRunning => String::from("no turn is running"),
            Rejection::UnknownRequest => String::from("no request with that id"),
            Rejection::AlreadyResolved { by } => format!("already resolved by {}", by.0),
        }
    }
}

/// Every event names its session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub seq: HostSeq,
    pub session: SessionId,
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    SessionCreated {
        provider: Provider,
        directory: HostPath,
        by: ClientName,
    },
    /// The agent process is up and takes messages.
    SessionReady,
    SessionClosed {
        reason: CloseReason,
    },
    UserMessage {
        text: String,
        by: ClientName,
    },
    AssistantDelta {
        text: String,
    },
    /// The finished text of one assistant message. It replaces the deltas
    /// since the previous message.
    AssistantMessage {
        text: String,
    },
    ToolCall {
        call: ToolCallId,
        phase: ToolPhase,
    },
    RequestOpened {
        request: RequestId,
        tool: String,
        detail: String,
    },
    RequestResolved {
        request: RequestId,
        resolution: Resolution,
    },
    TurnCompleted {
        outcome: TurnOutcome,
    },
    /// Something went wrong that did not end the session.
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolPhase {
    Started { tool: ToolKind },
    Finished { outcome: ToolOutcome },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolKind {
    Command { command: String },
    FileChange { paths: Vec<String> },
    Other { name: String, detail: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolOutcome {
    Succeeded,
    Failed,
    Declined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    Decided {
        decision: Decision,
        by: ClientName,
    },
    /// The agent withdrew the request before anyone answered.
    Withdrawn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnOutcome {
    Completed,
    Interrupted,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloseReason {
    FailedToStart { message: String },
    Exited { status: String },
}
