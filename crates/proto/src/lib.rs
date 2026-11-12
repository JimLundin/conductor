//! The wire protocol between a client and `conductord` (architecture §5). Pure
//! types and the frame codec; no I/O, so it compiles natively and to wasm.
//!
//! Every bi-stream starts with one [`Open`] frame from the client. The first
//! stream of a connection must be [`Open::Hello`], answered by one
//! [`HelloReply`]. A [`Open::Command`] stream gets one [`CommandResult`] and
//! closes. A [`Open::Subscribe`] stream then carries [`Event`]s until either
//! side closes it.
//!
//! Frame bodies are postcard, which is not self-describing. A variant is
//! its index and a field is its position, so a reordered enum or struct
//! decodes as the wrong value with no error, and a variant added at the end
//! fails an old peer mid-stream. So any change to a type reachable from
//! [`Open`], [`HelloReply`], [`CommandResult`] or [`Event`] bumps [`ALPN`],
//! including a variant added at the end. Renaming a variant or a field
//! doesn't change the bytes and needs no bump.

mod frame;
mod relay;
mod ticket;

use serde::{Deserialize, Serialize};

pub use frame::{FrameError, HEADER_LEN, body_len, decode, encode};
pub use relay::without_root_dot;
pub use ticket::{AuthToken, Ticket, TicketError};

/// The ALPN of the protocol. It changes with every change to the wire types
/// (see the crate docs), so an old client fails at the handshake instead of
/// mid-stream.
pub const ALPN: &[u8] = b"conductor/5";

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

/// An approval request, assigned by the host per session, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RequestId(pub u64);

impl RequestId {
    pub fn next(self) -> RequestId {
        RequestId(self.0.saturating_add(1))
    }
}

/// The first frame on every client-opened bi-stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Open {
    Hello(Hello),
    Subscribe(Subscribe),
    Command(Command),
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
    pub providers: Providers,
}

/// Whether the host can run each provider, fixed when it starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Providers {
    pub claude: Availability,
    pub codex: Availability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Availability {
    Available,
    /// The host can't run this provider's sessions, for this reason.
    Unavailable {
        reason: String,
    },
}

impl Providers {
    pub fn of(&self, provider: Provider) -> &Availability {
        match provider {
            Provider::Claude => &self.claude,
            Provider::Codex => &self.codex,
        }
    }
}

/// Replays every stored event after `after_seq`, then streams live ones,
/// deltas included. A subscription from another epoch is refused by closing
/// the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscribe {
    pub epoch: Epoch,
    pub after_seq: HostSeq,
}

/// Assigned by the host, from 1, in order of creation. Unique within an
/// epoch, which is all an in-memory log needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(pub u64);

impl SessionId {
    pub fn next(self) -> SessionId {
        SessionId(self.0.saturating_add(1))
    }
}

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

/// Every event names its session. A stored event has a seq of its own; an
/// `AssistantDelta` repeats the last one's.
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
    /// A fragment of the assistant message being streamed. Live only: the
    /// host stores none, so a subscription never replays one, and the
    /// `AssistantMessage` that follows replaces them. It carries the seq of
    /// the stored event before it, which may be another session's, so a
    /// client that has applied that seq takes it and one that hasn't sees a
    /// gap.
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
    /// An approval the tool call `call` waits on. `tool` repeats what the
    /// call's `Started` said, so a client can show the request on its own.
    /// `reason` is the agent's own words for why it asks, when it gives any.
    RequestOpened {
        request: RequestId,
        call: ToolCallId,
        tool: ToolKind,
        reason: Option<String>,
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
    FileChange { files: Vec<FileChange> },
    Other { name: String, detail: String },
}

/// One file a tool call changes, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub change: Change,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Change {
    /// The whole file written, new or replaced.
    Write,
    /// Part of an existing file changed.
    Edit,
    Delete,
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
