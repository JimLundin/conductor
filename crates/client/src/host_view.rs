//! A host's sessions folded from its events: what the UI renders. No UI
//! types, so it is unit-testable and shared by every client target.

use crate::prelude::*;
use alloc::collections::BTreeMap;
use core::cmp::Reverse;

use proto::{
    ChecklistEntry, ClientName, CloseReason, Effort, Event, EventKind,
    FinishState, Finished, HostPath, HostSeq, InstanceId, IssueId, ModelId,
    Question, RequestId, Resolution, SessionId, SubagentActivity,
    SubagentEvent, SubagentId, SubagentOutcome, ToolCallId, ToolKind,
    ToolOutcome, ToolPhase, TurnOutcome, Workspace,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostView {
    pub sessions: BTreeMap<SessionId, SessionView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    pub instance: InstanceId,
    pub directory: HostPath,
    /// What the agent runs with; `None` is the CLI's own default.
    pub model: Option<ModelId>,
    pub effort: Option<Effort>,
    status: SessionStatus,
    pub issue: Option<IssueId>,
    finish: FinishState,
    pub checklist: Vec<ChecklistEntry>,
    pub workspace: Option<Workspace>,
    pub snapshots: Vec<TurnSnapshot>,
    turn_starts: BTreeMap<HostSeq, usize>,
    pub items: Vec<Item>,
    /// The agents the session's agent started, in the order they started.
    pub subagents: Vec<Subagent>,
    /// The last event that touched this session.
    pub activity: HostSeq,
}

/// A sub-agent: a child of its session, shown under the tool call that
/// started it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subagent {
    pub id: SubagentId,
    pub call: ToolCallId,
    pub description: String,
    pub kind: Option<String>,
    pub checklist: Vec<ChecklistEntry>,
    pub status: SubagentStatus,
    /// Its own transcript: replies, reasoning and tool calls, and its final
    /// report as its last reply.
    pub items: Vec<Item>,
}

/// Derived from the events, never sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentStatus {
    Running,
    Ended(SubagentOutcome),
    /// The session closed while it ran.
    Stopped,
}

impl SubagentStatus {
    /// The status in one word, for the UI's text and its `data-status`.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Ended(SubagentOutcome::Finished) => "finished",
            Self::Ended(SubagentOutcome::Failed) => "failed",
            Self::Ended(SubagentOutcome::Stopped) | Self::Stopped => "stopped",
        }
    }
}

/// A completed turn and its transcript boundaries; provider identifiers stay opaque.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnSnapshot {
    pub snapshot: HostSeq,
    pub start: usize,
    pub end: usize,
}

/// A user turn's transcript, with its snapshot once the host has saved it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletedTurn<'a> {
    pub items: &'a [Item],
    pub outcome: &'a TurnOutcome,
    pub snapshot: Option<HostSeq>,
}

/// Derived from the events, never sent.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionStatus {
    Starting,
    Idle,
    Running,
    Closed(CloseReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    User {
        text: String,
        by: ClientName,
    },
    Reply {
        text: String,
        state: ReplyState,
    },
    /// The agent's reasoning, as its provider summarizes it. It streams
    /// like a reply.
    Reasoning {
        text: String,
        state: ReplyState,
    },
    /// A tool call, with the approval it asked for, if any.
    Tool {
        call: ToolCallId,
        tool: ToolKind,
        state: ToolState,
        approval: Approval,
    },
    /// Questions the agent asked, and their answers once given.
    Question {
        request: RequestId,
        questions: Vec<Question>,
        state: RequestState,
    },
    TurnCompleted(TurnOutcome),
    /// The agent started again after it had closed for this reason.
    Continued(CloseReason),
    /// The model changed, for this reason.
    ModelChanged(String),
    /// Finishing the session on its issue began.
    Finishing,
    /// The session was finished on its issue.
    Finished(Finished),
    Error(String),
}

/// A reply or reasoning streams until its final text arrives or something
/// else follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyState {
    Streaming,
    Done,
}

impl ReplyState {
    /// The class the transcript draws a reply with.
    #[must_use]
    pub const fn class(self) -> &'static str {
        match self {
            Self::Done => "reply",
            Self::Streaming => "reply streaming",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Finished(ToolOutcome),
    /// The turn or the session ended before the call finished.
    Stopped,
}

impl ToolState {
    /// How the transcript names the call's state.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Finished(ToolOutcome::Succeeded) => "succeeded",
            Self::Finished(ToolOutcome::Failed) => "failed",
            Self::Finished(ToolOutcome::Declined) => "declined",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Approval {
    NotAsked,
    Asked {
        request: RequestId,
        /// The agent's own words for why it asks, if it gave any.
        reason: Option<String>,
        /// What approving for the session allows, if the agent offers it.
        for_session: Option<String>,
        state: RequestState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestState {
    Open,
    Resolved(Resolution),
}

impl RequestState {
    #[must_use]
    pub const fn is_open(&self) -> bool {
        matches!(self, Self::Open)
    }
}

/// An in-place change to an earlier item. Asking, resolving and finishing
/// are the same shape, so they are one `map` over the items.
enum Change {
    Ask(ToolCallId, Approval),
    Resolve(RequestId, Resolution),
    Finish(ToolCallId, ToolOutcome),
    /// The host ended the turn or the session: nothing still running will
    /// finish, and nothing still open will be answered.
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLabel {
    Starting,
    Idle,
    Running,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListStatus {
    Session(StatusLabel),
    Requests(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptyTranscript {
    Starting,
    Ready,
    Closed,
}

/// What the UI offers for a session, decided here so markup only shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Controls {
    pub composer: Composer,
    pub interrupt: Interrupt,
    pub resume: Resume,
    pub finish: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Composer {
    Open,
    Busy,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interrupt {
    Offered,
    Hidden,
}

/// Whether a closed session offers to start its agent again (`Continue`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    Offered,
    Hidden,
}

/// Whether the transcript ends with a sign that the agent is working.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Working {
    Shown,
    Hidden,
}

/// Whether a session waits on a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attention {
    Quiet,
    Requests(usize),
}

/// A live delta repeats the last stored sequence, which may belong to
/// another session.
pub(crate) const fn is_delta(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::AssistantDelta { .. } | EventKind::ReasoningDelta { .. }
    )
}

impl HostView {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            sessions: BTreeMap::new(),
        }
    }

    /// An event for a session this view never saw created is dropped, and
    /// a removed session is gone.
    #[must_use]
    pub fn apply(self, event: Event) -> Self {
        let delta = is_delta(&event.kind);
        let mut sessions = self.sessions;
        let session = match (sessions.remove(&event.session), event.kind) {
            (
                _,
                EventKind::SessionCreated {
                    instance,
                    directory,
                    model,
                    effort,
                    ..
                },
            ) => Some(SessionView {
                instance,
                directory,
                model,
                effort,
                status: SessionStatus::Starting,
                issue: None,
                finish: FinishState::Ready,
                checklist: Vec::new(),
                workspace: None,
                snapshots: Vec::new(),
                turn_starts: BTreeMap::new(),
                items: Vec::new(),
                subagents: Vec::new(),
                activity: event.seq,
            }),
            (_, EventKind::SessionRemoved { .. }) | (None, _) => None,
            (Some(view), kind) => {
                let mut next = view.apply(kind, event.seq);
                if !delta {
                    next.activity = event.seq;
                }
                Some(next)
            }
        };
        sessions.extend(session.map(|view| (event.session, view)));
        Self { sessions }
    }

    /// The sessions' ids, the most recently active first.
    #[must_use]
    pub fn recent(&self) -> Vec<SessionId> {
        self.sessions
            .iter()
            .map(|(id, view)| (Reverse(view.activity), id))
            .collect::<BTreeMap<Reverse<HostSeq>, &SessionId>>()
            .into_values()
            .copied()
            .collect()
    }

    /// Each directory a session ran in, once, the most recent first: the
    /// suggestions for a new session.
    #[must_use]
    pub fn directories(&self) -> Vec<HostPath> {
        self.recent()
            .iter()
            .filter_map(|id| self.sessions.get(id))
            .fold(Vec::new(), |known, view| {
                match known.iter().find(|path| path.0 == view.directory.0) {
                    Some(_) => known,
                    None => known
                        .into_iter()
                        .chain([view.directory.clone()])
                        .collect(),
                }
            })
    }
}

impl SessionView {
    #[must_use]
    pub const fn controls(&self) -> Controls {
        let (composer, interrupt, resume) = match &self.status {
            SessionStatus::Starting => {
                (Composer::Busy, Interrupt::Hidden, Resume::Hidden)
            }
            SessionStatus::Idle => {
                (Composer::Open, Interrupt::Hidden, Resume::Hidden)
            }
            SessionStatus::Running => {
                (Composer::Busy, Interrupt::Offered, Resume::Hidden)
            }
            SessionStatus::Closed(_) => {
                (Composer::Closed, Interrupt::Hidden, Resume::Offered)
            }
        };
        Controls {
            composer,
            interrupt,
            resume,
            finish: self.issue.is_some()
                && matches!(
                    self.status,
                    SessionStatus::Idle | SessionStatus::Closed(_)
                )
                && matches!(
                    self.finish,
                    FinishState::Ready | FinishState::Retrying
                ),
        }
    }

    #[must_use]
    pub const fn status_label(&self) -> StatusLabel {
        match &self.status {
            SessionStatus::Starting => StatusLabel::Starting,
            SessionStatus::Idle => StatusLabel::Idle,
            SessionStatus::Running => StatusLabel::Running,
            SessionStatus::Closed(_) => StatusLabel::Closed,
        }
    }

    #[must_use]
    pub fn list_status(&self) -> ListStatus {
        let label = self.status_label();
        match self.attention() {
            Attention::Quiet => ListStatus::Session(label),
            Attention::Requests(open) => ListStatus::Requests(open),
        }
    }

    #[must_use]
    pub const fn closed_reason(&self) -> Option<&CloseReason> {
        match &self.status {
            SessionStatus::Closed(reason) => Some(reason),
            SessionStatus::Starting
            | SessionStatus::Idle
            | SessionStatus::Running => None,
        }
    }

    #[must_use]
    pub const fn empty_transcript(&self) -> Option<EmptyTranscript> {
        if !self.items.is_empty() {
            return None;
        }
        Some(match self.status {
            SessionStatus::Starting => EmptyTranscript::Starting,
            SessionStatus::Idle | SessionStatus::Running => {
                EmptyTranscript::Ready
            }
            SessionStatus::Closed(_) => EmptyTranscript::Closed,
        })
    }

    /// A running turn shows it is working, unless it waits on a person or
    /// its last item is live already: a reply or reasoning streaming in, or
    /// a tool call running.
    #[must_use]
    pub fn working(&self) -> Working {
        match (&self.status, self.attention(), self.items.last()) {
            (
                SessionStatus::Running,
                Attention::Quiet,
                Some(
                    Item::Reply {
                        state: ReplyState::Streaming,
                        ..
                    }
                    | Item::Reasoning {
                        state: ReplyState::Streaming,
                        ..
                    }
                    | Item::Tool {
                        state: ToolState::Running,
                        ..
                    },
                ),
            )
            | (SessionStatus::Running, Attention::Requests(_), _)
            | (
                SessionStatus::Starting
                | SessionStatus::Idle
                | SessionStatus::Closed(_),
                _,
                _,
            ) => Working::Hidden,
            (SessionStatus::Running, Attention::Quiet, _) => Working::Shown,
        }
    }

    #[must_use]
    pub fn attention(&self) -> Attention {
        let open = self
            .request_items()
            .filter(|item| item.requests_attention())
            .count();
        match open {
            0 => Attention::Quiet,
            count => Attention::Requests(count),
        }
    }

    pub fn request_items(&self) -> impl Iterator<Item = &Item> {
        self.items
            .iter()
            .chain(self.subagents.iter().flat_map(|agent| &agent.items))
    }

    #[must_use]
    pub fn completed_turn_since(
        &self,
        since: usize,
    ) -> Option<CompletedTurn<'_>> {
        let pending = self.items.get(since..)?;
        let start = pending
            .iter()
            .position(|item| matches!(item, Item::User { .. }))?;
        let items = pending.get(start..)?;
        let (end, outcome) = items
            .iter()
            .enumerate()
            .skip(1)
            .take_while(|(_, item)| {
                !matches!(item, Item::User { .. } | Item::Continued(_))
            })
            .find_map(|(index, item)| match item {
                Item::TurnCompleted(outcome) => Some((index, outcome)),
                _ => None,
            })?;
        let snapshot = self
            .snapshots
            .iter()
            .find(|turn| turn.start == since.saturating_add(start))
            .map(|turn| turn.snapshot);
        Some(CompletedTurn {
            items: items.get(..end)?,
            outcome,
            snapshot,
        })
    }

    fn apply(self, kind: EventKind, seq: HostSeq) -> Self {
        match kind {
            EventKind::ChecklistChanged { checklist } => {
                Self { checklist, ..self }
            }
            EventKind::SessionReady => self.ready(),
            EventKind::ModelChanged {
                model,
                effort,
                reason,
            } => self.model_changed(model, effort, reason),
            EventKind::SessionClosed { reason } => self.closed(reason),
            EventKind::UserMessage { text, by } => {
                self.user_message(Item::User { text, by }, seq)
            }
            EventKind::AssistantDelta { text } => {
                self.reply(text, ReplyState::Streaming)
            }
            EventKind::AssistantMessage { text } => {
                self.reply(text, ReplyState::Done)
            }
            EventKind::ReasoningDelta { text } => {
                self.reasoning(text, ReplyState::Streaming)
            }
            EventKind::Reasoning { text } => {
                self.reasoning(text, ReplyState::Done)
            }
            EventKind::ToolCall { call, phase } => self.tool(call, phase),
            EventKind::RequestOpened {
                request,
                call,
                tool,
                reason,
                for_session,
            } => self.ask_unscoped(
                call,
                tool,
                Approval::open(request, reason, for_session),
            ),
            EventKind::QuestionAsked { request, questions } => {
                self.push(Item::Question {
                    request,
                    questions,
                    state: RequestState::Open,
                })
            }
            EventKind::RequestResolved {
                request,
                resolution,
            } => self.resolve_request(request, resolution),
            EventKind::TurnCompleted { outcome } => self
                .change(&Change::End)
                .status(SessionStatus::Idle)
                .push(Item::TurnCompleted(outcome)),
            EventKind::WorkspaceCreated { workspace } => Self {
                workspace: Some(workspace),
                ..self
            },
            issue_event @ (EventKind::OnIssue { .. }
            | EventKind::FinishStarted
            | EventKind::FinishInterrupted
            | EventKind::SessionFinished { .. }) => {
                self.issue_event(issue_event)
            }
            EventKind::EffortChanged { effort } => Self {
                effort: Some(effort),
                ..self
            },
            EventKind::SnapshotRecorded { snapshot } => {
                self.snapshot_recorded(snapshot.turn_start, seq)
            }
            EventKind::SessionCreated { .. }
            | EventKind::SessionRemoved { .. }
            | EventKind::NativeTurnStarted { .. } => self,
            EventKind::Error { message } => self.push(Item::Error(message)),
            EventKind::Subagent { agent, event } => {
                self.subagent_event(agent, event)
            }
        }
    }

    fn tool(self, call: ToolCallId, phase: ToolPhase) -> Self {
        match phase {
            ToolPhase::Started { tool } => self.push(Item::Tool {
                call,
                tool,
                state: ToolState::Running,
                approval: Approval::NotAsked,
            }),
            ToolPhase::Finished { outcome } => {
                self.change(&Change::Finish(call, outcome))
            }
        }
    }

    fn model_changed(
        self,
        model: Option<ModelId>,
        effort: Option<Effort>,
        reason: String,
    ) -> Self {
        Self {
            model,
            effort,
            ..self
        }
        .push(Item::ModelChanged(reason))
    }

    /// The issue the session works on, and finishing the session on it.
    fn issue_event(self, kind: EventKind) -> Self {
        match kind {
            EventKind::OnIssue { issue } => Self {
                issue: Some(issue),
                ..self
            },
            EventKind::FinishStarted => Self {
                finish: self.finish.after(&EventKind::FinishStarted),
                ..self
            }
            .push(Item::Finishing),
            EventKind::FinishInterrupted => Self {
                finish: self.finish.after(&kind),
                ..self
            },
            EventKind::SessionFinished { finished } => Self {
                finish: FinishState::Done,
                ..self
            }
            .push(Item::Finished(finished)),
            _ => self,
        }
    }

    fn resolve_request(
        self,
        request: RequestId,
        resolution: Resolution,
    ) -> Self {
        let change = Change::Resolve(request, resolution);
        let session = self.change(&change);
        Self {
            subagents: session
                .subagents
                .into_iter()
                .map(|agent| agent.change(&change))
                .collect(),
            ..session
        }
    }

    fn subagent_event(self, agent: SubagentId, event: SubagentEvent) -> Self {
        match event {
            SubagentEvent::Started { call, description } => {
                self.subagent_started(agent, call, description)
            }
            SubagentEvent::Activity(activity) => {
                self.subagent(&agent, |subagent| subagent.did(activity))
            }
            SubagentEvent::Ended { outcome, summary } => {
                self.subagent(&agent, |subagent| {
                    subagent.ended(SubagentStatus::Ended(outcome), summary)
                })
            }
            SubagentEvent::Kind { kind } => {
                self.subagent(&agent, |subagent| Subagent {
                    kind: Some(kind),
                    ..subagent
                })
            }
            SubagentEvent::ChecklistChanged { checklist } => {
                self.subagent(&agent, |subagent| Subagent {
                    checklist,
                    ..subagent
                })
            }
            SubagentEvent::RequestOpened {
                request,
                call,
                tool,
                reason,
                for_session,
            } => self.subagent(&agent, |subagent| {
                subagent.ask(
                    call,
                    tool,
                    Approval::open(request, reason, for_session),
                )
            }),
            SubagentEvent::QuestionAsked { request, questions } => self
                .subagent(&agent, |subagent| {
                    subagent.pushed(Item::Question {
                        request,
                        questions,
                        state: RequestState::Open,
                    })
                }),
        }
    }

    /// A sub-agent the agent resumes runs again under its id, after what
    /// it did before.
    fn subagent_started(
        self,
        agent: SubagentId,
        call: ToolCallId,
        description: String,
    ) -> Self {
        let mut subagents = self.subagents;
        match subagents.iter_mut().find(|known| known.id == agent) {
            Some(known) => known.status = SubagentStatus::Running,
            None => subagents.push(Subagent {
                id: agent,
                call,
                description,
                kind: None,
                checklist: Vec::new(),
                status: SubagentStatus::Running,
                items: Vec::new(),
            }),
        }
        Self { subagents, ..self }
    }

    /// Activity of a sub-agent this view never saw start is dropped.
    fn subagent(
        self,
        agent: &SubagentId,
        apply: impl FnOnce(Subagent) -> Subagent,
    ) -> Self {
        let mut subagents = self.subagents;
        let position = subagents.iter().position(|known| &known.id == agent);
        if let Some(index) = position {
            let found = subagents.remove(index);
            subagents.insert(index, apply(found));
        }
        Self { subagents, ..self }
    }

    fn subagents_stopped(self) -> Self {
        Self {
            subagents: self
                .subagents
                .into_iter()
                .map(|subagent| match subagent.status {
                    SubagentStatus::Running => {
                        subagent.ended(SubagentStatus::Stopped, None)
                    }
                    SubagentStatus::Ended(_) | SubagentStatus::Stopped => {
                        subagent
                    }
                })
                .collect(),
            ..self
        }
    }

    /// Nothing still running will finish, and nothing still open will be
    /// answered.
    fn closed(self, reason: CloseReason) -> Self {
        self.change(&Change::End)
            .settle()
            .subagents_stopped()
            .status(SessionStatus::Closed(reason))
    }

    fn status(self, status: SessionStatus) -> Self {
        Self { status, ..self }
    }

    /// A session that closed and is ready again was continued.
    fn ready(self) -> Self {
        match self.status.clone() {
            SessionStatus::Closed(reason) => self
                .status(SessionStatus::Idle)
                .push(Item::Continued(reason)),
            SessionStatus::Starting
            | SessionStatus::Idle
            | SessionStatus::Running => self.status(SessionStatus::Idle),
        }
    }

    /// A user message starts a turn at `seq`.
    fn user_message(self, message: Item, seq: HostSeq) -> Self {
        let mut turn_starts = self.turn_starts;
        turn_starts.insert(seq, self.items.len());
        Self {
            turn_starts,
            finish: self.finish.after(&EventKind::UserMessage {
                text: String::new(),
                by: ClientName(String::new()),
            }),
            ..self
        }
        .status(SessionStatus::Running)
        .push(message)
    }

    fn snapshot_recorded(
        self,
        turn_start: Option<HostSeq>,
        seq: HostSeq,
    ) -> Self {
        let start = turn_start
            .and_then(|start| self.turn_starts.get(&start).copied())
            .unwrap_or_else(|| {
                self.snapshots
                    .last()
                    .map_or(0, |turn| turn.end.saturating_add(1))
            });
        if start >= self.items.len() {
            return self;
        }
        let mut snapshots = self.snapshots;
        snapshots.push(TurnSnapshot {
            snapshot: seq,
            start,
            end: self.items.len().saturating_sub(1),
        });
        Self { snapshots, ..self }
    }

    /// A delta joins the reply that is still streaming; the final text
    /// replaces it, so a client that missed the deltas shows the same reply.
    fn reply(self, text: String, state: ReplyState) -> Self {
        let mut items = self.items;
        let open = match items.pop() {
            Some(Item::Reply {
                text: streamed,
                state: ReplyState::Streaming,
            }) => Some(streamed),
            last => {
                items.extend(last.map(Item::settled));
                None
            }
        };
        let joined = match state {
            ReplyState::Streaming => {
                let mut joined = open.unwrap_or_default();
                joined.push_str(&text);
                joined
            }
            ReplyState::Done => text,
        };
        items.push(Item::Reply {
            text: joined,
            state,
        });
        Self { items, ..self }
    }

    /// Reasoning folds as a reply does, into an item of its own.
    fn reasoning(self, text: String, state: ReplyState) -> Self {
        let mut items = self.items;
        let open = match items.pop() {
            Some(Item::Reasoning {
                text: streamed,
                state: ReplyState::Streaming,
            }) => Some(streamed),
            last => {
                items.extend(last.map(Item::settled));
                None
            }
        };
        let joined = match state {
            ReplyState::Streaming => {
                let mut joined = open.unwrap_or_default();
                joined.push_str(&text);
                joined
            }
            ReplyState::Done => text,
        };
        items.push(Item::Reasoning {
            text: joined,
            state,
        });
        Self { items, ..self }
    }

    /// Closes a reply or reasoning that was still streaming.
    fn settle(self) -> Self {
        let mut items = self.items;
        let last = items.pop().map(Item::settled);
        items.extend(last);
        Self { items, ..self }
    }

    fn push(self, item: Item) -> Self {
        let view = self.settle();
        let mut items = view.items;
        items.push(item);
        Self { items, ..view }
    }

    /// A request joins its tool call's card. A request for a call this view
    /// never saw started gets a card of its own.
    fn ask_unscoped(
        self,
        call: ToolCallId,
        tool: ToolKind,
        approval: Approval,
    ) -> Self {
        match self.legacy_request_agent(&call) {
            Some(agent) => {
                self.subagent(&agent, |child| child.ask(call, tool, approval))
            }
            None => self.ask(call, tool, approval),
        }
    }

    // Older logs opened child approvals as session-level events. Recover the
    // child only when its recorded call has an unambiguous owner.
    fn legacy_request_agent(&self, call: &ToolCallId) -> Option<SubagentId> {
        if self
            .items
            .iter()
            .filter_map(Item::call)
            .any(|seen| seen == call)
        {
            return None;
        }
        let mut owners = self.subagents.iter().filter(|agent| {
            agent
                .items
                .iter()
                .filter_map(Item::call)
                .any(|seen| seen == call)
        });
        let owner = owners.next()?;
        owners.next().is_none().then(|| owner.id.clone())
    }

    fn ask(self, call: ToolCallId, tool: ToolKind, approval: Approval) -> Self {
        match self
            .items
            .iter()
            .filter_map(Item::call)
            .find(|seen| seen.0 == call.0)
        {
            Some(_) => self.change(&Change::Ask(call, approval)),
            None => self.push(Item::Tool {
                call,
                tool,
                state: ToolState::Running,
                approval,
            }),
        }
    }

    fn change(self, change: &Change) -> Self {
        Self {
            items: self
                .items
                .into_iter()
                .map(|item| item.changed(change))
                .collect(),
            ..self
        }
    }
}

impl Subagent {
    /// Its transcript holds only finished items: a sub-agent's text
    /// arrives whole.
    fn did(self, activity: SubagentActivity) -> Self {
        match activity {
            SubagentActivity::Message { text } => self.pushed(Item::Reply {
                text,
                state: ReplyState::Done,
            }),
            SubagentActivity::Reasoning { text } => {
                self.pushed(Item::Reasoning {
                    text,
                    state: ReplyState::Done,
                })
            }
            SubagentActivity::ToolCall {
                call,
                phase: ToolPhase::Started { tool },
            } => self.tool_started(call, tool),
            SubagentActivity::ToolCall {
                call,
                phase: ToolPhase::Finished { outcome },
            } => self.change(&Change::Finish(call, outcome)),
        }
    }

    /// Its tool calls still running stop with it, and its report ends its
    /// transcript.
    fn ended(self, status: SubagentStatus, summary: Option<String>) -> Self {
        let ended = Self {
            status,
            ..self.change(&Change::End)
        };
        match summary {
            Some(text) if !matches!(ended.items.last(), Some(Item::Reply { text: previous, .. }) if previous == &text) => {
                ended.pushed(Item::Reply {
                    text,
                    state: ReplyState::Done,
                })
            }
            _ => ended,
        }
    }

    fn tool_started(mut self, call: ToolCallId, tool: ToolKind) -> Self {
        let previous = self.items.iter_mut().find_map(|item| match item {
            Item::Tool { call: id, tool, .. } if id == &call => Some(tool),
            _ => None,
        });
        if let Some(known) = previous {
            *known = tool;
            self
        } else {
            self.pushed(Item::Tool {
                call,
                tool,
                state: ToolState::Running,
                approval: Approval::NotAsked,
            })
        }
    }

    fn ask(self, call: ToolCallId, tool: ToolKind, approval: Approval) -> Self {
        if self
            .items
            .iter()
            .filter_map(Item::call)
            .any(|seen| seen == &call)
        {
            self.change(&Change::Ask(call, approval))
        } else {
            self.pushed(Item::Tool {
                call,
                tool,
                state: ToolState::Running,
                approval,
            })
        }
    }

    fn pushed(self, item: Item) -> Self {
        let mut items = self.items;
        items.push(item);
        Self { items, ..self }
    }

    fn change(self, change: &Change) -> Self {
        Self {
            items: self
                .items
                .into_iter()
                .map(|item| item.changed(change))
                .collect(),
            ..self
        }
    }
}

impl Item {
    /// The item, done streaming: something else follows it.
    fn settled(self) -> Self {
        match self {
            Self::Reply { text, .. } => Self::Reply {
                text,
                state: ReplyState::Done,
            },
            Self::Reasoning { text, .. } => Self::Reasoning {
                text,
                state: ReplyState::Done,
            },
            other => other,
        }
    }

    #[must_use]
    pub const fn requests_attention(&self) -> bool {
        matches!(
            self,
            Self::Tool {
                approval: Approval::Asked {
                    state: RequestState::Open,
                    ..
                },
                ..
            } | Self::Question {
                state: RequestState::Open,
                ..
            }
        )
    }

    /// The tool call the item shows, if it shows one.
    #[must_use]
    pub const fn call(&self) -> Option<&ToolCallId> {
        match self {
            Self::Tool { call, .. } => Some(call),
            Self::User { .. }
            | Self::Reply { .. }
            | Self::Reasoning { .. }
            | Self::Question { .. }
            | Self::TurnCompleted(_)
            | Self::Continued(_)
            | Self::ModelChanged(_)
            | Self::Finishing
            | Self::Finished(_)
            | Self::Error(_) => None,
        }
    }

    fn changed(self, change: &Change) -> Self {
        match self {
            Self::Tool {
                call,
                tool,
                state,
                approval,
            } => Self::Tool {
                state: state.changed(&call, change),
                approval: approval.changed(&call, change),
                call,
                tool,
            },
            Self::Question {
                request,
                questions,
                state,
            } => Self::Question {
                state: state.changed(request, change),
                request,
                questions,
            },
            item => item,
        }
    }
}

impl ToolState {
    fn changed(self, call: &ToolCallId, change: &Change) -> Self {
        match (self, change) {
            (_, Change::Finish(id, outcome)) if call == id => {
                Self::Finished(outcome.to_owned())
            }
            (Self::Running, Change::End) => Self::Stopped,
            (
                state,
                Change::Ask(_, _)
                | Change::Resolve(_, _)
                | Change::Finish(_, _)
                | Change::End,
            ) => state,
        }
    }
}

impl Approval {
    const fn open(
        request: RequestId,
        reason: Option<String>,
        for_session: Option<String>,
    ) -> Self {
        Self::Asked {
            request,
            reason,
            for_session,
            state: RequestState::Open,
        }
    }

    fn changed(self, call: &ToolCallId, change: &Change) -> Self {
        match (self, change) {
            (_, Change::Ask(id, approval)) if call == id => approval.clone(),
            (current, _) => match current {
                Self::Asked {
                    request,
                    reason,
                    for_session,
                    state,
                } => Self::Asked {
                    state: state.changed(request, change),
                    request,
                    reason,
                    for_session,
                },
                Self::NotAsked => Self::NotAsked,
            },
        }
    }
}

impl RequestState {
    /// A request's answer, or its withdrawal when the turn or session ends
    /// with it still open.
    fn changed(self, request: RequestId, change: &Change) -> Self {
        match (self, change) {
            (_, Change::Resolve(id, resolution)) if request == *id => {
                Self::Resolved(resolution.clone())
            }
            (Self::Open, Change::End) => Self::Resolved(Resolution::Withdrawn),
            (
                state,
                Change::Ask(_, _)
                | Change::Resolve(_, _)
                | Change::Finish(_, _)
                | Change::End,
            ) => state,
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;

    use proto::{
        Answer, ChecklistEntry, ChecklistStatus, ClientName, CloseReason,
        Decision, Effort, Event, EventKind, FinishState, HostPath, HostSeq,
        InstanceId, ModelId, Provider, Question, QuestionOption, RequestId,
        Resolution, Selection, SessionId, SubagentActivity, SubagentEvent,
        SubagentId, SubagentOutcome, ToolCallId, ToolKind, ToolOutcome,
        ToolPhase, TurnOutcome,
    };

    use super::{
        Approval, Attention, Composer, HostView, Item, ListStatus, ReplyState,
        RequestState, Resume, SessionStatus, SessionView, StatusLabel,
        Subagent, SubagentStatus, ToolState, Working,
    };

    fn text(text: &str) -> String {
        String::from(text)
    }

    fn created(directory: &str) -> EventKind {
        EventKind::SessionCreated {
            instance: InstanceId::own(Provider::Claude),
            directory: HostPath(text(directory)),
            by: ClientName(text("desk")),
            model: None,
            effort: None,
        }
    }

    fn reasoned(piece: &str) -> EventKind {
        EventKind::ReasoningDelta { text: text(piece) }
    }

    fn delta(text: &str) -> EventKind {
        EventKind::AssistantDelta {
            text: String::from(text),
        }
    }

    /// Two sessions interleaved: a turn with a tool call, its approval and
    /// a request withdrawn, and a session that errs and closes.
    fn transcript_events(call: &ToolCallId) -> Vec<(u64, EventKind)> {
        Vec::from([
            (1, created("/a")),
            (2, created("/b")),
            (1, EventKind::SessionReady),
            (
                1,
                EventKind::UserMessage {
                    text: text("hi"),
                    by: ClientName(text("phone")),
                },
            ),
            (1, delta("Hel")),
            (2, EventKind::SessionReady),
            (1, delta("lo")),
            (
                1,
                EventKind::AssistantMessage {
                    text: text("Hello."),
                },
            ),
            (
                1,
                EventKind::ToolCall {
                    call: call.clone(),
                    phase: ToolPhase::Started {
                        tool: ToolKind::Command {
                            command: text("ls"),
                        },
                    },
                },
            ),
            (
                1,
                EventKind::RequestOpened {
                    request: RequestId(1),
                    call: call.clone(),
                    tool: ToolKind::Command {
                        command: text("ls"),
                    },
                    reason: Some(text("to list")),
                    for_session: None,
                },
            ),
            (
                1,
                EventKind::RequestOpened {
                    request: RequestId(2),
                    call: ToolCallId(text("toolu_2")),
                    tool: ToolKind::Command {
                        command: text("rm"),
                    },
                    reason: None,
                    for_session: None,
                },
            ),
            (
                1,
                EventKind::RequestResolved {
                    request: RequestId(1),
                    resolution: Resolution::Decided {
                        decision: Decision::Approve,
                        by: ClientName(text("phone")),
                    },
                },
            ),
            (
                1,
                EventKind::RequestResolved {
                    request: RequestId(2),
                    resolution: Resolution::Withdrawn,
                },
            ),
            (
                1,
                EventKind::ToolCall {
                    call: call.clone(),
                    phase: ToolPhase::Finished {
                        outcome: ToolOutcome::Succeeded,
                    },
                },
            ),
        ])
        .into_iter()
        .chain(closing_events())
        .collect()
    }

    /// The end of `transcript_events`: the turn completes, and the second
    /// session errs and closes.
    fn closing_events() -> [(u64, EventKind); 5] {
        [
            (1, delta("Done")),
            (
                1,
                EventKind::TurnCompleted {
                    outcome: TurnOutcome::Completed,
                },
            ),
            (
                2,
                EventKind::Error {
                    message: text("odd"),
                },
            ),
            (
                2,
                EventKind::SessionClosed {
                    reason: CloseReason::Exited { status: text("1") },
                },
            ),
            (3, EventKind::SessionReady),
        ]
    }

    fn expected_transcripts(
        call: ToolCallId,
    ) -> BTreeMap<SessionId, SessionView> {
        BTreeMap::from([
            (
                SessionId(1),
                SessionView {
                    instance: InstanceId::own(Provider::Claude),
                    directory: HostPath(text("/a")),
                    model: None,
                    effort: None,
                    status: SessionStatus::Idle,
                    issue: None,
                    finish: FinishState::Ready,
                    checklist: Vec::new(),
                    subagents: Vec::new(),
                    items: Vec::from([
                        Item::User {
                            text: text("hi"),
                            by: ClientName(text("phone")),
                        },
                        Item::Reply {
                            text: text("Hello."),
                            state: ReplyState::Done,
                        },
                        Item::Tool {
                            call,
                            tool: ToolKind::Command {
                                command: text("ls"),
                            },
                            state: ToolState::Finished(ToolOutcome::Succeeded),
                            approval: Approval::Asked {
                                request: RequestId(1),
                                reason: Some(text("to list")),
                                for_session: None,
                                state: RequestState::Resolved(
                                    Resolution::Decided {
                                        decision: Decision::Approve,
                                        by: ClientName(text("phone")),
                                    },
                                ),
                            },
                        },
                        Item::Tool {
                            call: ToolCallId(text("toolu_2")),
                            tool: ToolKind::Command {
                                command: text("rm"),
                            },
                            state: ToolState::Stopped,
                            approval: Approval::Asked {
                                request: RequestId(2),
                                reason: None,
                                for_session: None,
                                state: RequestState::Resolved(
                                    Resolution::Withdrawn,
                                ),
                            },
                        },
                        Item::Reply {
                            text: text("Done"),
                            state: ReplyState::Done,
                        },
                        Item::TurnCompleted(TurnOutcome::Completed),
                    ]),
                    workspace: None,
                    snapshots: Vec::new(),
                    turn_starts: BTreeMap::from([(HostSeq(4), 0)]),
                    activity: HostSeq(13),
                },
            ),
            (
                SessionId(2),
                SessionView {
                    instance: InstanceId::own(Provider::Claude),
                    directory: HostPath(text("/b")),
                    model: None,
                    effort: None,
                    status: SessionStatus::Closed(CloseReason::Exited {
                        status: text("1"),
                    }),
                    issue: None,
                    finish: FinishState::Ready,
                    checklist: Vec::new(),
                    subagents: Vec::new(),
                    items: Vec::from([Item::Error(text("odd"))]),
                    workspace: None,
                    snapshots: Vec::new(),
                    turn_starts: BTreeMap::new(),
                    activity: HostSeq(15),
                },
            ),
        ])
    }

    #[test]
    fn events_fold_into_each_sessions_transcript_in_order() {
        let call = ToolCallId(text("toolu_1"));
        let view = fold(transcript_events(&call));
        assert_eq!(view.sessions, expected_transcripts(call));
    }

    /// Numbers the events as the host does: a delta repeats the last seq.
    fn fold(events: Vec<(u64, EventKind)>) -> HostView {
        let (view, _) = events.into_iter().fold(
            (HostView::empty(), HostSeq(0)),
            |(view, last), (session, kind)| {
                let seq = match kind {
                    EventKind::AssistantDelta { .. } => last,
                    _ => last.next(),
                };
                let event = Event {
                    seq,
                    session: SessionId(u128::from(session)),
                    kind,
                };
                (view.apply(event), seq)
            },
        );
        view
    }

    fn opened(request: u64) -> EventKind {
        EventKind::RequestOpened {
            request: RequestId(request),
            call: ToolCallId(format!("toolu_{request}")),
            tool: ToolKind::Command {
                command: text("ls"),
            },
            reason: None,
            for_session: None,
        }
    }

    fn started(call: &str) -> EventKind {
        EventKind::ToolCall {
            call: ToolCallId(text(call)),
            phase: ToolPhase::Started {
                tool: ToolKind::Command {
                    command: text("ls"),
                },
            },
        }
    }

    fn asked(
        call: &str,
        request: u64,
        state: ToolState,
        approval: RequestState,
    ) -> Item {
        Item::Tool {
            call: ToolCallId(text(call)),
            tool: ToolKind::Command {
                command: text("ls"),
            },
            state,
            approval: Approval::Asked {
                request: RequestId(request),
                reason: None,
                for_session: None,
                state: approval,
            },
        }
    }

    #[test]
    fn the_end_of_a_turn_or_session_stops_its_tools_and_withdraws_its_requests()
    {
        let view = fold(Vec::from([
            (1, created("/a")),
            (1, started("toolu_1")),
            (1, opened(1)),
            (1, started("toolu_2")),
            (
                1,
                EventKind::ToolCall {
                    call: ToolCallId(text("toolu_2")),
                    phase: ToolPhase::Finished {
                        outcome: ToolOutcome::Succeeded,
                    },
                },
            ),
            (
                1,
                EventKind::TurnCompleted {
                    outcome: TurnOutcome::Interrupted,
                },
            ),
            (2, created("/b")),
            (2, opened(3)),
            (
                2,
                EventKind::SessionClosed {
                    reason: CloseReason::Exited { status: text("1") },
                },
            ),
        ]));
        assert_eq!(
            view.sessions
                .values()
                .map(|session| session.items.clone())
                .collect::<Vec<Vec<Item>>>(),
            Vec::from([
                Vec::from([
                    asked(
                        "toolu_1",
                        1,
                        ToolState::Stopped,
                        RequestState::Resolved(Resolution::Withdrawn)
                    ),
                    Item::Tool {
                        call: ToolCallId(text("toolu_2")),
                        tool: ToolKind::Command {
                            command: text("ls"),
                        },
                        state: ToolState::Finished(ToolOutcome::Succeeded),
                        approval: Approval::NotAsked,
                    },
                    Item::TurnCompleted(TurnOutcome::Interrupted),
                ]),
                Vec::from([asked(
                    "toolu_3",
                    3,
                    ToolState::Stopped,
                    RequestState::Resolved(Resolution::Withdrawn)
                )]),
            ])
        );
        assert_eq!(
            view.sessions
                .values()
                .map(SessionView::attention)
                .collect::<Vec<Attention>>(),
            Vec::from([Attention::Quiet, Attention::Quiet])
        );
    }

    #[test]
    fn the_list_puts_the_latest_activity_first_and_suggests_each_directory_once()
     {
        let view = fold(Vec::from([
            (1, created("/a")),
            (2, created("/b")),
            (3, created("/a")),
            (2, EventKind::SessionReady),
            (1, delta("x")),
        ]));
        assert_eq!(
            view.recent(),
            Vec::from([SessionId(2), SessionId(3), SessionId(1)])
        );
        assert_eq!(
            view.directories(),
            Vec::from([HostPath(text("/b")), HostPath(text("/a"))])
        );
    }

    #[test]
    fn a_removed_session_leaves_the_view_with_its_later_events() {
        let view = fold(Vec::from([
            (1, created("/a")),
            (2, created("/b")),
            (1, EventKind::SessionRemoved { kept: None }),
            (1, EventKind::SessionReady),
        ]));
        assert_eq!(view.recent(), Vec::from([SessionId(2)]));
    }

    #[test]
    fn reasoning_streams_into_one_item_that_its_text_replaces_and_a_close_is_kept()
     {
        let view = fold(Vec::from([
            (1, created("/a")),
            (1, EventKind::SessionReady),
            (1, reasoned("Look")),
            (1, reasoned("ing")),
            (
                1,
                EventKind::Reasoning {
                    text: text("Looking."),
                },
            ),
            (1, reasoned("Again")),
            (1, delta("Hi")),
            (
                1,
                EventKind::SessionClosed {
                    reason: CloseReason::Exited { status: text("9") },
                },
            ),
            (1, EventKind::SessionReady),
        ]));
        assert_eq!(
            view.sessions.get(&SessionId(1)).map(|session| (
                session.items.clone(),
                session.status.clone(),
                session.controls().resume
            )),
            Some((
                Vec::from([
                    Item::Reasoning {
                        text: text("Looking."),
                        state: ReplyState::Done,
                    },
                    Item::Reasoning {
                        text: text("Again"),
                        state: ReplyState::Done,
                    },
                    Item::Reply {
                        text: text("Hi"),
                        state: ReplyState::Done,
                    },
                    Item::Continued(CloseReason::Exited { status: text("9") }),
                ]),
                SessionStatus::Idle,
                Resume::Hidden,
            ))
        );
    }

    #[test]
    fn reply_and_reasoning_append_every_delta() {
        let view = fold(Vec::from([
            (1, created("/a")),
            (1, reasoned("One")),
            (1, reasoned(" two")),
            (1, reasoned(" three")),
            (1, delta("Four")),
            (1, delta(" five")),
            (1, delta(" six")),
        ]));
        assert_eq!(
            view.sessions
                .get(&SessionId(1))
                .map(|session| &session.items),
            Some(&Vec::from([
                Item::Reasoning {
                    text: text("One two three"),
                    state: ReplyState::Done,
                },
                Item::Reply {
                    text: text("Four five six"),
                    state: ReplyState::Streaming,
                },
            ]))
        );
    }

    #[test]
    fn a_close_settles_the_reasoning_or_reply_that_was_streaming() {
        let view = fold(Vec::from([
            (1, created("/a")),
            (1, EventKind::SessionReady),
            (1, reasoned("Look")),
            closed(1),
            (2, created("/b")),
            (2, EventKind::SessionReady),
            (2, delta("Hel")),
            closed(2),
        ]));
        assert_eq!(
            (
                view.sessions
                    .get(&SessionId(1))
                    .map(|session| session.items.clone()),
                view.sessions
                    .get(&SessionId(2))
                    .map(|session| session.items.clone()),
            ),
            (
                Some(Vec::from([Item::Reasoning {
                    text: text("Look"),
                    state: ReplyState::Done,
                }])),
                Some(Vec::from([Item::Reply {
                    text: text("Hel"),
                    state: ReplyState::Done,
                }])),
            )
        );
    }

    #[test]
    fn a_model_change_shows_in_the_header_and_the_transcript() {
        let view = fold(Vec::from([
            (1, created("/a")),
            (1, EventKind::SessionReady),
            closed(1),
            (
                1,
                EventKind::ModelChanged {
                    model: Some(ModelId(text("gpt"))),
                    effort: None,
                    reason: text("Model changed to gpt: small is not offered"),
                },
            ),
        ]));
        assert_eq!(
            view.sessions
                .get(&SessionId(1))
                .map(|session| (session.model.clone(), session.items.clone())),
            Some((
                Some(ModelId(text("gpt"))),
                Vec::from([Item::ModelChanged(text(
                    "Model changed to gpt: small is not offered"
                ))])
            ))
        );
    }

    fn task(text: &str, status: ChecklistStatus) -> ChecklistEntry {
        ChecklistEntry {
            text: String::from(text),
            status,
        }
    }

    fn subagent(agent: &str, activity: SubagentActivity) -> EventKind {
        EventKind::Subagent {
            agent: SubagentId(text(agent)),
            event: SubagentEvent::Activity(activity),
        }
    }

    fn subagent_started(agent: &str, call: &str) -> EventKind {
        EventKind::Subagent {
            agent: SubagentId(text(agent)),
            event: SubagentEvent::Started {
                call: ToolCallId(text(call)),
                description: format!("look for {agent}"),
            },
        }
    }

    fn subagent_ended(
        agent: &str,
        outcome: SubagentOutcome,
        summary: Option<&str>,
    ) -> EventKind {
        EventKind::Subagent {
            agent: SubagentId(text(agent)),
            event: SubagentEvent::Ended {
                outcome,
                summary: summary.map(text),
            },
        }
    }

    fn subagent_ls(agent: &str, call: &str, phase: ToolPhase) -> EventKind {
        subagent(
            agent,
            SubagentActivity::ToolCall {
                call: ToolCallId(text(call)),
                phase,
            },
        )
    }

    fn ls_phase() -> ToolPhase {
        ToolPhase::Started {
            tool: ToolKind::Command {
                command: text("ls"),
            },
        }
    }

    fn ls(call: &str, state: ToolState) -> Item {
        Item::Tool {
            call: ToolCallId(text(call)),
            tool: ToolKind::Command {
                command: text("ls"),
            },
            state,
            approval: Approval::NotAsked,
        }
    }

    /// Two sub-agents: `a` asks to run a call, which the user approves, and
    /// finishes; `b` still runs a call when the session closes.
    fn subagent_events() -> Vec<(u64, EventKind)> {
        let succeeded = ToolPhase::Finished {
            outcome: ToolOutcome::Succeeded,
        };
        let said =
            |words: &str| SubagentActivity::Message { text: text(words) };
        Vec::from([
            (1, created("/a")),
            (1, EventKind::SessionReady),
            (1, started("toolu_agent_a")),
            (1, subagent_started("a", "toolu_agent_a")),
            (1, started("toolu_agent_b")),
            (1, subagent_started("b", "toolu_agent_b")),
            (
                1,
                subagent(
                    "a",
                    SubagentActivity::Reasoning {
                        text: text("where?"),
                    },
                ),
            ),
            (1, subagent_ls("a", "toolu_a1", ls_phase())),
            // Older logs published child approvals as session events.
            (
                1,
                EventKind::RequestOpened {
                    request: RequestId(1),
                    call: ToolCallId(text("toolu_a1")),
                    tool: ToolKind::Command {
                        command: text("ls"),
                    },
                    reason: None,
                    for_session: None,
                },
            ),
            (
                1,
                EventKind::RequestResolved {
                    request: RequestId(1),
                    resolution: Resolution::Decided {
                        decision: Decision::Approve,
                        by: ClientName(text("phone")),
                    },
                },
            ),
            (1, subagent_ls("a", "toolu_a1", succeeded)),
            (1, subagent("a", said("found it"))),
            (
                1,
                subagent_ended(
                    "a",
                    SubagentOutcome::Finished,
                    Some("it is in /a"),
                ),
            ),
            (1, subagent_ls("b", "toolu_b1", ls_phase())),
            (1, subagent("nobody", said("lost"))),
            (
                1,
                EventKind::SessionClosed {
                    reason: CloseReason::Exited { status: text("1") },
                },
            ),
        ])
    }

    fn expected_subagents() -> Vec<Subagent> {
        Vec::from([
            Subagent {
                kind: None,
                checklist: Vec::new(),
                id: SubagentId(text("a")),
                call: ToolCallId(text("toolu_agent_a")),
                description: text("look for a"),
                status: SubagentStatus::Ended(SubagentOutcome::Finished),
                items: Vec::from([
                    Item::Reasoning {
                        text: text("where?"),
                        state: ReplyState::Done,
                    },
                    Item::Tool {
                        call: ToolCallId(text("toolu_a1")),
                        tool: ToolKind::Command {
                            command: text("ls"),
                        },
                        state: ToolState::Finished(ToolOutcome::Succeeded),
                        approval: Approval::Asked {
                            request: RequestId(1),
                            reason: None,
                            for_session: None,
                            state: RequestState::Resolved(
                                Resolution::Decided {
                                    decision: Decision::Approve,
                                    by: ClientName(text("phone")),
                                },
                            ),
                        },
                    },
                    done_reply("found it"),
                    done_reply("it is in /a"),
                ]),
            },
            Subagent {
                kind: None,
                checklist: Vec::new(),
                id: SubagentId(text("b")),
                call: ToolCallId(text("toolu_agent_b")),
                description: text("look for b"),
                status: SubagentStatus::Stopped,
                items: Vec::from([ls("toolu_b1", ToolState::Stopped)]),
            },
        ])
    }

    #[test]
    fn subagents_fold_under_their_session_and_stop_when_it_closes() {
        let view = fold(subagent_events());
        assert_eq!(
            view.sessions.get(&SessionId(1)).map(|found| {
                (
                    found.subagents.clone(),
                    found
                        .items
                        .iter()
                        .map(|item| match item {
                            Item::Tool { call, state, .. } => {
                                Some((call.0.clone(), *state))
                            }
                            _ => None,
                        })
                        .collect::<Vec<Option<(String, ToolState)>>>(),
                )
            }),
            Some((
                expected_subagents(),
                Vec::from([
                    Some((text("toolu_agent_a"), ToolState::Stopped)),
                    Some((text("toolu_agent_b"), ToolState::Stopped)),
                ]),
            ))
        );
    }

    #[test]
    fn a_childs_tool_result_does_not_complete_a_main_tool_with_the_same_id() {
        let call = "shared";
        let view = fold(Vec::from([
            (1, created("/a")),
            (1, EventKind::SessionReady),
            (
                1,
                EventKind::ToolCall {
                    call: ToolCallId(text(call)),
                    phase: ls_phase(),
                },
            ),
            (1, subagent_started("a", "spawn")),
            (1, subagent_ls("a", call, ls_phase())),
            (
                1,
                subagent_ls(
                    "a",
                    call,
                    ToolPhase::Finished {
                        outcome: ToolOutcome::Succeeded,
                    },
                ),
            ),
        ]));
        assert_eq!(
            view.sessions
                .get(&SessionId(1))
                .and_then(|session| session.items.last()),
            Some(&ls(call, ToolState::Running))
        );
        assert_eq!(
            view.sessions
                .get(&SessionId(1))
                .and_then(|session| session.subagents.first())
                .and_then(|child| child.items.last()),
            Some(&ls(call, ToolState::Finished(ToolOutcome::Succeeded)))
        );
    }

    #[test]
    fn a_resumed_subagent_runs_again() {
        let resumed = fold(Vec::from([
            (1, created("/a")),
            (1, EventKind::SessionReady),
            (1, started("toolu_agent_a")),
            (1, subagent_started("a", "toolu_agent_a")),
            (1, subagent_ended("a", SubagentOutcome::Failed, None)),
            (1, subagent_started("a", "toolu_agent_a")),
        ]));
        assert_eq!(
            resumed.sessions.get(&SessionId(1)).map(|found| found
                .subagents
                .iter()
                .map(|known| (known.status, known.status.word()))
                .collect::<Vec<(SubagentStatus, &str)>>()),
            Some(Vec::from([(SubagentStatus::Running, "running")]))
        );
    }

    fn done_reply(said: &str) -> Item {
        Item::Reply {
            text: text(said),
            state: ReplyState::Done,
        }
    }

    /// Each change replaces the checklist, and a new effort shows in the
    /// header, with no transcript item for either.
    #[test]
    fn tasks_are_replaced_and_the_effort_follows_its_last_change() {
        let first = Vec::from([
            task("write a.txt", ChecklistStatus::InProgress),
            task("write b.txt", ChecklistStatus::Pending),
        ]);
        let later =
            Vec::from([task("write a.txt", ChecklistStatus::Completed)]);
        let view = fold(Vec::from([
            (1, created("/a")),
            (1, EventKind::SessionReady),
            (1, EventKind::ChecklistChanged { checklist: first }),
            (
                1,
                EventKind::ChecklistChanged {
                    checklist: later.clone(),
                },
            ),
            (
                1,
                EventKind::EffortChanged {
                    effort: Effort::Low,
                },
            ),
        ]));
        assert_eq!(
            view.sessions.get(&SessionId(1)).map(|session| (
                session.checklist.clone(),
                session.effort,
                session.items.clone()
            )),
            Some((later, Some(Effort::Low), Vec::new()))
        );
    }

    fn closed(session: u64) -> (u64, EventKind) {
        (
            session,
            EventKind::SessionClosed {
                reason: CloseReason::Exited { status: text("9") },
            },
        )
    }

    /// The events after a session starts, and whether it then shows that
    /// the agent is working.
    struct WorkingCase {
        events: Vec<EventKind>,
        working: Working,
    }

    #[test]
    fn a_running_turn_shows_it_is_working_until_something_live_ends_the_transcript()
     {
        let user = EventKind::UserMessage {
            text: text("hi"),
            by: ClientName(text("phone")),
        };
        let finished = EventKind::ToolCall {
            call: ToolCallId(text("toolu_1")),
            phase: ToolPhase::Finished {
                outcome: ToolOutcome::Succeeded,
            },
        };
        let cases = [
            WorkingCase {
                events: Vec::new(),
                working: Working::Hidden,
            },
            WorkingCase {
                events: Vec::from([user.clone()]),
                working: Working::Shown,
            },
            WorkingCase {
                events: Vec::from([user.clone(), delta("Hel")]),
                working: Working::Hidden,
            },
            WorkingCase {
                events: Vec::from([
                    user.clone(),
                    delta("Hel"),
                    EventKind::AssistantMessage {
                        text: text("Hello"),
                    },
                ]),
                working: Working::Shown,
            },
            WorkingCase {
                events: Vec::from([
                    user.clone(),
                    EventKind::ReasoningDelta { text: text("Hm") },
                ]),
                working: Working::Hidden,
            },
            WorkingCase {
                events: Vec::from([user.clone(), started("toolu_1")]),
                working: Working::Hidden,
            },
            WorkingCase {
                events: Vec::from([user.clone(), started("toolu_1"), finished]),
                working: Working::Shown,
            },
            WorkingCase {
                events: Vec::from([
                    user,
                    EventKind::TurnCompleted {
                        outcome: TurnOutcome::Completed,
                    },
                ]),
                working: Working::Hidden,
            },
        ];
        let (got, want) = cases
            .into_iter()
            .map(|case| {
                let view = fold(
                    [created("/a"), EventKind::SessionReady]
                        .into_iter()
                        .chain(case.events)
                        .map(|kind| (1, kind))
                        .collect(),
                );
                (
                    view.sessions
                        .values()
                        .map(SessionView::working)
                        .collect::<Vec<Working>>(),
                    Vec::from([case.working]),
                )
            })
            .unzip::<Vec<Working>, Vec<Working>, Vec<Vec<Working>>, Vec<Vec<Working>>>();
        assert_eq!(got, want);
    }

    #[test]
    fn rv_no_thinking_under_an_open_approval_after_a_parallel_tool_finished()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = fold(Vec::from([
            (1, created("/a")),
            (1, EventKind::SessionReady),
            (
                1,
                EventKind::UserMessage {
                    text: text("hi"),
                    by: ClientName(text("phone")),
                },
            ),
            (1, started("toolu_1")),
            (1, started("toolu_2")),
            (
                1,
                EventKind::ToolCall {
                    call: ToolCallId(text("toolu_2")),
                    phase: ToolPhase::Finished {
                        outcome: ToolOutcome::Succeeded,
                    },
                },
            ),
            (1, opened(1)),
        ]));
        let session =
            view.sessions.values().next().ok_or("expected a value")?;
        assert_eq!(session.attention(), Attention::Requests(1));
        assert_eq!(session.working(), Working::Hidden);
        Ok(())
    }

    #[test]
    fn rv_no_thinking_under_an_open_approval_after_a_commentary_reply()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = fold(Vec::from([
            (1, created("/a")),
            (1, EventKind::SessionReady),
            (
                1,
                EventKind::UserMessage {
                    text: text("hi"),
                    by: ClientName(text("phone")),
                },
            ),
            (1, started("toolu_1")),
            (
                1,
                EventKind::AssistantMessage {
                    text: text("I need to write a file."),
                },
            ),
            (1, opened(1)),
        ]));
        let session =
            view.sessions.values().next().ok_or("expected a value")?;
        assert_eq!(session.attention(), Attention::Requests(1));
        assert_eq!(session.working(), Working::Hidden);
        Ok(())
    }

    #[test]
    fn attention_counts_the_open_requests() {
        let view = fold(Vec::from([
            (1, created("/a")),
            (1, opened(1)),
            (1, opened(2)),
            (1, opened(3)),
            (
                1,
                EventKind::RequestResolved {
                    request: RequestId(2),
                    resolution: Resolution::Withdrawn,
                },
            ),
            (2, created("/b")),
        ]));
        assert_eq!(
            view.sessions
                .values()
                .map(SessionView::attention)
                .collect::<Vec<Attention>>(),
            Vec::from([Attention::Requests(2), Attention::Quiet])
        );
    }

    fn question(request: u64) -> EventKind {
        EventKind::QuestionAsked {
            request: RequestId(request),
            questions: Vec::from([Question {
                header: text("Color"),
                text: text("Which color?"),
                options: Vec::from([QuestionOption {
                    label: text("Blue"),
                    description: text("Cool"),
                }]),
                selection: Selection::One,
            }]),
        }
    }

    fn questions(
        view: &HostView,
        session: u64,
    ) -> Vec<(RequestId, RequestState)> {
        view.sessions.get(&SessionId(u128::from(session))).map_or(
            Vec::new(),
            |found| {
                found
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        Item::Question { request, state, .. } => {
                            Some((request.to_owned(), state.clone()))
                        }
                        Item::User { .. }
                        | Item::Reply { .. }
                        | Item::Reasoning { .. }
                        | Item::Tool { .. }
                        | Item::TurnCompleted(_)
                        | Item::Continued(_)
                        | Item::ModelChanged(_)
                        | Item::Finishing
                        | Item::Finished(_)
                        | Item::Error(_) => None,
                    })
                    .collect()
            },
        )
    }

    #[test]
    fn a_question_waits_on_a_person_until_answered_or_the_turn_ends() {
        let answered = Resolution::Answered {
            answers: Vec::from([Answer(Vec::from([text("Blue")]))]),
            by: ClientName(text("phone")),
        };
        let open = fold(Vec::from([
            (1, created("/a")),
            (1, EventKind::SessionReady),
            (
                1,
                EventKind::UserMessage {
                    text: text("ask me"),
                    by: ClientName(text("desk")),
                },
            ),
            (1, question(1)),
            (1, question(2)),
            (1, opened(3)),
        ]));
        let later = fold(Vec::from([
            (1, created("/a")),
            (1, question(1)),
            (1, question(2)),
            (
                1,
                EventKind::RequestResolved {
                    request: RequestId(1),
                    resolution: answered.clone(),
                },
            ),
            (
                1,
                EventKind::TurnCompleted {
                    outcome: TurnOutcome::Interrupted,
                },
            ),
        ]));
        assert_eq!(
            (
                open.sessions.get(&SessionId(1)).map(SessionView::attention),
                open.sessions.get(&SessionId(1)).map(SessionView::working),
                questions(&later, 1),
                later
                    .sessions
                    .get(&SessionId(1))
                    .map(SessionView::attention),
            ),
            (
                Some(Attention::Requests(3)),
                Some(Working::Hidden),
                Vec::from([
                    (RequestId(1), RequestState::Resolved(answered)),
                    (
                        RequestId(2),
                        RequestState::Resolved(Resolution::Withdrawn)
                    ),
                ]),
                Some(Attention::Quiet),
            )
        );
    }

    fn snapshot(start: u64) -> EventKind {
        EventKind::SnapshotRecorded {
            snapshot: proto::Snapshot {
                turn_start: Some(HostSeq(start)),
                commit: text("opaque"),
            },
        }
    }

    fn snapshot_events() -> Vec<EventKind> {
        Vec::from([
            created("/a"),
            EventKind::SessionReady,
            EventKind::UserMessage {
                text: text("first"),
                by: ClientName(text("phone")),
            },
            EventKind::AssistantMessage {
                text: text("first reply"),
            },
            EventKind::TurnCompleted {
                outcome: TurnOutcome::Completed,
            },
            snapshot(3),
            EventKind::UserMessage {
                text: text("second"),
                by: ClientName(text("phone")),
            },
            EventKind::AssistantMessage {
                text: text("second reply"),
            },
            EventKind::TurnCompleted {
                outcome: TurnOutcome::Completed,
            },
            snapshot(7),
            EventKind::UserMessage {
                text: text("third"),
                by: ClientName(text("phone")),
            },
            EventKind::AssistantMessage {
                text: text("third reply"),
            },
            EventKind::TurnCompleted {
                outcome: TurnOutcome::Completed,
            },
            snapshot(11),
        ])
    }

    #[test]
    fn a_completed_user_turn_gets_only_its_own_snapshot_in_either_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut events = snapshot_events();
        events.truncate(10);
        let suffix = events.split_off(8);
        let prefix = numbered(events)
            .into_iter()
            .fold(HostView::empty(), HostView::apply);
        let [completion, snapshot] = suffix.as_slice() else {
            return Err("expected completion and snapshot".into());
        };
        for ([first, last], snapshot_seq) in
            [([completion, snapshot], 10), ([snapshot, completion], 9)]
        {
            let event = |kind: &EventKind, seq| Event {
                seq: HostSeq(seq),
                session: SessionId(1),
                kind: kind.clone(),
            };
            let partial = prefix.clone().apply(event(first, 9));
            let partial_view = partial
                .sessions
                .get(&SessionId(1))
                .ok_or("expected a session")?;
            assert_eq!(
                partial_view
                    .completed_turn_since(3)
                    .and_then(|turn| turn.snapshot),
                None
            );
            let complete = partial.apply(event(last, 10));
            let view = complete
                .sessions
                .get(&SessionId(1))
                .ok_or("expected a session")?;
            let turn = view
                .completed_turn_since(3)
                .ok_or("expected a completed turn")?;
            assert_eq!(
                (turn.outcome, turn.snapshot),
                (&TurnOutcome::Completed, Some(HostSeq(snapshot_seq)))
            );
            assert_eq!(turn.items.len(), 2);
        }
        Ok(())
    }

    #[test]
    fn turns_sharing_a_commit_keep_their_event_identity_across_replay()
    -> Result<(), Box<dyn std::error::Error>> {
        let events = numbered(snapshot_events());
        let live = events
            .iter()
            .cloned()
            .fold(HostView::empty(), HostView::apply);
        let before_reconnect = events
            .iter()
            .take(8)
            .cloned()
            .fold(HostView::empty(), HostView::apply);
        let reconnected = events
            .into_iter()
            .skip(8)
            .fold(before_reconnect, HostView::apply);
        assert_eq!(live, reconnected);
        let session =
            live.sessions.get(&SessionId(1)).ok_or("missing session")?;
        assert_eq!(
            session
                .snapshots
                .iter()
                .map(|turn| (turn.snapshot, turn.start, turn.end,))
                .collect::<Vec<_>>(),
            [(HostSeq(6), 0, 2), (HostSeq(10), 3, 5), (HostSeq(14), 6, 8),],
        );
        assert_eq!(session.items.len(), 9);
        Ok(())
    }

    #[test]
    fn a_later_user_turn_does_not_complete_an_abandoned_one()
    -> Result<(), Box<dyn std::error::Error>> {
        let events = snapshot_events()
            .into_iter()
            .enumerate()
            .filter(|(index, _)| !matches!(index, 4 | 5))
            .map(|(_, kind)| kind)
            .collect();
        let host = numbered(events)
            .into_iter()
            .fold(HostView::empty(), HostView::apply);
        let view = host
            .sessions
            .get(&SessionId(1))
            .ok_or("expected a session")?;
        assert!(view.completed_turn_since(0).is_none());
        assert!(view.completed_turn_since(2).is_some());
        Ok(())
    }

    fn numbered(kinds: Vec<EventKind>) -> Vec<Event> {
        (1..)
            .zip(kinds)
            .map(|(seq, kind)| Event {
                seq: HostSeq(seq),
                session: SessionId(1),
                kind,
            })
            .collect()
    }

    #[test]
    fn a_session_without_workspace_events_has_no_changes_controls()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = fold(Vec::from([
            (1, created("/legacy")),
            (1, EventKind::SessionReady),
        ]));
        let session =
            view.sessions.get(&SessionId(1)).ok_or("expected a value")?;
        assert_eq!(session.workspace, None);
        assert_eq!(session.snapshots, Vec::new());
        assert_eq!(session.controls().composer, Composer::Open);
        Ok(())
    }

    #[test]
    fn autonomous_snapshot_has_a_turn_boundary_but_initial_snapshot_does_not()
    -> Result<(), Box<dyn std::error::Error>> {
        let initial = EventKind::SnapshotRecorded {
            snapshot: proto::Snapshot {
                turn_start: None,
                commit: text("initial"),
            },
        };
        let autonomous = EventKind::SnapshotRecorded {
            snapshot: proto::Snapshot {
                turn_start: None,
                commit: text("autonomous"),
            },
        };
        let events = numbered(Vec::from([
            created("/a"),
            initial,
            EventKind::AssistantMessage {
                text: text("autonomous reply"),
            },
            EventKind::TurnCompleted {
                outcome: TurnOutcome::Completed,
            },
            autonomous,
        ]));
        let view = events.into_iter().fold(HostView::empty(), HostView::apply);
        let session =
            view.sessions.get(&SessionId(1)).ok_or("expected a value")?;
        assert_eq!(session.snapshots.len(), 1);
        assert_eq!(
            session.snapshots.first().ok_or("expected a value")?.start,
            0
        );
        assert_eq!(session.snapshots.first().ok_or("expected a value")?.end, 1);
        Ok(())
    }
    #[test]
    fn each_state_has_its_word_class_and_label() {
        assert_eq!(
            [ReplyState::Done.class(), ReplyState::Streaming.class()],
            ["reply", "reply streaming"]
        );
        assert_eq!(
            [
                ToolState::Running,
                ToolState::Finished(ToolOutcome::Succeeded),
                ToolState::Finished(ToolOutcome::Failed),
                ToolState::Finished(ToolOutcome::Declined),
                ToolState::Stopped,
            ]
            .map(ToolState::label),
            ["running", "succeeded", "failed", "declined", "stopped"]
        );
        assert_eq!(
            [
                RequestState::Open.is_open(),
                RequestState::Resolved(Resolution::Withdrawn).is_open()
            ],
            [true, false]
        );
    }

    #[test]
    fn a_session_keeps_its_workspace_and_a_changed_effort()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = |at: &str| HostPath(text(at));
        let workspace = proto::Workspace {
            repository: path("/repo"),
            original: path("/repo"),
            worktree: path("/data/worktrees/1"),
            directory: path("/data/worktrees/1"),
            branch: text("conductor/1"),
        };
        let view = fold(Vec::from([
            (1, created("/repo")),
            (
                1,
                EventKind::WorkspaceCreated {
                    workspace: workspace.clone(),
                },
            ),
            (
                1,
                EventKind::ModelChanged {
                    model: Some(ModelId(text("opus"))),
                    effort: Some(Effort::High),
                    reason: text("Effort set to high"),
                },
            ),
        ]));
        let session =
            view.sessions.get(&SessionId(1)).ok_or("expected a value")?;
        assert_eq!(
            (session.workspace.clone(), session.effort),
            (Some(workspace), Some(Effort::High))
        );
        Ok(())
    }

    #[test]
    fn finish_and_status_follow_the_session_lifecycle()
    -> Result<(), Box<dyn std::error::Error>> {
        let reason = CloseReason::Exited { status: text("0") };
        let cases = [
            (created("/a"), StatusLabel::Starting, false),
            (EventKind::SessionReady, StatusLabel::Idle, false),
            (
                EventKind::OnIssue {
                    issue: proto::IssueId::new(0, 7),
                },
                StatusLabel::Idle,
                true,
            ),
            (
                EventKind::UserMessage {
                    text: text("work"),
                    by: ClientName(text("phone")),
                },
                StatusLabel::Running,
                false,
            ),
            (
                EventKind::SessionClosed {
                    reason: reason.clone(),
                },
                StatusLabel::Closed,
                true,
            ),
            (EventKind::FinishStarted, StatusLabel::Closed, false),
            (
                EventKind::SessionFinished {
                    finished: proto::Finished::Failed {
                        reason: text("merge conflict"),
                    },
                },
                StatusLabel::Closed,
                false,
            ),
        ];
        let mut view = HostView::empty();
        for (index, (kind, status, finish)) in cases.into_iter().enumerate() {
            view = view.apply(Event {
                seq: HostSeq(u64::try_from(index)? + 1),
                session: SessionId(1),
                kind,
            });
            let session =
                view.sessions.get(&SessionId(1)).ok_or("expected a value")?;
            assert_eq!(session.status_label(), status);
            assert_eq!(session.list_status(), ListStatus::Session(status));
            assert_eq!(session.controls().finish, finish);
            assert_eq!(
                session.closed_reason(),
                (status == StatusLabel::Closed).then_some(&reason)
            );
        }
        Ok(())
    }

    #[test]
    fn finishing_on_an_issue_shows_and_a_new_message_makes_the_session_ready_again()
    -> Result<(), Box<dyn std::error::Error>> {
        let failed = proto::Finished::Failed {
            reason: text("merge conflict"),
        };
        let mut events = Vec::from([
            (1, created("/a")),
            (1, EventKind::SessionReady),
            (
                1,
                EventKind::OnIssue {
                    issue: proto::IssueId::new(0, 7),
                },
            ),
            (1, EventKind::FinishStarted),
        ]);
        let finishing = fold(events.clone());
        let session = finishing
            .sessions
            .get(&SessionId(1))
            .ok_or("expected a value")?;
        assert_eq!(
            (session.issue, session.finish, session.items.clone()),
            (
                Some(proto::IssueId::new(0, 7)),
                FinishState::Finishing,
                Vec::from([Item::Finishing])
            )
        );
        events.extend([(
            1,
            EventKind::SessionFinished {
                finished: failed.clone(),
            },
        )]);
        let finished = fold(events.clone());
        let session = finished
            .sessions
            .get(&SessionId(1))
            .ok_or("expected a value")?;
        assert_eq!(
            (session.finish, session.items.clone()),
            (
                FinishState::Done,
                Vec::from([Item::Finishing, Item::Finished(failed)])
            )
        );
        events.push((
            1,
            EventKind::UserMessage {
                text: text("more"),
                by: ClientName(text("phone")),
            },
        ));
        let again = fold(events);
        let session = again
            .sessions
            .get(&SessionId(1))
            .ok_or("expected a value")?;
        assert_eq!(session.finish, FinishState::Ready);
        Ok(())
    }
}
