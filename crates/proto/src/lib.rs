//! The wire protocol between a client and `conductord` (architecture §5): the
//! types and the frame codec. It is the pure core: `no_std`, with the stream
//! I/O in the `net` crate.
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

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod frame;
mod ticket;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

pub use frame::{FrameError, HEADER_LEN, body_len, decode, encode};
pub use ticket::{AuthToken, Ticket, TicketError, WrongToken};

/// The ALPN of the protocol. It changes with every change to the wire types
/// (see the crate docs), so an old client fails at the handshake instead of
/// mid-stream.
pub const ALPN: &[u8] = b"conductor/12";

/// The host's event cursor. Monotonic per [`Epoch`]; the first event is 1.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
pub struct HostSeq(pub u64);

impl HostSeq {
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// Identifies one incarnation of a host's event log. A cursor from another
/// epoch is meaningless, so the client starts over from seq 0 (§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Epoch(pub u64);

/// A client-generated, fleet-unique 128-bit random id for idempotent retries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommandId(pub u128);

/// An approval request or a question, assigned by the host per session, in
/// order.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
pub struct RequestId(pub u64);

impl RequestId {
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
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

/// Whether the host can run each provider, and how it signs in. A host
/// lists every provider it knows of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Providers(pub BTreeMap<Provider, Availability>);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Availability {
    Available {
        auth: Auth,
        models: Models,
    },
    /// The host can't run this provider's sessions, for this reason.
    Unavailable {
        reason: String,
    },
}

/// The models a provider's CLI offers on this host, as the CLI lists them
/// when the host starts, and the one a session gets when it names none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Models {
    pub choices: Vec<ModelChoice>,
    pub default: Option<ModelId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChoice {
    pub id: ModelId,
    pub name: String,
    pub description: String,
    /// The efforts this model takes, lowest first.
    pub efforts: Vec<Effort>,
}

/// A model by the name its CLI takes (`haiku`, `gpt-6-luna`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelId(pub String);

/// How hard the model reasons: Claude's `--effort`, Codex's
/// `model_reasoning_effort`. Both CLIs spell the levels the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effort {
    Low,
    Medium,
    High,
    XHigh,
    Max,
    Ultra,
}

impl Effort {
    #[must_use]
    pub const fn wire(&self) -> &str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
    }

    /// A level a CLI lists. Any other, such as Codex's `minimal`, is `None`.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::XHigh),
            "max" => Some(Self::Max),
            "ultra" => Some(Self::Ultra),
            _ => None,
        }
    }
}

impl Models {
    #[must_use]
    pub const fn none() -> Self {
        Self {
            choices: Vec::new(),
            default: None,
        }
    }

    #[must_use]
    pub fn find(&self, id: &ModelId) -> Option<&ModelChoice> {
        self.choices.iter().find(|choice| &choice.id == id)
    }
}

/// How the host's CLI for a provider reaches its model. It names the kind
/// and never carries a secret: credentials are write-only (§6.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Auth {
    /// A gateway credential the host stores, at this base URL.
    Gateway { base_url: String },
    /// The CLI's own sign-in in the host's private home (`conductord login`).
    SignedIn,
    /// A CLI the host was started with (`--claude-path`, `--codex-path`),
    /// which signs in however it does.
    Unmanaged,
}

/// The credentials a host stores, at most one per provider.
///
/// This is the unit the fleet config (§6.5) carries to every host. Other
/// kinds, an API key and the Claude subscription token, become variants of
/// each provider's enum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials(BTreeMap<Provider, Credential>);

/// One provider's credential, as `SetCredential` carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Credential {
    Claude(ClaudeCredential),
    Codex(CodexCredential),
}

/// Its `Debug` shows only the kind, so no value reaches a log.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClaudeCredential {
    /// Claude Code's `ANTHROPIC_BASE_URL` and `ANTHROPIC_AUTH_TOKEN`.
    Gateway { base_url: String, token: String },
}

/// Its `Debug` shows only the kind, so no value reaches a log.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodexCredential {
    /// A custom model provider: Codex's `[model_providers.<id>]` with
    /// `name` and `base_url`, selected with `model`. Its `wire_api` is
    /// `responses`, the only one Codex 0.166 knows. The key reaches Codex in
    /// the environment variable the section's `env_key` names, never in its
    /// config.
    Provider {
        name: String,
        base_url: String,
        key: String,
        model: String,
    },
}

impl core::fmt::Debug for ClaudeCredential {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Gateway { .. } => write!(f, "Gateway"),
        }
    }
}

impl core::fmt::Debug for CodexCredential {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Provider { .. } => write!(f, "Provider"),
        }
    }
}

impl Credentials {
    #[must_use]
    pub const fn none() -> Self {
        Self(BTreeMap::new())
    }

    /// These credentials with `credential` in its provider's place.
    #[must_use]
    pub fn set(mut self, credential: Credential) -> Self {
        self.0.insert(credential.provider(), credential);
        self
    }

    #[must_use]
    pub fn clear(mut self, provider: Provider) -> Self {
        self.0.remove(&provider);
        self
    }

    #[must_use]
    pub fn of(&self, provider: Provider) -> Option<&Credential> {
        self.0.get(&provider)
    }

    /// What `Welcome` may say about a provider's credential: its kind and
    /// base URL.
    #[must_use]
    pub fn auth(&self, provider: Provider) -> Option<Auth> {
        self.of(provider).map(|credential| Auth::Gateway {
            base_url: String::from(credential.base_url()),
        })
    }
}

impl Credential {
    #[must_use]
    pub const fn provider(&self) -> Provider {
        match self {
            Self::Claude(_) => Provider::Claude,
            Self::Codex(_) => Provider::Codex,
        }
    }

    /// Refuses a base URL that isn't plain `http(s)://host/path`. The URL is
    /// shown to every client and written into Codex's config, so it must
    /// not hide a secret in userinfo, a query or a fragment.
    ///
    /// # Errors
    ///
    /// Why the base URL is refused.
    pub fn check(&self) -> Result<(), String> {
        let url = url::Url::parse(self.base_url())
            .map_err(|err| format!("the base URL: {err}"))?;
        match (
            url.scheme(),
            url.username(),
            url.password(),
            url.query(),
            url.fragment(),
        ) {
            ("http" | "https", "", None, None, None) => Ok(()),
            ("http" | "https", _, _, _, _) => Err(String::from(
                "the base URL may not carry a user, a password, a query or a fragment",
            )),
            (_, _, _, _, _) => {
                Err(String::from("the base URL must be http or https"))
            }
        }
    }
}

impl Credential {
    fn base_url(&self) -> &str {
        match self {
            Self::Claude(ClaudeCredential::Gateway { base_url, .. })
            | Self::Codex(CodexCredential::Provider { base_url, .. }) => {
                base_url
            }
        }
    }
}

impl Providers {
    /// `None` when the host doesn't know of `provider`.
    #[must_use]
    pub fn of(&self, provider: Provider) -> Option<&Availability> {
        self.0.get(&provider)
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

/// A fleet-unique ULID assigned by the host, represented as its 128 bits.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
pub struct SessionId(pub u128);

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

/// The providers a host may run: the one place they are listed.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    pub const ALL: [Self; 2] = [Self::Claude, Self::Codex];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub id: CommandId,
    pub body: CommandBody,
}

/// Every command after `StartSession` names the session it acts on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandBody {
    /// `None` takes the provider's default model, or the CLI's default
    /// effort for the model.
    StartSession {
        provider: Provider,
        directory: HostPath,
        model: Option<ModelId>,
        effort: Option<Effort>,
    },
    SendMessage {
        session: SessionId,
        text: String,
    },
    Interrupt {
        session: SessionId,
    },
    /// Starts a closed session's agent again, resuming its conversation
    /// with the provider's own resume. `SessionReady` follows in the log.
    /// On a session whose agent runs it is `Accepted` and does nothing.
    Continue {
        session: SessionId,
    },
    Resolve {
        session: SessionId,
        request: RequestId,
        decision: Decision,
    },
    /// Answers a question, one [`Answer`] per question in the order asked.
    /// Like `Resolve`, the first reply wins.
    Answer {
        session: SessionId,
        request: RequestId,
        answers: Vec<Answer>,
    },
    /// Stores a provider's credential on the host, replacing any it had.
    /// The host never sends it back. Like every command it needs a client
    /// admitted by the ticket's token, which device certificates replace
    /// with the fleet (§6.1).
    SetCredential {
        credential: Credential,
    },
    ClearCredential {
        provider: Provider,
    },
    /// A bounded diff between this checkpoint and its predecessor.
    GetDiff {
        session: SessionId,
        checkpoint: CheckpointId,
    },
    /// Removes this turn and every later turn, restoring its predecessor.
    UndoTurn {
        session: SessionId,
        checkpoint: CheckpointId,
    },
    /// Sets the effort of the session's later turns, one its model lists.
    /// A running turn keeps its effort: the next turn the user starts, or
    /// a `Continue`, takes the new one.
    SetEffort {
        session: SessionId,
        effort: Effort,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Approve,
    /// Approves this call and the calls the request's `for_session` names,
    /// until the session ends.
    ApproveForSession,
    Deny,
}

/// The answer to one question: the labels of the options chosen, and any
/// text the user typed instead, in that order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer(pub Vec<String>);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandResult {
    Accepted,
    /// The answer to `StartSession`. `SessionCreated` follows in the log.
    Started {
        session: SessionId,
    },
    /// The answer to `SetCredential` and `ClearCredential`: what the host
    /// runs now. Another client learns it from its next `Welcome`.
    Providers(Providers),
    Rejected(Rejection),
    Diff(CheckpointDiff),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rejection {
    NoSuchDirectory,
    ProviderUnavailable {
        reason: String,
    },
    UnknownSession,
    SessionClosed,
    /// The host doesn't offer that model or effort, for this reason.
    NotOffered {
        reason: String,
    },
    TurnInProgress,
    NoTurnRunning,
    UnknownRequest,
    AlreadyResolved {
        by: ClientName,
    },
    /// The reply doesn't fit the request, for this reason.
    WrongReply {
        reason: String,
    },
    /// The host could not store the credential, and kept the one it had.
    CredentialNotSaved {
        reason: String,
    },
    /// The credential is malformed, for this reason, and was not stored.
    InvalidCredential {
        reason: String,
    },
    NotGitRepository,
    CheckpointUnavailable,
    UndoInProgress,
    UndoFailed {
        reason: String,
    },
    WorkspaceFailure {
        reason: String,
    },
}

impl Rejection {
    /// Why the host refused the command, for a person to read.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::NoSuchDirectory => {
                String::from("no directory at that path on the host")
            }
            Self::ProviderUnavailable { reason } => {
                format!("that provider is unavailable: {reason}")
            }
            Self::UnknownSession => String::from("no session with that id"),
            Self::SessionClosed => String::from("the session has closed"),
            Self::TurnInProgress => String::from("a turn is already running"),
            Self::NoTurnRunning => String::from("no turn is running"),
            Self::UnknownRequest => String::from("no request with that id"),
            Self::AlreadyResolved { by } => {
                format!("already resolved by {}", by.0)
            }
            Self::CredentialNotSaved { reason } => {
                format!("the host could not store the credential: {reason}")
            }
            Self::InvalidCredential { reason } => {
                format!("the credential is not valid: {reason}")
            }
            Self::NotGitRepository => {
                String::from("choose a Git repository with a committed HEAD")
            }
            Self::CheckpointUnavailable => {
                String::from("this checkpoint cannot be undone")
            }
            Self::UndoInProgress => {
                String::from("finish recovering the interrupted undo first")
            }
            Self::NotOffered { reason }
            | Self::WrongReply { reason }
            | Self::UndoFailed { reason }
            | Self::WorkspaceFailure { reason } => reason.clone(),
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
    /// `model` and `effort` are what the agent runs with; `None` is the
    /// CLI's own default.
    SessionCreated {
        provider: Provider,
        directory: HostPath,
        by: ClientName,
        model: Option<ModelId>,
        effort: Option<Effort>,
    },
    /// The agent process is up and takes messages: at the start, and again
    /// after a `Continue`.
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
    /// A fragment of the agent's reasoning as it streams. Live only, like
    /// `AssistantDelta`; the `Reasoning` that follows replaces them.
    ReasoningDelta {
        text: String,
    },
    /// The finished text of one stretch of reasoning: the summary the
    /// provider gives, not the model's raw thoughts.
    Reasoning {
        text: String,
    },
    ToolCall {
        call: ToolCallId,
        phase: ToolPhase,
    },
    /// An approval the tool call `call` waits on. `tool` repeats what the
    /// call's `Started` said, so a client can show the request on its own.
    /// `reason` is the agent's own words for why it asks, when it gives any.
    /// `for_session` names what `ApproveForSession` would allow, in the
    /// agent's words (`Bash(git status)`, or the command), when it offers
    /// that.
    RequestOpened {
        request: RequestId,
        call: ToolCallId,
        tool: ToolKind,
        reason: Option<String>,
        for_session: Option<String>,
    },
    /// The agent asks the user one or more questions and waits for an
    /// `Answer`. `RequestResolved` closes it, as it does an approval.
    QuestionAsked {
        request: RequestId,
        questions: Vec<Question>,
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
    /// A `Continue` runs the agent on another model, because the current
    /// credential no longer offers the one it ran, as `reason` says.
    ModelChanged {
        model: Option<ModelId>,
        effort: Option<Effort>,
        reason: String,
    },
    /// Appended metadata: legacy sessions keep their original directory.
    WorkspaceCreated {
        workspace: Workspace,
    },
    CheckpointCreated {
        checkpoint: Checkpoint,
    },
    /// Durable intent before changing either files or provider history.
    UndoStarted {
        checkpoint: CheckpointId,
        restore: CheckpointId,
        history: HistoryAnchor,
    },
    UndoFailed {
        checkpoint: CheckpointId,
        reason: String,
    },
    /// Successful native-history phase; the new conversation is the resume target.
    UndoHistoryRestored {
        checkpoint: CheckpointId,
        history: HistoryAnchor,
    },
    /// Both files and native history were restored; remove this turn and later.
    TurnsUndone {
        checkpoint: CheckpointId,
        restored: CheckpointId,
    },
    /// Reserves an ID before a filesystem snapshot; failed snapshots never reuse refs.
    CheckpointReserved {
        checkpoint: CheckpointId,
    },
    /// A prepared native history descriptor has materialized on an actual turn.
    HistoryMaterialized {
        history: HistoryAnchor,
    },
    NativeTurnStarted {
        history: HistoryAnchor,
        turn_start: Option<HostSeq>,
    },
    /// Every task the session holds now, in the agent's order. It replaces
    /// the list before it.
    TasksChanged {
        tasks: Vec<Task>,
    },
    /// The effort of the session's later turns, after a `SetEffort`.
    EffortChanged {
        effort: Effort,
    },
}

/// A fleet-unique ULID assigned by the host, represented as its 128 bits.
/// A task keeps it while the agent rewrites its checklist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub u128);

/// A step of an agent's checklist (Claude's `TodoWrite`, Codex's plan), as
/// the host keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub text: String,
    pub status: TaskStatus,
    /// The session whose agent works on it.
    pub holder: SessionId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
}

/// Monotonic per session, including discarded checkpoints; zero is initial.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
)]
pub struct CheckpointId(pub u64);

impl CheckpointId {
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub repository: HostPath,
    pub original: HostPath,
    pub worktree: HostPath,
    pub directory: HostPath,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: CheckpointId,
    pub parent: Option<CheckpointId>,
    /// The existing `UserMessage` event for this turn; None for initial/autonomous turns.
    pub turn_start: Option<HostSeq>,
    pub commit: String,
    /// Opaque provider metadata; clients never present these IDs as controls.
    pub history: HistoryAnchor,
    pub undo: UndoAvailability,
}

/// A point in an agent's own history, by the provider's ids. How an agent
/// anchors its history decides the shape: at the last message a turn ended
/// on, or at each turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryAnchor {
    Fresh,
    /// The conversation through its message `message`, once one is seen.
    Messages {
        conversation: String,
        message: Option<String>,
    },
    /// The conversation as of its turn `turn`, once one is seen.
    Turns {
        conversation: String,
        turn: Option<String>,
    },
    /// A new conversation `target` with `source`'s messages through
    /// `through`. Starting the agent alone may not create the target yet.
    Fork {
        source: String,
        through: String,
        target: String,
    },
    /// Files may have changed before an actual native turn boundary was observed.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UndoAvailability {
    Available,
    Unavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointDiff {
    pub checkpoint: CheckpointId,
    pub parent: CheckpointId,
    pub files: Vec<CheckpointFile>,
    pub extent: DiffExtent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointFile {
    pub path: String,
    pub change: Change,
    pub patch: FilePatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilePatch {
    Text(String),
    Binary,
    TooLarge,
    Unsupported { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffExtent {
    Complete,
    Truncated,
}

/// One question. Both agents take any typed text as an answer, so every
/// question also allows one besides its options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    /// A short label for the question, such as "Color".
    pub header: String,
    pub text: String,
    pub options: Vec<QuestionOption>,
    pub selection: Selection,
}

/// How many options the user may choose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Selection {
    One,
    Many,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
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
    /// A question's answers, one per question.
    Answered {
        answers: Vec<Answer>,
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
    FailedToStart {
        message: String,
    },
    Exited {
        status: String,
    },
    /// The host stopped the agent on purpose, for this reason.
    Stopped {
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::{ClaudeCredential, Credential};

    #[test]
    fn constant_cursor_saturates_without_wrapping() {
        const LAST: super::HostSeq = super::HostSeq(u64::MAX).next();
        assert_eq!(LAST, super::HostSeq(u64::MAX));
    }

    #[test]
    fn durability_v9_bytes_keep_their_original_meaning()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            postcard::from_bytes::<super::Event>(&[1, 1, 1])?,
            super::Event {
                seq: super::HostSeq(1),
                session: super::SessionId(1),
                kind: super::EventKind::SessionReady,
            }
        );
        assert_eq!(
            postcard::from_bytes::<super::CommandResult>(&[0])?,
            super::CommandResult::Accepted
        );
        assert_eq!(
            postcard::from_bytes::<super::CommandResult>(&[3, 3])?,
            super::CommandResult::Rejected(super::Rejection::SessionClosed)
        );
        assert_eq!(
            postcard::to_stdvec(&super::Event {
                seq: super::HostSeq(1),
                session: super::SessionId(1),
                kind: super::EventKind::SessionReady,
            })?,
            Vec::from([1, 1, 1])
        );
        Ok(())
    }

    #[test]
    fn a_base_url_may_not_hide_a_secret() {
        let checked = [
            "https://gateway.example/v1",
            "https://user:pw@gateway.example",
            "https://gateway.example/?key=1",
            "https://gateway.example/#k",
            "ftp://gateway.example",
            "gateway.example",
        ]
        .into_iter()
        .map(|base_url| {
            Credential::Claude(ClaudeCredential::Gateway {
                base_url: String::from(base_url),
                token: String::from("t"),
            })
            .check()
        })
        .collect::<Vec<Result<(), String>>>();
        let hidden = Err(String::from(
            "the base URL may not carry a user, a password, a query or a fragment",
        ));
        assert_eq!(
            checked,
            Vec::from([
                Ok(()),
                hidden.clone(),
                hidden.clone(),
                hidden,
                Err(String::from("the base URL must be http or https")),
                Err(String::from("the base URL: relative URL without a base")),
            ])
        );
    }
}
