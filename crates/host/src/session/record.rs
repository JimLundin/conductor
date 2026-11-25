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
use crate::workspace::{Snapshot, SnapshotDiff, Workspace};
use vcs::Kept;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, JsonSchema,
)]
pub struct SessionId(#[schemars(with = "String")] pub u128);

impl SessionId {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum StartAt {
    Directory(SessionDirectory),
    Project(ProjectId),
    Issue(IssueId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SessionCommand {
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
    Continue {
        session: SessionId,
    },
    Resolve {
        session: SessionId,
        request: RequestId,
        decision: Decision,
    },
    Answer {
        session: SessionId,
        request: RequestId,
        answers: Vec<Answer>,
    },
    GetDiff {
        session: SessionId,
        snapshot: HostSeq,
    },
    SetEffort {
        session: SessionId,
        effort: Effort,
    },
    Remove {
        session: SessionId,
    },
    Finish {
        session: SessionId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SessionResult {
    Accepted,
    Started { session: SessionId },
    Refused(SessionRefusal),
    Diff(SnapshotDiff),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SessionRefusal {
    NoSuchDirectory,
    InstanceUnavailable { reason: String },
    UnknownSession,
    SessionClosed,
    NotOffered { reason: String },
    TurnInProgress,
    NoTurnRunning,
    UnknownRequest,
    AlreadyResolved { by: ClientName },
    WrongReply { reason: String },
    NotGitRepository,
    SnapshotUnavailable,
    WorkspaceFailure { reason: String },
    UnknownInstance,
    SessionRemoved,
    Home(HomeRefusal),
    IssueTaken { session: SessionId },
    NotOnIssue,
    AlreadyFinished,
}

impl SessionRefusal {
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
    SessionCreated {
        instance: InstanceId,
        directory: SessionDirectory,
        by: ClientName,
        model: Option<ModelId>,
        effort: Option<Effort>,
    },
    SessionReady,
    SessionClosed {
        reason: CloseReason,
    },
    UserMessage {
        text: String,
        by: ClientName,
    },
    Agent(Output),
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
    RequestResolved {
        request: RequestId,
        resolution: Resolution,
    },
    TurnCompleted {
        outcome: TurnOutcome,
    },
    Error {
        message: String,
    },
    ModelChanged {
        model: Option<ModelId>,
        effort: Option<Effort>,
        reason: String,
    },
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
    EffortChanged {
        effort: Effort,
    },
    Subagent {
        agent: SubagentId,
        event: SubagentEvent,
    },
    SessionRemoved {
        kept: Option<Kept>,
    },
    OnIssue {
        issue: IssueId,
    },
    SessionFinished {
        finished: Finished,
    },
    FinishStarted,
    FinishInterrupted,
}

impl EventKind {
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
    Started {
        call: ToolCallId,
        description: String,
    },
    Activity(SubagentActivity),
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
    Answered {
        answers: Vec<Answer>,
        by: ClientName,
    },
    Withdrawn,
}
