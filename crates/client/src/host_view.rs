//! A host's sessions folded from its events: what the UI renders. No UI
//! types, so it is unit-testable and shared by every client target.

use crate::prelude::*;
use alloc::collections::BTreeMap;
use core::cmp::Reverse;

use proto::{
    CheckpointId, ClientName, CloseReason, Effort, Event, EventKind,
    FinishState, Finished, HostPath, HostSeq, InstanceId, IssueId, ModelId,
    Provider, Question, RequestId, Resolution, SessionId, SubagentActivity,
    SubagentEvent, SubagentId, SubagentOutcome, Task, ToolCallId, ToolKind,
    ToolOutcome, ToolPhase, TurnOutcome, UndoAvailability, Workspace,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostView {
    pub sessions: BTreeMap<SessionId, SessionView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    /// The instance it runs on, and that instance's provider.
    pub instance: InstanceId,
    pub provider: Provider,
    pub directory: HostPath,
    /// What the agent runs with; `None` is the CLI's own default.
    pub model: Option<ModelId>,
    pub effort: Option<Effort>,
    status: SessionStatus,
    pub issue: Option<IssueId>,
    finish: FinishState,
    /// The tasks the session holds, in its agent's order.
    pub tasks: Vec<Task>,
    pub workspace: Option<Workspace>,
    pub checkpoints: Vec<TurnCheckpoint>,
    pub undo: UndoState,
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
pub struct TurnCheckpoint {
    pub checkpoint: CheckpointId,
    pub start: usize,
    pub end: usize,
    pub undo: UndoAvailability,
}

/// A user turn's transcript, with its checkpoint once the host has saved it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompletedTurn<'a> {
    pub items: &'a [Item],
    pub outcome: &'a TurnOutcome,
    pub checkpoint: Option<CheckpointId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoState {
    Ready,
    Undoing(CheckpointId),
    Failed {
        checkpoint: CheckpointId,
        reason: String,
    },
    Undone,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLabel {
    Starting,
    Idle,
    Running,
    Closed,
    Undoing,
    RecoveryNeeded,
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
    pub fn apply(&mut self, event: Event) {
        match event.kind {
            EventKind::SessionCreated {
                instance,
                provider,
                directory,
                model,
                effort,
                ..
            } => {
                self.sessions.insert(
                    event.session,
                    SessionView {
                        instance,
                        provider,
                        directory,
                        model,
                        effort,
                        status: SessionStatus::Starting,
                        issue: None,
                        finish: FinishState::Ready,
                        tasks: Vec::new(),
                        workspace: None,
                        checkpoints: Vec::new(),
                        undo: UndoState::Ready,
                        turn_starts: BTreeMap::new(),
                        items: Vec::new(),
                        subagents: Vec::new(),
                        activity: event.seq,
                    },
                );
            }
            EventKind::SessionRemoved { .. } => {
                self.sessions.remove(&event.session);
            }
            kind => {
                if let Some(view) = self.sessions.get_mut(&event.session) {
                    if !is_delta(&kind) {
                        view.activity = event.seq;
                    }
                    view.apply(kind, event.seq);
                }
            }
        }
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
        let (composer, interrupt, resume) = match (&self.undo, &self.status) {
            (UndoState::Undoing(_) | UndoState::Failed { .. }, _)
            | (_, SessionStatus::Starting) => {
                (Composer::Busy, Interrupt::Hidden, Resume::Hidden)
            }
            (_, SessionStatus::Idle) => {
                (Composer::Open, Interrupt::Hidden, Resume::Hidden)
            }
            (_, SessionStatus::Running) => {
                (Composer::Busy, Interrupt::Offered, Resume::Hidden)
            }
            (_, SessionStatus::Closed(_)) => {
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
                && matches!(self.undo, UndoState::Ready | UndoState::Undone)
                && matches!(self.finish, FinishState::Ready),
        }
    }

    #[must_use]
    pub const fn status_label(&self) -> StatusLabel {
        match (&self.undo, &self.status) {
            (UndoState::Undoing(_), _) => StatusLabel::Undoing,
            (UndoState::Failed { .. }, _) => StatusLabel::RecoveryNeeded,
            (_, SessionStatus::Starting) => StatusLabel::Starting,
            (_, SessionStatus::Idle) => StatusLabel::Idle,
            (_, SessionStatus::Running) => StatusLabel::Running,
            (_, SessionStatus::Closed(_)) => StatusLabel::Closed,
        }
    }

    #[must_use]
    pub fn list_status(&self) -> ListStatus {
        let label = self.status_label();
        match label {
            StatusLabel::Undoing | StatusLabel::RecoveryNeeded => {
                ListStatus::Session(label)
            }
            StatusLabel::Starting
            | StatusLabel::Idle
            | StatusLabel::Running
            | StatusLabel::Closed => match self.attention() {
                Attention::Quiet => ListStatus::Session(label),
                Attention::Requests(open) => ListStatus::Requests(open),
            },
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

    #[must_use]
    pub const fn can_view_changes(&self) -> bool {
        !matches!(self.undo, UndoState::Undoing(_))
    }

    /// A running turn shows it is working, unless it waits on a person or
    /// its last item is live already: a reply or reasoning streaming in, or
    /// a tool call running.
    #[must_use]
    pub fn working(&self) -> Working {
        match self.undo {
            UndoState::Undoing(_) | UndoState::Failed { .. } => Working::Hidden,
            UndoState::Ready | UndoState::Undone => self.turn_working(),
        }
    }

    fn turn_working(&self) -> Working {
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
            .items
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    Item::Tool {
                        approval: Approval::Asked {
                            state: RequestState::Open,
                            ..
                        },
                        ..
                    } | Item::Question {
                        state: RequestState::Open,
                        ..
                    }
                )
            })
            .count();
        match open {
            0 => Attention::Quiet,
            count => Attention::Requests(count),
        }
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
        let checkpoint = self
            .checkpoints
            .iter()
            .find(|turn| turn.start == since.saturating_add(start))
            .map(|turn| turn.checkpoint);
        Some(CompletedTurn {
            items: items.get(..end)?,
            outcome,
            checkpoint,
        })
    }

    fn apply(&mut self, kind: EventKind, seq: HostSeq) {
        self.finish = self.finish.after(&kind);
        match kind {
            EventKind::SessionReady => {
                if let SessionStatus::Closed(reason) =
                    core::mem::replace(&mut self.status, SessionStatus::Idle)
                {
                    self.push(Item::Continued(reason));
                }
            }
            EventKind::ModelChanged {
                model,
                effort,
                reason,
            } => {
                self.model = model;
                self.effort = effort;
                self.push(Item::ModelChanged(reason));
            }
            EventKind::SessionClosed { reason } => self.close(reason),
            EventKind::UserMessage { text, by } => {
                self.turn_starts.insert(seq, self.items.len());
                self.undo = UndoState::Ready;
                self.status = SessionStatus::Running;
                self.push(Item::User { text, by });
            }
            EventKind::AssistantDelta { text } => self.push(Item::Reply {
                text,
                state: ReplyState::Streaming,
            }),
            EventKind::AssistantMessage { text } => self.push(Item::Reply {
                text,
                state: ReplyState::Done,
            }),
            EventKind::ReasoningDelta { text } => self.push(Item::Reasoning {
                text,
                state: ReplyState::Streaming,
            }),
            EventKind::Reasoning { text } => self.push(Item::Reasoning {
                text,
                state: ReplyState::Done,
            }),
            EventKind::ToolCall { call, phase } => {
                tool(&mut self.items, call, phase);
            }
            EventKind::RequestOpened {
                request,
                call,
                tool,
                reason,
                for_session,
            } => self.ask(call, tool, request, reason, for_session),
            EventKind::QuestionAsked { request, questions } => {
                self.push(Item::Question {
                    request,
                    questions,
                    state: RequestState::Open,
                });
            }
            EventKind::RequestResolved {
                request,
                resolution,
            } => {
                self.resolve(request, &resolution);
            }
            EventKind::TurnCompleted { outcome } => {
                end(&mut self.items);
                self.status = SessionStatus::Idle;
                self.push(Item::TurnCompleted(outcome));
            }
            EventKind::WorkspaceCreated { workspace } => {
                self.workspace = Some(workspace);
            }
            EventKind::TasksChanged { tasks } => self.tasks = tasks,
            EventKind::OnIssue { issue } => self.issue = Some(issue),
            EventKind::FinishStarted => self.push(Item::Finishing),
            EventKind::SessionFinished { finished } => {
                self.push(Item::Finished(finished));
            }
            EventKind::EffortChanged { effort } => self.effort = Some(effort),
            EventKind::CheckpointCreated { checkpoint } => {
                self.checkpoint_created(checkpoint);
            }
            EventKind::UndoStarted { checkpoint, .. } => {
                self.undo = UndoState::Undoing(checkpoint);
            }
            EventKind::UndoFailed { checkpoint, reason } => {
                self.undo = UndoState::Failed { checkpoint, reason }
            }
            EventKind::TurnsUndone { checkpoint, .. } => {
                self.turns_undone(checkpoint);
            }
            EventKind::Error { message } => self.push(Item::Error(message)),
            EventKind::Subagent { agent, event } => self.subagent(agent, event),
            EventKind::SessionCreated { .. }
            | EventKind::SessionRemoved { .. }
            | EventKind::UndoHistoryRestored { .. }
            | EventKind::CheckpointReserved { .. }
            | EventKind::HistoryMaterialized { .. }
            | EventKind::NativeTurnStarted { .. } => {}
        }
    }

    fn close(&mut self, reason: CloseReason) {
        end(&mut self.items);
        for agent in &mut self.subagents {
            if agent.status == SubagentStatus::Running {
                agent.end(SubagentStatus::Stopped, None);
            }
        }
        self.status = SessionStatus::Closed(reason);
    }

    fn subagent(&mut self, id: SubagentId, event: SubagentEvent) {
        // A child's finished tool may also have an approval card in the parent's transcript.
        if let SubagentEvent::Activity(SubagentActivity::ToolCall {
            call,
            phase: ToolPhase::Finished { outcome },
        }) = &event
        {
            finish_tool(&mut self.items, call, *outcome);
        }
        let known = self.subagents.iter_mut().find(|agent| agent.id == id);
        match (known, event) {
            (Some(agent), SubagentEvent::Started { .. }) => {
                agent.status = SubagentStatus::Running;
            }
            (None, SubagentEvent::Started { call, description }) => {
                self.subagents.push(Subagent {
                    id,
                    call,
                    description,
                    status: SubagentStatus::Running,
                    items: Vec::new(),
                });
            }
            (Some(agent), SubagentEvent::Activity(activity)) => {
                agent.did(activity);
            }
            (Some(agent), SubagentEvent::Ended { outcome, summary }) => {
                agent.end(SubagentStatus::Ended(outcome), summary);
            }
            (None, _) => {}
        }
    }

    /// A checkpoint covers the items since its turn started, or since the
    /// last checkpoint when the host names no turn start.
    fn checkpoint_created(&mut self, checkpoint: proto::Checkpoint) {
        let start = checkpoint
            .turn_start
            .and_then(|seq| self.turn_starts.get(&seq).copied())
            .or_else(|| {
                checkpoint.parent.map(|_| {
                    self.checkpoints
                        .last()
                        .map_or(0, |turn| turn.end.saturating_add(1))
                })
            });
        let Some(first) = start else {
            return;
        };
        self.checkpoints.push(TurnCheckpoint {
            checkpoint: checkpoint.id,
            undo: checkpoint.undo,
            start: first,
            end: self.items.len().saturating_sub(1),
        });
    }

    fn turns_undone(&mut self, checkpoint: CheckpointId) {
        self.undo = UndoState::Undone;
        if let Some(turn) = self
            .checkpoints
            .iter()
            .find(|turn| turn.checkpoint == checkpoint)
        {
            let start = turn.start;
            self.items.truncate(start);
            self.subagents.retain(|agent| {
                self.items
                    .iter()
                    .filter_map(Item::call)
                    .any(|call| call == &agent.call)
            });
            self.checkpoints.retain(|turn| turn.start < start);
            self.turn_starts.retain(|_, index| *index < start);
            self.status = SessionStatus::Idle;
        }
    }

    // Consecutive deltas extend the same text buffer. The final message replaces
    // them so live and replayed transcripts agree even after missed deltas.
    fn push(&mut self, item: Item) {
        match (self.items.last_mut(), item) {
            (
                Some(Item::Reply {
                    text,
                    state: previous @ ReplyState::Streaming,
                }),
                Item::Reply { text: piece, state },
            )
            | (
                Some(Item::Reasoning {
                    text,
                    state: previous @ ReplyState::Streaming,
                }),
                Item::Reasoning { text: piece, state },
            ) => {
                match state {
                    ReplyState::Streaming => text.push_str(&piece),
                    ReplyState::Done => *text = piece,
                }
                *previous = state;
            }
            (_, next) => {
                settle(&mut self.items);
                self.items.push(next);
            }
        }
    }

    fn resolve(&mut self, request: RequestId, resolution: &Resolution) {
        for (id, state) in self.items.iter_mut().filter_map(Item::request_mut) {
            if id == request {
                *state = RequestState::Resolved(resolution.clone());
            }
        }
    }

    fn ask(
        &mut self,
        call: ToolCallId,
        tool: ToolKind,
        request: RequestId,
        reason: Option<String>,
        for_session: Option<String>,
    ) {
        let approval = Approval::Asked {
            request,
            reason,
            for_session,
            state: RequestState::Open,
        };
        let mut found = false;
        for item in &mut self.items {
            if let Item::Tool {
                call: id,
                approval: card,
                ..
            } = item
                && *id == call
            {
                card.clone_from(&approval);
                found = true;
            }
        }
        if !found {
            self.push(Item::Tool {
                call,
                tool,
                state: ToolState::Running,
                approval,
            });
        }
    }
}

impl Subagent {
    fn did(&mut self, activity: SubagentActivity) {
        match activity {
            SubagentActivity::Message { text } => {
                self.items.push(Item::Reply {
                    text,
                    state: ReplyState::Done,
                });
            }
            SubagentActivity::Reasoning { text } => {
                self.items.push(Item::Reasoning {
                    text,
                    state: ReplyState::Done,
                });
            }
            SubagentActivity::ToolCall { call, phase } => {
                tool(&mut self.items, call, phase);
            }
        }
    }

    fn end(&mut self, status: SubagentStatus, summary: Option<String>) {
        self.status = status;
        end(&mut self.items);
        if let Some(text) = summary {
            self.items.push(Item::Reply {
                text,
                state: ReplyState::Done,
            });
        }
    }
}

fn tool(items: &mut Vec<Item>, call: ToolCallId, phase: ToolPhase) {
    match phase {
        ToolPhase::Started { tool } => {
            settle(items);
            items.push(Item::Tool {
                call,
                tool,
                state: ToolState::Running,
                approval: Approval::NotAsked,
            });
        }
        ToolPhase::Finished { outcome } => finish_tool(items, &call, outcome),
    }
}

fn finish_tool(items: &mut [Item], call: &ToolCallId, outcome: ToolOutcome) {
    for item in items {
        if let Item::Tool {
            call: id, state, ..
        } = item
            && id == call
        {
            *state = ToolState::Finished(outcome);
        }
    }
}

const fn settle(items: &mut [Item]) {
    if let Some(Item::Reply { state, .. } | Item::Reasoning { state, .. }) =
        items.last_mut()
    {
        *state = ReplyState::Done;
    }
}

fn end(items: &mut [Item]) {
    settle(items);
    for item in items {
        if let Item::Tool {
            state: state @ ToolState::Running,
            ..
        } = item
        {
            *state = ToolState::Stopped;
        }
        if let Some((_, state @ RequestState::Open)) = item.request_mut() {
            *state = RequestState::Resolved(Resolution::Withdrawn);
        }
    }
}

impl Item {
    #[must_use]
    pub const fn call(&self) -> Option<&ToolCallId> {
        if let Self::Tool { call, .. } = self {
            Some(call)
        } else {
            None
        }
    }

    const fn request_mut(&mut self) -> Option<(RequestId, &mut RequestState)> {
        match self {
            Self::Tool {
                approval: Approval::Asked { request, state, .. },
                ..
            }
            | Self::Question { request, state, .. } => Some((*request, state)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;

    use proto::{
        Answer, ClientName, CloseReason, Decision, Effort, Event, EventKind,
        FinishState, HostPath, HostSeq, InstanceId, ModelId, Provider,
        Question, QuestionOption, RequestId, Resolution, Selection, SessionId,
        SubagentActivity, SubagentEvent, SubagentId, SubagentOutcome, Task,
        TaskId, TaskStatus, ToolCallId, ToolKind, ToolOutcome, ToolPhase,
        TurnOutcome,
    };

    use super::{
        Approval, Attention, Composer, HostView, Interrupt, Item, ListStatus,
        ReplyState, RequestState, Resume, SessionStatus, SessionView,
        StatusLabel, Subagent, SubagentStatus, ToolState, UndoState, Working,
    };

    fn applied(mut view: HostView, event: Event) -> HostView {
        view.apply(event);
        view
    }

    fn text(text: &str) -> String {
        String::from(text)
    }

    fn created(directory: &str) -> EventKind {
        EventKind::SessionCreated {
            instance: InstanceId::own(Provider::Claude),
            provider: Provider::Claude,
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
                    provider: Provider::Claude,
                    directory: HostPath(text("/a")),
                    model: None,
                    effort: None,
                    status: SessionStatus::Idle,
                    issue: None,
                    finish: FinishState::Ready,
                    tasks: Vec::new(),
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
                    checkpoints: Vec::new(),
                    undo: UndoState::Ready,
                    turn_starts: BTreeMap::from([(HostSeq(4), 0)]),
                    activity: HostSeq(13),
                },
            ),
            (
                SessionId(2),
                SessionView {
                    instance: InstanceId::own(Provider::Claude),
                    provider: Provider::Claude,
                    directory: HostPath(text("/b")),
                    model: None,
                    effort: None,
                    status: SessionStatus::Closed(CloseReason::Exited {
                        status: text("1"),
                    }),
                    issue: None,
                    finish: FinishState::Ready,
                    tasks: Vec::new(),
                    subagents: Vec::new(),
                    items: Vec::from([Item::Error(text("odd"))]),
                    workspace: None,
                    checkpoints: Vec::new(),
                    undo: UndoState::Ready,
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
                (applied(view, event), seq)
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

    fn task(id: u128, text: &str, status: TaskStatus) -> Task {
        Task {
            id: TaskId(id),
            text: String::from(text),
            status,
            holder: SessionId(1),
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
            // The request for the sub-agent's call has a card of its own in
            // the session, which the sub-agent's result finishes.
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
                id: SubagentId(text("a")),
                call: ToolCallId(text("toolu_agent_a")),
                description: text("look for a"),
                status: SubagentStatus::Ended(SubagentOutcome::Finished),
                items: Vec::from([
                    Item::Reasoning {
                        text: text("where?"),
                        state: ReplyState::Done,
                    },
                    ls("toolu_a1", ToolState::Finished(ToolOutcome::Succeeded)),
                    done_reply("found it"),
                    done_reply("it is in /a"),
                ]),
            },
            Subagent {
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
                    Some((
                        text("toolu_a1"),
                        ToolState::Finished(ToolOutcome::Succeeded)
                    )),
                ]),
            ))
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

    /// Each change replaces the tasks, and a new effort shows in the
    /// header, with no transcript item for either.
    #[test]
    fn tasks_are_replaced_and_the_effort_follows_its_last_change() {
        let first = Vec::from([
            task(1, "write a.txt", TaskStatus::InProgress),
            task(2, "write b.txt", TaskStatus::Pending),
        ]);
        let later = Vec::from([task(1, "write a.txt", TaskStatus::Completed)]);
        let view = fold(Vec::from([
            (1, created("/a")),
            (1, EventKind::SessionReady),
            (1, EventKind::TasksChanged { tasks: first }),
            (
                1,
                EventKind::TasksChanged {
                    tasks: later.clone(),
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
                session.tasks.clone(),
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

    fn checkpoint(id: u64, start: u64, parent: u64) -> EventKind {
        EventKind::CheckpointCreated {
            checkpoint: proto::Checkpoint {
                id: proto::CheckpointId(id),
                parent: Some(proto::CheckpointId(parent)),
                turn_start: Some(HostSeq(start)),
                commit: text("opaque"),
                history: proto::HistoryAnchor::Fresh,
                undo: proto::UndoAvailability::Available,
            },
        }
    }

    fn checkpoint_events() -> Vec<EventKind> {
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
            checkpoint(1, 3, 0),
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
            checkpoint(2, 7, 1),
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
            checkpoint(3, 11, 2),
        ])
    }

    #[test]
    fn a_completed_user_turn_gets_only_its_own_checkpoint_in_either_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut events = checkpoint_events();
        events.truncate(10);
        let suffix = events.split_off(8);
        let prefix = numbered(events)
            .into_iter()
            .fold(HostView::empty(), applied);
        let [completion, checkpoint] = suffix.as_slice() else {
            return Err("expected completion and checkpoint".into());
        };
        for [first, last] in
            [[completion, checkpoint], [checkpoint, completion]]
        {
            let event = |kind: &EventKind, seq| Event {
                seq: HostSeq(seq),
                session: SessionId(1),
                kind: kind.clone(),
            };
            let partial = applied(prefix.clone(), event(first, 9));
            let partial_view = partial
                .sessions
                .get(&SessionId(1))
                .ok_or("expected a session")?;
            assert_eq!(
                partial_view
                    .completed_turn_since(3)
                    .and_then(|turn| turn.checkpoint),
                None
            );
            let complete = applied(partial, event(last, 10));
            let view = complete
                .sessions
                .get(&SessionId(1))
                .ok_or("expected a session")?;
            let turn = view
                .completed_turn_since(3)
                .ok_or("expected a completed turn")?;
            assert_eq!(
                (turn.outcome, turn.checkpoint),
                (&TurnOutcome::Completed, Some(proto::CheckpointId(2)))
            );
            assert_eq!(turn.items.len(), 2);
        }
        Ok(())
    }

    #[test]
    fn a_later_user_turn_does_not_complete_an_abandoned_one()
    -> Result<(), Box<dyn std::error::Error>> {
        let events = checkpoint_events()
            .into_iter()
            .enumerate()
            .filter(|(index, _)| !matches!(index, 4 | 5))
            .map(|(_, kind)| kind)
            .collect();
        let host = numbered(events)
            .into_iter()
            .fold(HostView::empty(), applied);
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
    fn undo_removes_selected_and_later_turns_on_live_fold_replay_and_reconnect()
    -> Result<(), Box<dyn std::error::Error>> {
        let events = numbered(
            checkpoint_events()
                .into_iter()
                .chain([
                    EventKind::UndoStarted {
                        checkpoint: proto::CheckpointId(2),
                        restore: proto::CheckpointId(1),
                        history: proto::HistoryAnchor::Fresh,
                    },
                    EventKind::UndoHistoryRestored {
                        checkpoint: proto::CheckpointId(2),
                        history: proto::HistoryAnchor::Fresh,
                    },
                    EventKind::TurnsUndone {
                        checkpoint: proto::CheckpointId(2),
                        restored: proto::CheckpointId(1),
                    },
                ])
                .collect(),
        );
        let live = events.clone().into_iter().fold(HostView::empty(), applied);
        let before_reconnect = events
            .iter()
            .take(15)
            .cloned()
            .fold(HostView::empty(), applied);
        let reconnect =
            events.into_iter().skip(15).fold(before_reconnect, applied);
        assert_eq!(live, reconnect);
        let session =
            live.sessions.get(&SessionId(1)).ok_or("expected a value")?;
        assert_eq!(session.items.len(), 3);
        assert_eq!(session.checkpoints.len(), 1);
        assert_eq!(
            session
                .checkpoints
                .first()
                .ok_or("expected a value")?
                .checkpoint,
            proto::CheckpointId(1)
        );
        assert_eq!(session.undo, super::UndoState::Undone);
        assert_eq!(session.controls().composer, Composer::Open);
        Ok(())
    }

    #[test]
    fn in_progress_and_failed_undo_preserve_transcript_and_block_controls()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut original = numbered(checkpoint_events())
            .into_iter()
            .fold(HostView::empty(), applied);
        original.apply(Event {
            seq: HostSeq(15),
            session: SessionId(1),
            kind: EventKind::OnIssue {
                issue: proto::IssueId(7),
            },
        });
        let session = original
            .sessions
            .get(&SessionId(1))
            .ok_or("expected a value")?;
        assert!(session.controls().finish);
        let pending = applied(
            original.clone(),
            Event {
                seq: HostSeq(16),
                session: SessionId(1),
                kind: EventKind::UndoStarted {
                    checkpoint: proto::CheckpointId(2),
                    restore: proto::CheckpointId(1),
                    history: proto::HistoryAnchor::Fresh,
                },
            },
        );
        let session = pending
            .sessions
            .get(&SessionId(1))
            .ok_or("expected a value")?;
        assert_eq!(session.items.len(), 9);
        assert_eq!(session.status_label(), StatusLabel::Undoing);
        assert_eq!(
            session.list_status(),
            ListStatus::Session(StatusLabel::Undoing)
        );
        assert_eq!(
            session.controls(),
            super::Controls {
                composer: Composer::Busy,
                interrupt: Interrupt::Hidden,
                resume: Resume::Hidden,
                finish: false,
            }
        );
        let failed = applied(
            pending,
            Event {
                seq: HostSeq(17),
                session: SessionId(1),
                kind: EventKind::UndoFailed {
                    checkpoint: proto::CheckpointId(2),
                    reason: text("restore failed"),
                },
            },
        );
        let session = failed
            .sessions
            .get(&SessionId(1))
            .ok_or("expected a value")?;
        assert_eq!(
            session.items,
            original
                .sessions
                .get(&SessionId(1))
                .ok_or("expected a value")?
                .items
        );
        assert_eq!(session.checkpoints.len(), 3);
        assert_eq!(
            session.undo,
            super::UndoState::Failed {
                checkpoint: proto::CheckpointId(2),
                reason: text("restore failed")
            }
        );
        assert_eq!(session.controls().composer, Composer::Busy);
        assert!(!session.controls().finish);
        assert_eq!(session.status_label(), StatusLabel::RecoveryNeeded);
        assert_eq!(
            session.list_status(),
            ListStatus::Session(StatusLabel::RecoveryNeeded)
        );
        Ok(())
    }

    #[test]
    fn legacy_session_stays_usable_without_workspace_or_checkpoint_controls()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = fold(Vec::from([
            (1, created("/legacy")),
            (1, EventKind::SessionReady),
        ]));
        let session =
            view.sessions.get(&SessionId(1)).ok_or("expected a value")?;
        assert_eq!(session.workspace, None);
        assert_eq!(session.checkpoints, Vec::new());
        assert_eq!(session.controls().composer, Composer::Open);
        Ok(())
    }

    #[test]
    fn autonomous_checkpoint_has_a_turn_boundary_but_initial_checkpoint_does_not()
    -> Result<(), Box<dyn std::error::Error>> {
        let initial = EventKind::CheckpointCreated {
            checkpoint: proto::Checkpoint {
                id: proto::CheckpointId(0),
                parent: None,
                turn_start: None,
                commit: text("initial"),
                history: proto::HistoryAnchor::Fresh,
                undo: proto::UndoAvailability::Available,
            },
        };
        let autonomous = EventKind::CheckpointCreated {
            checkpoint: proto::Checkpoint {
                id: proto::CheckpointId(1),
                parent: Some(proto::CheckpointId(0)),
                turn_start: None,
                commit: text("autonomous"),
                history: proto::HistoryAnchor::Fresh,
                undo: proto::UndoAvailability::Available,
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
        let view = events.into_iter().fold(HostView::empty(), applied);
        let session =
            view.sessions.get(&SessionId(1)).ok_or("expected a value")?;
        assert_eq!(session.checkpoints.len(), 1);
        assert_eq!(
            session.checkpoints.first().ok_or("expected a value")?.start,
            0
        );
        assert_eq!(
            session.checkpoints.first().ok_or("expected a value")?.end,
            1
        );
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
                    issue: proto::IssueId(7),
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
            view.apply(Event {
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
                    issue: proto::IssueId(7),
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
                Some(proto::IssueId(7)),
                FinishState::Finishing,
                Vec::from([Item::Finishing])
            )
        );
        events.extend([
            (
                1,
                EventKind::SessionFinished {
                    finished: failed.clone(),
                },
            ),
            (
                1,
                EventKind::UndoFailed {
                    checkpoint: proto::CheckpointId(1),
                    reason: text("restore failed"),
                },
            ),
        ]);
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
        assert_eq!(
            (session.finish, session.undo.clone()),
            (FinishState::Ready, UndoState::Ready)
        );
        Ok(())
    }

    #[test]
    fn an_undo_keeps_only_the_earlier_turns_sub_agents_and_starts_and_idles()
    -> Result<(), Box<dyn std::error::Error>> {
        let said = |words: &str| EventKind::UserMessage {
            text: text(words),
            by: ClientName(text("phone")),
        };
        let completed = || EventKind::TurnCompleted {
            outcome: TurnOutcome::Completed,
        };
        let undone = numbered(Vec::from([
            created("/a"),
            EventKind::SessionReady,
            EventKind::OnIssue {
                issue: proto::IssueId(7),
            },
            said("first"),
            started("call1"),
            subagent_started("a", "call1"),
            completed(),
            checkpoint(1, 4, 0),
            said("second"),
            started("call2"),
            subagent_started("b", "call2"),
            completed(),
            checkpoint(2, 9, 1),
            said("third"),
            EventKind::TurnsUndone {
                checkpoint: proto::CheckpointId(2),
                restored: proto::CheckpointId(1),
            },
        ]))
        .into_iter()
        .fold(HostView::empty(), applied);
        let session = undone
            .sessions
            .get(&SessionId(1))
            .ok_or("expected a value")?;
        assert_eq!(
            session
                .subagents
                .iter()
                .map(|subagent| subagent.id.0.as_str())
                .collect::<Vec<_>>(),
            Vec::from(["a"])
        );
        assert_eq!(
            session.turn_starts.keys().copied().collect::<Vec<_>>(),
            Vec::from([HostSeq(4)])
        );
        assert_eq!(
            (session.status.clone(), session.undo.clone()),
            (SessionStatus::Idle, UndoState::Undone)
        );
        assert_eq!(session.status_label(), StatusLabel::Idle);
        assert!(session.controls().finish);
        Ok(())
    }

    #[test]
    fn an_undo_of_a_checkpoint_the_view_lacks_still_ends()
    -> Result<(), Box<dyn std::error::Error>> {
        let view = numbered(
            checkpoint_events()
                .into_iter()
                .chain([
                    EventKind::UndoStarted {
                        checkpoint: proto::CheckpointId(9),
                        restore: proto::CheckpointId(1),
                        history: proto::HistoryAnchor::Fresh,
                    },
                    EventKind::TurnsUndone {
                        checkpoint: proto::CheckpointId(9),
                        restored: proto::CheckpointId(1),
                    },
                ])
                .collect(),
        )
        .into_iter()
        .fold(HostView::empty(), applied);
        let session =
            view.sessions.get(&SessionId(1)).ok_or("expected a value")?;
        assert_eq!(
            (session.items.len(), session.undo.clone()),
            (9, UndoState::Undone)
        );
        Ok(())
    }
}
