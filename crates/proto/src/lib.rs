//! The wire protocol between a client and `conductord` (architecture §5). Pure
//! types and the frame codec; no I/O, so it compiles natively and to wasm.
//!
//! Every bi-stream starts with one [`Open`] frame from the client. The first
//! stream of a connection must be [`Open::Hello`], answered by one
//! [`HelloReply`]. A [`Open::Command`] stream gets one [`CommandResult`] and
//! closes. A [`Open::Subscribe`] stream then carries [`Event`]s until either
//! side closes it.

mod frame;
mod ticket;

use serde::{Deserialize, Serialize};

pub use frame::{FrameError, HEADER_LEN, MAX_FRAME_LEN, body_len, decode, encode};
pub use ticket::{AuthToken, Ticket, TicketError};

/// The ALPN for the S3 protocol. P1 moves to `conductor/1` (§5.1).
pub const ALPN: &[u8] = b"conductor/0";

/// The host's event cursor. Monotonic per [`Epoch`]; the first event is 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Rejection {
    #[error("a turn is already running")]
    TurnInProgress,
    #[error("no approval with that id")]
    UnknownApproval,
    #[error("already resolved by {by}")]
    AlreadyResolved { by: String },
}
