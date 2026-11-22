//! The wire protocol between a client and `conductord` (architecture §5): the
//! types and the frame codec. It is the pure core: `no_std`, with the stream
//! I/O in the `net` crate.
//!
//! Every bi-stream starts with one [`Open`] frame from the client. The first
//! stream of a connection must be [`Open::Hello`], answered by one
//! [`HelloReply`]. A [`Open::Command`] stream gets one [`CommandResult`] and
//! closes. A [`Open::Subscribe`] stream then carries [`Feed`] frames, the
//! events and the sign-ins, until either side closes it.
//!
//! Frame bodies are postcard, which is not self-describing. A variant is
//! its index and a field is its position, so a reordered enum or struct
//! decodes as the wrong value with no error, and a variant added at the end
//! fails an old peer mid-stream. So any change to a type reachable from
//! [`Open`], [`HelloReply`], [`CommandResult`] or [`Feed`] bumps [`ALPN`],
//! including a variant added at the end. Renaming a variant or a field
//! doesn't change the bytes and needs no bump.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

mod calendar;
mod credential;
pub mod fleet;
pub mod flow;
mod frame;
mod home;
mod id;
mod instance;
mod issue;
mod sign_in;
mod ticket;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use core::fmt;

use serde::{Deserialize, Serialize};

pub use calendar::civil;
pub use credential::{
    Catalog, Credential, CredentialForm, Field, FieldKind, ProviderInfo,
};
pub use frame::{FrameError, HEADER_LEN, body_len, decode, encode};
pub use home::{Checkout, Cloning, Home, HomeSync, Origin, Project, StartAt};
pub use id::{IssueId, ProjectId};
pub use instance::{
    Account, Instance, InstanceId, InstanceStatus, Instances, Roster,
};
pub use issue::{
    ChecklistChange, ChecklistItem, Datum, Entry, Issue, IssueChange, Item,
    LogEvent, NewIssue, Part, Section,
};
pub use sign_in::{SignIn, SignInCode, SignInStep};
pub use ticket::{AuthToken, Ticket, TicketError, WrongToken};

/// The ALPN of the protocol. It changes with every change to the wire types
/// (see the crate docs), so an old client fails at the handshake instead of
/// mid-stream.
pub const ALPN: &[u8] = b"conductor/37";

/// A logged event or a live update to the host's sign-ins, home or offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Feed {
    Event(Event),
    SignIn(SignIn),
    Home(Home),
    /// The current instance offers, replacing the previous snapshot.
    Instances(Instances),
}

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
    FleetHello(fleet::DeviceHello),
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
    pub instances: Instances,
    /// How each provider presents itself and the credentials it takes.
    /// Unlike `instances` it doesn't change while the host runs.
    pub catalog: Catalog,
    /// `None` on a host given no home. `Feed::Home` brings each change.
    pub home: Option<Home>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Availability {
    Available {
        auth: Auth,
        models: Models,
    },
    /// The host can't run this instance's sessions, for this reason.
    Unavailable {
        reason: String,
    },
}

/// The note to show for an instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderNote {
    Nothing,
    /// The host can't run it, for this reason.
    Unavailable(String),
    /// What the provider's plugin says of it, such as what its agent does
    /// without asking.
    Says(String),
}

impl Availability {
    /// `info` and `availability` are `None` until the host has said.
    #[must_use]
    pub fn note(
        info: Option<&ProviderInfo>,
        availability: Option<&Self>,
    ) -> ProviderNote {
        match (availability, info.and_then(|known| known.note.clone())) {
            (Some(Self::Unavailable { reason }), _) => {
                ProviderNote::Unavailable(reason.clone())
            }
            (Some(Self::Available { .. }) | None, Some(text)) => {
                ProviderNote::Says(text)
            }
            (Some(Self::Available { .. }) | None, None) => {
                ProviderNote::Nothing
            }
        }
    }

    /// The models the host offers when it can run the instance.
    #[must_use]
    pub fn models(&self) -> Models {
        match self {
            Self::Available { models, .. } => models.clone(),
            Self::Unavailable { .. } => Models::none(),
        }
    }
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
    /// The label of the model picker's first entry, which sends no model.
    #[must_use]
    pub fn default_label(&self) -> String {
        self.default.as_ref().map_or_else(
            || String::from("Default"),
            |id| {
                format!(
                    "Default ({})",
                    self.find(id)
                        .map_or(id.0.as_str(), |choice| choice.name.as_str())
                )
            },
        )
    }

    /// The efforts `model` takes (`None`: the default model), lowest first.
    #[must_use]
    pub fn efforts(&self, model: Option<&ModelId>) -> Vec<Effort> {
        model
            .or(self.default.as_ref())
            .and_then(|id| self.find(id))
            .map_or(Vec::new(), |choice| choice.efforts.clone())
    }

    /// The effort to keep when the model changes to `model`: `effort` if the
    /// model takes it, else the model's default.
    #[must_use]
    pub fn kept_effort(
        &self,
        model: Option<&ModelId>,
        effort: Option<Effort>,
    ) -> Option<Effort> {
        effort.filter(|kept| self.efforts(model).contains(kept))
    }

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

/// How an instance's CLI reaches its model. It names the kind and never
/// carries a secret: credentials are write-only (§6.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Auth {
    /// A credential the host stores, of this kind, reaching this URL if
    /// its form has one ([`ProviderInfo::auth`]).
    Stored { kind: String, url: Option<String> },
    /// The CLI's own sign-in in the instance's private home, from the app
    /// (`StartSignIn`) or on the host (`conductord login <instance>`).
    SignedIn,
    /// A CLI the host was started with (`--cli <provider>=<path>`),
    /// which signs in however it does.
    Unmanaged,
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

    /// What the host's command line and data dir call it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// The provider called `name`.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|provider| provider.name() == name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub id: CommandId,
    pub body: CommandBody,
}

/// Every session command after `StartSession` names the session it acts
/// on, and every instance command the instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandBody {
    /// `None` takes the instance's default model, or the CLI's default
    /// effort for the model.
    /// A session in a project starts in its first checkout on the host.
    StartSession {
        instance: InstanceId,
        at: StartAt,
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
    /// Stores a credential of `provider` on the host. The account its form
    /// names makes the instance: one that has it takes the credential in
    /// place of its own, and otherwise the host adds the instance and
    /// starts its CLI before it answers. The host never sends it back.
    /// Like every command it needs a client admitted by the ticket's
    /// token, which device certificates replace with the fleet (§6.1).
    SetCredential {
        provider: Provider,
        credential: Credential,
    },
    /// A bounded diff between this snapshot and its predecessor.
    GetDiff {
        session: SessionId,
        snapshot: HostSeq,
    },
    /// Sets the effort of the session's later turns, one its model lists.
    /// A running turn keeps its effort: the next turn the user starts, or
    /// a `Continue`, takes the new one.
    SetEffort {
        session: SessionId,
        effort: Effort,
    },
    /// Refused while a session runs on the instance. Its closed sessions
    /// stay, and can't continue. A provider's own sign-in stays; on a host
    /// whose store doesn't open, removing it moves the store aside.
    RemoveInstance {
        instance: InstanceId,
    },
    /// Runs the CLI's own sign-in in the instance's home; every client
    /// follows it as `Feed::SignIn`. Refused while one runs on it. Once
    /// the CLI names the account it signed in as, the instance is that
    /// account's, and its provider has a new own sign-in.
    StartSignIn {
        instance: InstanceId,
    },
    /// The code the sign-in page gave, for a sign-in whose step asks for
    /// one to be pasted.
    SubmitSignInCode {
        instance: InstanceId,
        code: SignInCode,
    },
    /// Stops the instance's sign-in, if one runs.
    CancelSignIn {
        instance: InstanceId,
    },
    /// Pulls the home now. `Accepted` once it has; how it went is in the
    /// home's `sync`.
    SyncHome,
    /// Clones the project into the host's data dir, for a project with no
    /// checkout here. `Accepted` once it has; how it went is in the
    /// project's `cloning`.
    CloneProject {
        project: ProjectId,
    },
    /// Stops the session's agent between turns, commits what its workspace
    /// holds onto its branch, pushes that branch to the checkout's
    /// `origin`, and removes the workspace. `SessionRemoved` follows in the
    /// log; the session takes no command after it.
    RemoveSession {
        session: SessionId,
    },
    /// Opens an issue, as `home issue new` does. Answers `IssueWritten`
    /// once the host has written it to its clone of the home; pushing it
    /// follows, and how that went is in the home's `sync`.
    CreateIssue(NewIssue),
    /// Answers `IssueWritten` once written, like `CreateIssue`.
    ChangeIssue {
        issue: IssueId,
        change: IssueChange,
    },
    /// Finishes a session on an issue between turns. `Accepted` at once;
    /// the host then asks a running agent for a summary and stops it, adds
    /// the summary to the issue's notes, lands the session's branch on the
    /// issue's, pushes that, and moves the issue to `review` when it
    /// gained commits. `SessionFinished` says how it went.
    FinishSession {
        session: SessionId,
    },
    PushPublicKey,
    RegisterPush(PushSubscription),
    FleetStatus,
    FleetInvite,
    FleetRevoke(fleet::NodeId),
    FleetApprove(fleet::PairRequest),
    FleetAdmit {
        ticket: Ticket,
        name: String,
    },
    /// Runs the provider's live credential setup through its own instance.
    StartCredentialSignIn {
        instance: InstanceId,
        kind: String,
        label: String,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushSubscription {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

impl fmt::Debug for PushSubscription {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str("PushSubscription")
    }
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
    Rejected(Rejection),
    Diff(SnapshotDiff),
    /// The issue a `CreateIssue` or `ChangeIssue` wrote, and what the
    /// flow warns of it.
    IssueWritten {
        issue: IssueId,
        warnings: Vec<String>,
    },
    PushPublicKey(String),
    PushUnavailable,
    FleetStatus(fleet::SignedSnapshot),
    FleetInvite(fleet::Invite),
    FleetPairing(Box<fleet::Pairing>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rejection {
    NoSuchDirectory,
    InstanceUnavailable {
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
    SnapshotUnavailable,
    WorkspaceFailure {
        reason: String,
    },
    UnknownInstance,
    /// The instance can't be removed while sessions run on it.
    InstanceInUse {
        sessions: u64,
    },
    SignInRunning,
    /// No sign-in runs on the instance, or it takes no code now.
    NoCodeAsked,
    /// The host was given no home.
    NoHome,
    UnknownProject,
    /// The project has no checkout on the host.
    NotCheckedOut,
    /// The project is local only: there is no remote to clone.
    NoRemote,
    SessionRemoved,
    UnknownIssue,
    /// The home can't be written now, for this reason.
    IssueNotSaved {
        reason: String,
    },
    /// The flow refuses the edit, for this reason, as the `home` command
    /// line would.
    IssueRefused {
        reason: String,
    },
    /// The instance has this session on the issue already: one session
    /// per instance and issue.
    IssueTaken {
        session: SessionId,
    },
    /// The session works on no issue, so there is nothing to finish it on.
    NotOnIssue,
    /// The session is finishing, or has done nothing since it finished.
    AlreadyFinished,
    FleetUnavailable,
}

impl Rejection {
    /// Why the host refused the command, for a person to read.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::FleetUnavailable => {
                String::from("fleet changes need the root host")
            }
            Self::NoSuchDirectory => {
                String::from("no directory at that path on the host")
            }
            Self::InstanceUnavailable { reason } => {
                format!("that instance is unavailable: {reason}")
            }
            Self::UnknownInstance => {
                String::from("the host has no such instance")
            }
            Self::InstanceInUse { sessions: 1 } => String::from(
                "a session is running on it; stop it before removing the instance",
            ),
            Self::InstanceInUse { sessions } => format!(
                "{sessions} sessions are running on it; stop them before removing the instance"
            ),
            Self::UnknownSession => String::from("no session with that id"),
            Self::SessionClosed => String::from("the session has closed"),
            Self::SessionRemoved => {
                String::from("the session and its workspace were removed")
            }
            Self::TurnInProgress => String::from("a turn is already running"),
            Self::NoTurnRunning => String::from("no turn is running"),
            Self::UnknownRequest => String::from("no request with that id"),
            Self::AlreadyResolved { by } => {
                format!("already resolved by {}", by.0)
            }
            Self::CredentialNotSaved { reason } => {
                format!("the host could not store the change: {reason}")
            }
            Self::InvalidCredential { reason } => {
                format!("the credential is not valid: {reason}")
            }
            Self::NotGitRepository => {
                String::from("choose a Git repository with a committed HEAD")
            }
            Self::SnapshotUnavailable => {
                String::from("this snapshot has no available comparison")
            }
            Self::SignInRunning => String::from(
                "a sign-in is already running on it; cancel it to start again",
            ),
            Self::NoCodeAsked => {
                String::from("its sign-in isn't waiting for a code")
            }
            Self::NoHome => String::from(
                "the host has no home; start it with --home <remote>",
            ),
            Self::UnknownProject => {
                String::from("the host's home lists no such project")
            }
            Self::NotCheckedOut => String::from(
                "the project isn't checked out on the host; clone it there first",
            ),
            Self::NoRemote => String::from(
                "the project is local only, so there is nothing to clone",
            ),
            Self::UnknownIssue => {
                String::from("the host's home lists no such issue")
            }
            Self::IssueNotSaved { reason } => {
                format!("the issue was not saved: {reason}")
            }
            Self::NotOnIssue => String::from("the session works on no issue"),
            Self::AlreadyFinished => String::from(
                "the session is finishing, or has done nothing since it finished",
            ),
            Self::IssueTaken { .. } => String::from(
                "that instance has a session on the issue already; continue it, or remove it first",
            ),
            Self::NotOffered { reason }
            | Self::IssueRefused { reason }
            | Self::WrongReply { reason }
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
    /// CLI's own default. A `Continue` runs on `instance` again.
    SessionCreated {
        instance: InstanceId,
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
    /// The session's managed worktree, logged before its first snapshot.
    WorkspaceCreated {
        workspace: Workspace,
    },
    SnapshotRecorded {
        snapshot: Snapshot,
    },
    NativeTurnStarted {
        turn_start: Option<HostSeq>,
    },
    ChecklistChanged {
        checklist: Vec<ChecklistEntry>,
    },
    /// The effort of the session's later turns, after a `SetEffort`.
    EffortChanged {
        effort: Effort,
    },
    /// A sub-agent belongs to its session: its events are the session's, in
    /// the session's log, and it has no session of its own.
    Subagent {
        agent: SubagentId,
        event: SubagentEvent,
    },
    /// The session's workspace is gone and its branch kept as `kept` says;
    /// `None` when it had no workspace. Nothing follows it.
    SessionRemoved {
        kept: Option<Kept>,
    },
    /// The session works on this issue, on a branch cut from the issue's.
    /// It follows `SessionCreated`.
    OnIssue {
        issue: IssueId,
    },
    /// What finishing the session did.
    SessionFinished {
        finished: Finished,
    },
    /// Finishing the session has begun; `SessionFinished` ends it.
    FinishStarted,
    /// A restarted host can resume the interrupted landing.
    FinishInterrupted,
}

/// Where a session on an issue stands with finishing, as its events say:
/// a message after a finish makes it ready again, while its own summary
/// turn doesn't.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FinishState {
    #[default]
    Ready,
    Finishing,
    Retrying,
    Done,
}

impl FinishState {
    /// The state after `kind`.
    #[must_use]
    pub const fn after(self, kind: &EventKind) -> Self {
        match (self, kind) {
            (_, EventKind::FinishStarted) => Self::Finishing,
            (Self::Finishing, EventKind::FinishInterrupted) => Self::Retrying,
            (_, EventKind::SessionFinished { .. }) => Self::Done,
            (Self::Done, EventKind::UserMessage { .. }) => Self::Ready,
            (state, _) => state,
        }
    }
}

/// How finishing a session on an issue went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Finished {
    /// The session's branch landed on the issue's with this many commits
    /// the issue's lacked, and the issue's branch was kept as `kept` says.
    Landed { commits: u64, kept: Kept },
    /// Nothing landed, for this reason; the session's branch stays.
    Failed { reason: String },
}

impl EventKind {
    /// The request created by this event, whether from the session or a subagent.
    #[must_use]
    pub const fn opened_request(&self) -> Option<RequestId> {
        match self {
            Self::RequestOpened { request, .. }
            | Self::QuestionAsked { request, .. }
            | Self::Subagent {
                event:
                    SubagentEvent::RequestOpened { request, .. }
                    | SubagentEvent::QuestionAsked { request, .. },
                ..
            } => Some(*request),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubagentEvent {
    /// The agent's tool call `call` started it. A sub-agent the agent
    /// resumes starts again under the same id.
    Started {
        call: ToolCallId,
        description: String,
    },
    Activity(SubagentActivity),
    /// `summary` is its final report, when its provider gives one.
    Ended {
        outcome: SubagentOutcome,
        summary: Option<String>,
    },
    Kind {
        kind: String,
    },
    ChecklistChanged {
        checklist: Vec<ChecklistEntry>,
    },
    RequestOpened {
        request: RequestId,
        call: ToolCallId,
        tool: ToolKind,
        reason: Option<String>,
        for_session: Option<String>,
    },
    QuestionAsked {
        request: RequestId,
        questions: Vec<Question>,
    },
}

/// A sub-agent's id as its provider gave it (Claude's `task_id`).
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct SubagentId(pub String);

/// One entry of a sub-agent's own transcript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubagentActivity {
    Message { text: String },
    Reasoning { text: String },
    ToolCall { call: ToolCallId, phase: ToolPhase },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubagentOutcome {
    Finished,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistEntry {
    pub text: String,
    pub status: ChecklistStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChecklistStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub repository: HostPath,
    pub original: HostPath,
    pub worktree: HostPath,
    pub directory: HostPath,
    pub branch: String,
}

/// Where a removed session's branch was kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kept {
    /// In the checkout only: it has no `origin`.
    Local,
    Pushed {
        remote: String,
    },
    /// In the checkout only, since the push failed.
    PushFailed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// The existing `UserMessage` event for this turn; None for initial/autonomous turns.
    pub turn_start: Option<HostSeq>,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotDiff {
    pub snapshot: HostSeq,
    pub files: Vec<SnapshotFile>,
    pub extent: DiffExtent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotFile {
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
    use super::{
        Auth, Availability, Effort, ModelChoice, ModelId, Models, ProviderInfo,
        ProviderNote,
    };

    #[test]
    fn the_form_says_why_a_provider_is_unavailable_else_its_plugin_s_note() {
        let unavailable = Availability::Unavailable {
            reason: String::from("not on PATH"),
        };
        let available = Availability::Available {
            auth: Auth::SignedIn,
            models: Models::none(),
        };
        let info = |note: Option<&str>| ProviderInfo {
            label: String::from("Agent"),
            note: note.map(String::from),
            credentials: Vec::new(),
        };
        let noted = info(Some("writes unasked"));
        let quiet = info(None);
        assert_eq!(
            [
                (Some(&noted), Some(&unavailable)),
                (Some(&quiet), Some(&unavailable)),
                (Some(&noted), Some(&available)),
                (Some(&noted), None),
                (Some(&quiet), Some(&available)),
                (None, None),
            ]
            .map(|(info, availability)| Availability::note(info, availability)),
            [
                ProviderNote::Unavailable(String::from("not on PATH")),
                ProviderNote::Unavailable(String::from("not on PATH")),
                ProviderNote::Says(String::from("writes unasked")),
                ProviderNote::Says(String::from("writes unasked")),
                ProviderNote::Nothing,
                ProviderNote::Nothing,
            ]
        );
    }

    #[test]
    fn the_pickers_offer_the_default_then_the_models_and_their_efforts() {
        let models = Models {
            choices: Vec::from([
                ModelChoice {
                    id: ModelId(String::from("opus")),
                    name: String::from("Opus"),
                    description: String::from("best"),
                    efforts: Vec::from([Effort::Low, Effort::Max]),
                },
                ModelChoice {
                    id: ModelId(String::from("haiku")),
                    name: String::from("Haiku"),
                    description: String::from("fast"),
                    efforts: Vec::from([Effort::High]),
                },
            ]),
            default: Some(ModelId(String::from("opus"))),
        };
        let haiku = ModelId(String::from("haiku"));
        assert_eq!(
            (
                models.default_label(),
                Models::none().default_label(),
                models.efforts(None),
                models.efforts(Some(&haiku)),
                Models::none().efforts(None),
                models.kept_effort(Some(&haiku), Some(Effort::Max)),
                models.kept_effort(Some(&haiku), Some(Effort::High)),
            ),
            (
                String::from("Default (Opus)"),
                String::from("Default"),
                Vec::from([Effort::Low, Effort::Max]),
                Vec::from([Effort::High]),
                Vec::new(),
                None,
                Some(Effort::High),
            )
        );
    }

    #[test]
    fn constant_cursor_saturates_without_wrapping() {
        const LAST: super::HostSeq = super::HostSeq(u64::MAX).next();
        assert_eq!(LAST, super::HostSeq(u64::MAX));
    }

    #[test]
    fn each_provider_is_found_by_its_name() {
        assert_eq!(
            super::Provider::ALL
                .map(|provider| super::Provider::named(provider.name())),
            super::Provider::ALL.map(Some)
        );
        assert_eq!(
            super::Provider::ALL.map(super::Provider::name),
            ["claude", "codex"]
        );
        assert_eq!(super::Provider::named("gemini"), None);
    }

    #[test]
    fn stored_events_and_receipts_use_the_current_format()
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
            postcard::from_bytes::<super::CommandResult>(&[2, 3])?,
            super::CommandResult::Rejected(super::Rejection::SessionClosed)
        );
        assert_eq!(
            postcard::to_allocvec(&super::Event {
                seq: super::HostSeq(1),
                session: super::SessionId(1),
                kind: super::EventKind::SessionReady,
            })?,
            Vec::from([1, 1, 1])
        );
        Ok(())
    }
    #[test]
    fn each_effort_reads_back_from_its_word() {
        use super::Effort;
        let all = [
            Effort::Low,
            Effort::Medium,
            Effort::High,
            Effort::XHigh,
            Effort::Max,
            Effort::Ultra,
        ];
        assert_eq!(
            all.iter().map(Effort::wire).collect::<Vec<_>>(),
            ["low", "medium", "high", "xhigh", "max", "ultra"]
        );
        assert_eq!(
            all.map(|effort| Effort::parse(effort.wire())),
            all.map(Some)
        );
        assert_eq!(Effort::parse("minimal"), None);
    }

    #[test]
    fn a_model_is_found_by_its_id() {
        use super::{ModelChoice, ModelId, Models};
        let choice = |id: &str| ModelChoice {
            id: ModelId(String::from(id)),
            name: String::from(id),
            description: String::new(),
            efforts: Vec::new(),
        };
        let models = Models {
            choices: Vec::from([choice("opus"), choice("haiku")]),
            default: None,
        };
        assert_eq!(
            models.find(&ModelId(String::from("haiku"))),
            Some(&choice("haiku"))
        );
        assert_eq!(models.find(&ModelId(String::from("sonnet"))), None);
    }

    #[test]
    fn a_rejection_says_why_for_a_person() {
        assert_eq!(
            super::Rejection::NoSuchDirectory.label(),
            "no directory at that path on the host"
        );
    }

    #[test]
    fn a_provider_is_labelled_by_its_plugin_else_by_its_name() {
        use super::{Catalog, Provider, ProviderInfo};
        let info = ProviderInfo {
            label: String::from("Claude Code"),
            note: None,
            credentials: Vec::new(),
        };
        let catalog = Catalog(alloc::collections::BTreeMap::from([(
            Provider::Claude,
            info.clone(),
        )]));
        assert_eq!(catalog.of(Provider::Claude), Some(&info));
        assert_eq!(catalog.of(Provider::Codex), None);
        assert_eq!(
            [
                catalog.label(Provider::Claude),
                catalog.label(Provider::Codex)
            ],
            [String::from("Claude Code"), String::from("codex")]
        );
    }

    #[test]
    fn an_instance_is_shown_and_found_by_its_provider_and_account() {
        use super::{
            Availability, InstanceId, InstanceStatus, Instances, Provider,
        };
        let own = InstanceId::own(Provider::Claude);
        let named = InstanceId::named(Provider::Codex, "me@example.com");
        assert_eq!(
            [format!("{own}"), format!("{named}")],
            [String::from("claude"), String::from("codex me@example.com")]
        );
        let available = InstanceStatus {
            availability: Availability::Unavailable {
                reason: String::from("signed out"),
            },
        };
        let instances = Instances(alloc::collections::BTreeMap::from([(
            own.clone(),
            available.clone(),
        )]));
        assert_eq!(instances.of(&own), Some(&available));
        assert_eq!(instances.availability(&own), Some(&available.availability));
        assert_eq!(instances.availability(&named), None);
        let roster = super::Roster(alloc::collections::BTreeMap::from([(
            named.clone(),
            super::Instance::default(),
        )]));
        assert_eq!(roster.of(&named), Some(&super::Instance::default()));
        assert_eq!(roster.of(&own), None);
    }
}
