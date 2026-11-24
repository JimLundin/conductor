//! The domains' types that have yet to move to the crates that own them
//! (ADR 0003): the client link, `link`, composes them into its frames. It
//! is a pure core, `no_std`.
//!
//! The client link is JSON, so a change to a type it reaches bumps
//! `link::ALPN`. Host stores and traffic between hosts are postcard, where
//! a variant is its index and a field its position, so there a reordered
//! type decodes as the wrong value with no error.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod fleet;
mod instance;
mod json;
mod push;
mod sign_in;
mod ticket;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use home::{HomeRefusal, IssueId, ProjectId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// The agent vocabulary, which `agents` owns, until each type's users name
// it there.
pub use agents::{
    Account, Answer, Auth, Change, ChecklistEntry, ChecklistStatus, ClientName,
    CloseReason, Credential, CredentialForm, Decision, Effort, Field,
    FieldKind, FileChange, ModelChoice, ModelId, Models, Output, Provider,
    ProviderInfo, Question, QuestionOption, Selection, SignInCode,
    SubagentActivity, SubagentId, SubagentOutcome, ToolCallId, ToolKind,
    ToolOutcome, ToolPhase, TurnOutcome,
};
pub use instance::{
    Instance, InstanceCommand, InstanceId, InstanceRefusal, InstanceResult,
    InstanceStatus, Instances, Roster,
};
pub use push::{PushCommand, PushResult, PushSubscription};
pub use sign_in::{
    SignIn, SignInCommand, SignInRefusal, SignInResult, SignInStep,
};
pub use ticket::{AuthToken, Ticket, TicketError, WrongToken};

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
    JsonSchema,
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
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct Epoch(
    #[serde(with = "json::decimal")]
    #[schemars(with = "String")]
    pub u64,
);

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
    JsonSchema,
)]
pub struct CommandId(
    #[serde(with = "json::ulid")]
    #[schemars(with = "String")]
    pub u128,
);

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
    JsonSchema,
)]
pub struct RequestId(pub u64);

impl RequestId {
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

/// What the host's plugins describe, by provider.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub struct Catalog(pub BTreeMap<Provider, ProviderInfo>);

impl Catalog {
    /// `None` when the host runs no plugin for `provider`.
    #[must_use]
    pub fn of(&self, provider: &Provider) -> Option<&ProviderInfo> {
        self.0.get(provider)
    }

    /// What the app calls `provider`: its plugin's label, else its name.
    #[must_use]
    pub fn label(&self, provider: &Provider) -> String {
        self.of(provider).map_or_else(
            || String::from(provider.name()),
            |info| info.label.clone(),
        )
    }
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
    JsonSchema,
)]
pub struct SessionId(
    #[serde(with = "json::ulid")]
    #[schemars(with = "String")]
    pub u128,
);

/// A path on the host, as the host's OS spells it. The client never
/// interprets it: a browser on a phone can't know a Windows path's rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostPath(pub String);

/// Where a new session runs: a directory, a project's checkout, or an
/// issue's project's checkout on a branch cut from the issue's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum StartAt {
    Directory(HostPath),
    Project(ProjectId),
    Issue(IssueId),
}

/// Every command after `Start` names the session it acts on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SessionCommand {
    /// `None` takes the instance's default model, or the CLI's default
    /// effort for the model.
    /// A session in a project starts in its first checkout on the host.
    Start {
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
    /// Stops the session's agent between turns, commits what its workspace
    /// holds onto its branch, pushes that branch to the checkout's
    /// `origin`, and removes the workspace. `SessionRemoved` follows in the
    /// log; the session takes no command after it.
    Remove {
        session: SessionId,
    },
    /// Finishes a session on an issue between turns. `Accepted` at once;
    /// the host then asks a running agent for a summary and stops it, adds
    /// the summary to the issue's notes, lands the session's branch on the
    /// issue's, pushes that, and moves the issue to `review` when it
    /// gained commits. `SessionFinished` says how it went.
    Finish {
        session: SessionId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SessionResult {
    Accepted,
    /// The answer to `Start`. `SessionCreated` follows in the log.
    Started {
        session: SessionId,
    },
    Refused(SessionRefusal),
    Diff(SnapshotDiff),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SessionRefusal {
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
    NotGitRepository,
    SnapshotUnavailable,
    WorkspaceFailure {
        reason: String,
    },
    UnknownInstance,
    SessionRemoved,
    /// The session can't start where it was asked to: the home refuses
    /// the project or issue.
    Home(HomeRefusal),
    /// The instance has this session on the issue already: one session
    /// per instance and issue.
    IssueTaken {
        session: SessionId,
    },
    /// The session works on no issue, so there is nothing to finish it on.
    NotOnIssue,
    /// The session is finishing, or has done nothing since it finished.
    AlreadyFinished,
}

impl SessionRefusal {
    /// Why the host refused the command, for a person to read.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::NoSuchDirectory => {
                String::from("no directory at that path on the host")
            }
            Self::InstanceUnavailable { reason } => {
                format!("that instance is unavailable: {reason}")
            }
            Self::UnknownInstance => {
                String::from("the host has no such instance")
            }
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
            Self::NotGitRepository => {
                String::from("choose a Git repository with a committed HEAD")
            }
            Self::SnapshotUnavailable => {
                String::from("this snapshot has no available comparison")
            }
            Self::Home(refusal) => refusal.label(),
            Self::NotOnIssue => String::from("the session works on no issue"),
            Self::AlreadyFinished => String::from(
                "the session is finishing, or has done nothing since it finished",
            ),
            Self::IssueTaken { .. } => String::from(
                "that instance has a session on the issue already; continue it, or remove it first",
            ),
            Self::NotOffered { reason }
            | Self::WrongReply { reason }
            | Self::WorkspaceFailure { reason } => reason.clone(),
        }
    }
}

/// Every event names its session. A stored event has a seq of its own; an
/// `AssistantDelta` repeats the last one's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    pub seq: HostSeq,
    pub session: SessionId,
    pub kind: EventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    /// What the agent produced. An `AssistantDelta` or a `ReasoningDelta`
    /// is live only: the host stores none, so a subscription never replays
    /// one, and the message or reasoning that follows replaces them. It
    /// carries the seq of the stored event before it, which may be another
    /// session's, so a client that has applied that seq takes it and one
    /// that hasn't sees a gap.
    Agent(Output),
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
    /// Something went wrong in the host, such as a snapshot or a removal,
    /// that did not end the session. An error the agent reports is its
    /// `Output::Error`.
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Workspace {
    pub repository: HostPath,
    pub original: HostPath,
    pub worktree: HostPath,
    pub directory: HostPath,
    pub branch: String,
}

/// Where a removed session's branch was kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Snapshot {
    /// The existing `UserMessage` event for this turn; None for initial/autonomous turns.
    pub turn_start: Option<HostSeq>,
    pub commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotDiff {
    pub snapshot: HostSeq,
    pub files: Vec<SnapshotFile>,
    pub extent: DiffExtent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SnapshotFile {
    pub path: String,
    pub change: Change,
    pub patch: FilePatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum FilePatch {
    Text(String),
    Binary,
    TooLarge,
    Unsupported { reason: String },
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum DiffExtent {
    Complete,
    Truncated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

#[cfg(test)]
mod tests {
    use super::{Auth, Availability, Models, ProviderInfo, ProviderNote};

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
    fn constant_cursor_saturates_without_wrapping() {
        const LAST: super::HostSeq = super::HostSeq(u64::MAX).next();
        assert_eq!(LAST, super::HostSeq(u64::MAX));
    }

    #[test]
    fn stored_events_and_rosters_use_the_current_format()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            postcard::from_bytes::<super::Event>(&[1, 1, 1])?,
            super::Event {
                seq: super::HostSeq(1),
                session: super::SessionId(1),
                kind: super::EventKind::SessionReady,
            }
        );
        let roster = super::Roster(alloc::collections::BTreeMap::from([(
            super::InstanceId::own(super::Provider::new("codex")),
            super::Instance::default(),
        )]));
        let bytes = postcard::to_allocvec(&roster)?;
        assert_eq!(bytes, b"\x01\x05codex\x00\x00\x00\x00");
        assert_eq!(postcard::from_bytes::<super::Roster>(&bytes)?, roster);
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
    fn a_rejection_says_why_for_a_person() {
        assert_eq!(
            super::SessionRefusal::NoSuchDirectory.label(),
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
            Provider::new("claude"),
            info.clone(),
        )]));
        assert_eq!(catalog.of(&Provider::new("claude")), Some(&info));
        assert_eq!(catalog.of(&Provider::new("codex")), None);
        assert_eq!(
            [
                catalog.label(&Provider::new("claude")),
                catalog.label(&Provider::new("codex"))
            ],
            [String::from("Claude Code"), String::from("codex")]
        );
    }

    #[test]
    fn an_instance_is_shown_and_found_by_its_provider_and_account() {
        use super::{
            Availability, InstanceId, InstanceStatus, Instances, Provider,
        };
        let own = InstanceId::own(Provider::new("claude"));
        let named = InstanceId::named(Provider::new("codex"), "me@example.com");
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
