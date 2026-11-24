//! The session record: the events a session's log holds, the commands that
//! act on a session, and what the host answers them.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use agents::{
    Answer, ChecklistEntry, ClientName, CloseReason, Decision, Effort, ModelId,
    Output, Question, SubagentActivity, SubagentId, SubagentOutcome,
    ToolCallId, ToolKind, TurnOutcome,
};
use home::{HomeRefusal, IssueId, ProjectId};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::finish::Finished;
use crate::log::HostSeq;
use crate::registry::InstanceId;
use crate::workspace::{Kept, Snapshot, SnapshotDiff, Workspace};

/// A fleet-unique ULID assigned by the host, represented as its 128 bits,
/// whose text, on the client link, in its worktree's and branch's names and in
/// logs, is the `ulid` crate's.
///
/// A human-readable format writes the text, since JSON's numbers are exact
/// only to 2^53; others write the number.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, JsonSchema,
)]
pub struct SessionId(#[schemars(with = "String")] pub u128);

impl SessionId {
    /// The id `text` writes, in either case. The crate drops the bits of a
    /// first digit above 7, so text that doesn't write back is refused.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let id = ulid::Ulid::from_string(text).ok()?;
        let mut buffer = [0; ulid::ULID_LEN];
        id.array_to_str(&mut buffer)
            .eq_ignore_ascii_case(text)
            .then_some(Self(id.0))
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        ulid::Ulid(self.0).fmt(out)
    }
}

impl Serialize for SessionId {
    fn serialize<S: Serializer>(&self, out: S) -> Result<S::Ok, S::Error> {
        if out.is_human_readable() {
            out.collect_str(self)
        } else {
            self.0.serialize(out)
        }
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D: Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        if input.is_human_readable() {
            let text = String::deserialize(input)?;
            Self::parse(&text).ok_or_else(|| {
                de::Error::invalid_value(de::Unexpected::Str(&text), &"a ULID")
            })
        } else {
            u128::deserialize(input).map(Self)
        }
    }
}

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

/// The directory a session starts in and works in, as the host's OS
/// spells it. The client never interprets it: a browser on a phone can't
/// know a Windows path's rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionDirectory(pub String);

impl From<home::CheckoutLocation> for SessionDirectory {
    fn from(checkout: home::CheckoutLocation) -> Self {
        Self(checkout.0)
    }
}

impl From<SessionDirectory> for agents::runtime::Directory {
    fn from(directory: SessionDirectory) -> Self {
        Self(directory.0)
    }
}

/// Where a new session runs: a directory, a project's checkout, or an
/// issue's project's checkout on a branch cut from the issue's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum StartAt {
    Directory(SessionDirectory),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum EventKind {
    /// `model` and `effort` are what the agent runs with; `None` is the
    /// CLI's own default. A `Continue` runs on `instance` again.
    SessionCreated {
        instance: InstanceId,
        directory: SessionDirectory,
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
