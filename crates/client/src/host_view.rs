//! A host's sessions folded from its events: what the UI renders. No UI
//! types, so it is unit-testable and shared by every client target.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use proto::{
    CheckpointId, ClientName, CloseReason, Effort, Event, EventKind, HostPath, HostSeq, ModelId,
    Provider, Question, RequestId, Resolution, SessionId, ToolCallId, ToolKind, ToolOutcome,
    ToolPhase, TurnOutcome, UndoAvailability, Workspace,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostView {
    pub sessions: BTreeMap<SessionId, SessionView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    pub provider: Provider,
    pub directory: HostPath,
    /// What the agent runs with; `None` is the CLI's own default.
    pub model: Option<ModelId>,
    pub effort: Option<Effort>,
    pub status: SessionStatus,
    pub workspace: Option<Workspace>,
    pub checkpoints: Vec<TurnCheckpoint>,
    pub undo: UndoState,
    turn_starts: BTreeMap<HostSeq, usize>,
    pub items: Vec<Item>,
    /// The last event that touched this session.
    pub activity: HostSeq,
}

/// A completed turn and its transcript boundaries; provider identifiers stay opaque.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnCheckpoint {
    pub checkpoint: CheckpointId,
    pub start: usize,
    pub end: usize,
    pub undo: UndoAvailability,
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
pub enum SessionStatus {
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
    Error(String),
}

/// A reply or reasoning streams until its final text arrives or something
/// else follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyState {
    Streaming,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Finished(ToolOutcome),
    /// The turn or the session ended before the call finished.
    Stopped,
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

impl SessionStatus {
    /// The status in one word, for the UI's text and its `data-status`.
    pub fn word(&self) -> String {
        String::from(match self {
            SessionStatus::Starting => "starting",
            SessionStatus::Idle => "idle",
            SessionStatus::Running => "running",
            SessionStatus::Closed(_) => "closed",
        })
    }
}

/// What the UI offers for a session, decided here so markup only shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Controls {
    pub composer: Composer,
    pub interrupt: Interrupt,
    pub resume: Resume,
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

impl HostView {
    pub fn empty() -> HostView {
        HostView {
            sessions: BTreeMap::new(),
        }
    }

    /// An event for a session this view never saw created is dropped.
    pub fn apply(self, event: Event) -> HostView {
        let mut sessions = self.sessions;
        let session = match (sessions.remove(&event.session), event.kind) {
            (
                _,
                EventKind::SessionCreated {
                    provider,
                    directory,
                    model,
                    effort,
                    ..
                },
            ) => Some(SessionView {
                provider,
                directory,
                model,
                effort,
                status: SessionStatus::Starting,
                workspace: None,
                checkpoints: Vec::new(),
                undo: UndoState::Ready,
                turn_starts: BTreeMap::new(),
                items: Vec::new(),
                activity: event.seq,
            }),
            // A delta repeats the seq of the event before it, which may be
            // another session's, so it is not this session's activity.
            (Some(view), EventKind::AssistantDelta { text }) => {
                Some(view.reply(text, ReplyState::Streaming))
            }
            (Some(view), EventKind::ReasoningDelta { text }) => {
                Some(view.reasoning(text, ReplyState::Streaming))
            }
            (Some(view), kind) => Some(SessionView {
                activity: event.seq,
                ..view.apply(kind, event.seq)
            }),
            (None, _) => None,
        };
        sessions.extend(session.map(|view| (event.session, view)));
        HostView { sessions }
    }

    /// The sessions' ids, the most recently active first.
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
    pub fn directories(&self) -> Vec<HostPath> {
        self.recent()
            .iter()
            .filter_map(|id| self.sessions.get(id))
            .fold(Vec::new(), |known, view| {
                match known.iter().find(|path| path.0 == view.directory.0) {
                    Some(_) => known,
                    None => known.into_iter().chain([view.directory.clone()]).collect(),
                }
            })
    }
}

impl SessionView {
    pub fn controls(&self) -> Controls {
        let (composer, interrupt, resume) = match (&self.undo, &self.status) {
            (UndoState::Undoing(_) | UndoState::Failed { .. }, _) => {
                (Composer::Busy, Interrupt::Hidden, Resume::Hidden)
            }
            (_, SessionStatus::Starting) => (Composer::Busy, Interrupt::Hidden, Resume::Hidden),
            (_, SessionStatus::Idle) => (Composer::Open, Interrupt::Hidden, Resume::Hidden),
            (_, SessionStatus::Running) => (Composer::Busy, Interrupt::Offered, Resume::Hidden),
            (_, SessionStatus::Closed(_)) => (Composer::Closed, Interrupt::Hidden, Resume::Offered),
        };
        Controls {
            composer,
            interrupt,
            resume,
        }
    }

    /// A running turn shows it is working, unless it waits on a person or
    /// its last item is live already: a reply or reasoning streaming in, or
    /// a tool call running.
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
            ) => Working::Hidden,
            (SessionStatus::Running, Attention::Quiet, _) => Working::Shown,
            (SessionStatus::Running, Attention::Requests(_), _) => Working::Hidden,
            (SessionStatus::Starting | SessionStatus::Idle | SessionStatus::Closed(_), _, _) => {
                Working::Hidden
            }
        }
    }

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
            open => Attention::Requests(open),
        }
    }

    fn apply(self, kind: EventKind, seq: HostSeq) -> SessionView {
        match kind {
            EventKind::SessionCreated { .. } => self,
            EventKind::SessionReady => match self.status.clone() {
                SessionStatus::Closed(reason) => self
                    .status(SessionStatus::Idle)
                    .push(Item::Continued(reason)),
                SessionStatus::Starting | SessionStatus::Idle | SessionStatus::Running => {
                    self.status(SessionStatus::Idle)
                }
            },
            EventKind::ModelChanged {
                model,
                effort,
                reason,
            } => SessionView {
                model,
                effort,
                ..self
            }
            .push(Item::ModelChanged(reason)),
            EventKind::SessionClosed { reason } => self
                .change(Change::End)
                .settle()
                .status(SessionStatus::Closed(reason)),
            EventKind::UserMessage { text, by } => {
                let mut turn_starts = self.turn_starts;
                turn_starts.insert(seq, self.items.len());
                SessionView {
                    turn_starts,
                    undo: UndoState::Ready,
                    ..self
                }
                .status(SessionStatus::Running)
                .push(Item::User { text, by })
            }
            EventKind::AssistantDelta { text } => self.reply(text, ReplyState::Streaming),
            EventKind::AssistantMessage { text } => self.reply(text, ReplyState::Done),
            EventKind::ReasoningDelta { text } => self.reasoning(text, ReplyState::Streaming),
            EventKind::Reasoning { text } => self.reasoning(text, ReplyState::Done),
            EventKind::ToolCall {
                call,
                phase: ToolPhase::Started { tool },
            } => self.push(Item::Tool {
                call,
                tool,
                state: ToolState::Running,
                approval: Approval::NotAsked,
            }),
            EventKind::ToolCall {
                call,
                phase: ToolPhase::Finished { outcome },
            } => self.change(Change::Finish(call, outcome)),
            EventKind::RequestOpened {
                request,
                call,
                tool,
                reason,
                for_session,
            } => self.ask(
                call,
                tool,
                Approval::Asked {
                    request,
                    reason,
                    for_session,
                    state: RequestState::Open,
                },
            ),
            EventKind::QuestionAsked { request, questions } => self.push(Item::Question {
                request,
                questions,
                state: RequestState::Open,
            }),
            EventKind::RequestResolved {
                request,
                resolution,
            } => self.change(Change::Resolve(request, resolution)),
            EventKind::TurnCompleted { outcome } => self
                .change(Change::End)
                .status(SessionStatus::Idle)
                .push(Item::TurnCompleted(outcome)),
            EventKind::WorkspaceCreated { workspace } => SessionView {
                workspace: Some(workspace),
                ..self
            },
            EventKind::CheckpointCreated { checkpoint } => {
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
                match start {
                    Some(start) => {
                        let mut checkpoints = self.checkpoints;
                        checkpoints.push(TurnCheckpoint {
                            checkpoint: checkpoint.id,
                            undo: checkpoint.undo,
                            start,
                            end: self.items.len().saturating_sub(1),
                        });
                        SessionView {
                            checkpoints,
                            ..self
                        }
                    }
                    None => self,
                }
            }
            EventKind::UndoHistoryRestored { .. }
            | EventKind::CheckpointReserved { .. }
            | EventKind::HistoryMaterialized { .. }
            | EventKind::NativeTurnStarted { .. } => self,
            EventKind::UndoStarted { checkpoint, .. } => SessionView {
                undo: UndoState::Undoing(checkpoint),
                ..self
            },
            EventKind::UndoFailed { checkpoint, reason } => SessionView {
                undo: UndoState::Failed { checkpoint, reason },
                ..self
            },
            EventKind::TurnsUndone { checkpoint, .. } => {
                match self
                    .checkpoints
                    .iter()
                    .find(|turn| turn.checkpoint == checkpoint)
                    .map(|turn| turn.start)
                {
                    Some(start) => SessionView {
                        items: self.items.into_iter().take(start).collect(),
                        checkpoints: self
                            .checkpoints
                            .into_iter()
                            .filter(|turn| turn.start < start)
                            .collect(),
                        turn_starts: self
                            .turn_starts
                            .into_iter()
                            .filter(|(_, index)| index < &start)
                            .collect(),
                        undo: UndoState::Undone,
                        status: SessionStatus::Idle,
                        ..self
                    },
                    None => SessionView {
                        undo: UndoState::Undone,
                        ..self
                    },
                }
            }
            EventKind::Error { message } => self.push(Item::Error(message)),
        }
    }

    fn status(self, status: SessionStatus) -> SessionView {
        SessionView { status, ..self }
    }

    /// A delta joins the reply that is still streaming; the final text
    /// replaces it, so a client that missed the deltas shows the same reply.
    fn reply(self, text: String, state: ReplyState) -> SessionView {
        let mut items = self.items;
        let open = match items.pop() {
            Some(Item::Reply {
                text,
                state: ReplyState::Streaming,
            }) => Some(text),
            last => {
                items.extend(last.map(Item::settled));
                None
            }
        };
        let text = match state {
            ReplyState::Streaming => format!("{}{text}", open.unwrap_or_default()),
            ReplyState::Done => text,
        };
        items.push(Item::Reply { text, state });
        SessionView { items, ..self }
    }

    /// Reasoning folds as a reply does, into an item of its own.
    fn reasoning(self, text: String, state: ReplyState) -> SessionView {
        let mut items = self.items;
        let open = match items.pop() {
            Some(Item::Reasoning {
                text,
                state: ReplyState::Streaming,
            }) => Some(text),
            last => {
                items.extend(last.map(Item::settled));
                None
            }
        };
        let text = match state {
            ReplyState::Streaming => format!("{}{text}", open.unwrap_or_default()),
            ReplyState::Done => text,
        };
        items.push(Item::Reasoning { text, state });
        SessionView { items, ..self }
    }

    /// Closes a reply or reasoning that was still streaming.
    fn settle(self) -> SessionView {
        let mut items = self.items;
        let last = items.pop().map(Item::settled);
        items.extend(last);
        SessionView { items, ..self }
    }

    fn push(self, item: Item) -> SessionView {
        let view = self.settle();
        let mut items = view.items;
        items.push(item);
        SessionView { items, ..view }
    }

    /// A request joins its tool call's card. A request for a call this view
    /// never saw started gets a card of its own.
    fn ask(self, call: ToolCallId, tool: ToolKind, approval: Approval) -> SessionView {
        match self
            .items
            .iter()
            .filter_map(Item::call)
            .find(|seen| seen.0 == call.0)
        {
            Some(_) => self.change(Change::Ask(call, approval)),
            None => self.push(Item::Tool {
                call,
                tool,
                state: ToolState::Running,
                approval,
            }),
        }
    }

    fn change(self, change: Change) -> SessionView {
        SessionView {
            items: self
                .items
                .into_iter()
                .map(|item| item.changed(&change))
                .collect(),
            ..self
        }
    }
}

impl Item {
    /// The item, done streaming: something else follows it.
    fn settled(self) -> Item {
        match self {
            Item::Reply { text, .. } => Item::Reply {
                text,
                state: ReplyState::Done,
            },
            Item::Reasoning { text, .. } => Item::Reasoning {
                text,
                state: ReplyState::Done,
            },
            other => other,
        }
    }

    fn call(&self) -> Option<&ToolCallId> {
        match self {
            Item::Tool { call, .. } => Some(call),
            Item::User { .. }
            | Item::Reply { .. }
            | Item::Reasoning { .. }
            | Item::Question { .. }
            | Item::TurnCompleted(_)
            | Item::Continued(_)
            | Item::ModelChanged(_)
            | Item::Error(_) => None,
        }
    }

    fn changed(self, change: &Change) -> Item {
        match self {
            Item::Tool {
                call,
                tool,
                state,
                approval,
            } => Item::Tool {
                state: state.changed(&call, change),
                approval: approval.changed(&call, change),
                call,
                tool,
            },
            Item::Question {
                request,
                questions,
                state,
            } => Item::Question {
                state: state.changed(&request, change),
                request,
                questions,
            },
            item => item,
        }
    }
}

impl ToolState {
    fn changed(self, call: &ToolCallId, change: &Change) -> ToolState {
        match (self, change) {
            (_, Change::Finish(id, outcome)) if call == id => {
                ToolState::Finished(outcome.to_owned())
            }
            (ToolState::Running, Change::End) => ToolState::Stopped,
            (
                state,
                Change::Ask(_, _) | Change::Resolve(_, _) | Change::Finish(_, _) | Change::End,
            ) => state,
        }
    }
}

impl Approval {
    fn changed(self, call: &ToolCallId, change: &Change) -> Approval {
        match (self, change) {
            (_, Change::Ask(id, approval)) if call == id => approval.clone(),
            (approval, change) => match approval {
                Approval::Asked {
                    request,
                    reason,
                    for_session,
                    state,
                } => Approval::Asked {
                    state: state.changed(&request, change),
                    request,
                    reason,
                    for_session,
                },
                Approval::NotAsked => Approval::NotAsked,
            },
        }
    }
}

impl RequestState {
    /// A request's answer, or its withdrawal when the turn or session ends
    /// with it still open.
    fn changed(self, request: &RequestId, change: &Change) -> RequestState {
        match (self, change) {
            (_, Change::Resolve(id, resolution)) if request == id => {
                RequestState::Resolved(resolution.clone())
            }
            (RequestState::Open, Change::End) => RequestState::Resolved(Resolution::Withdrawn),
            (
                state,
                Change::Ask(_, _) | Change::Resolve(_, _) | Change::Finish(_, _) | Change::End,
            ) => state,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proto::{
        Answer, ClientName, CloseReason, Decision, Event, EventKind, HostPath, HostSeq, ModelId,
        Provider, Question, QuestionOption, RequestId, Resolution, Selection, SessionId,
        ToolCallId, ToolKind, ToolOutcome, ToolPhase, TurnOutcome,
    };

    use super::{
        Approval, Attention, Composer, HostView, Interrupt, Item, ReplyState, RequestState, Resume,
        SessionStatus, SessionView, ToolState, UndoState, Working,
    };

    fn text(text: &str) -> String {
        String::from(text)
    }

    fn created(directory: &str) -> EventKind {
        EventKind::SessionCreated {
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

    #[test]
    fn events_fold_into_each_sessions_transcript_in_order() {
        let call = ToolCallId(text("toolu_1"));
        let events = Vec::from([
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
        ]);
        let view = fold(events);
        assert_eq!(
            view.sessions,
            BTreeMap::from([
                (
                    SessionId(1),
                    SessionView {
                        provider: Provider::Claude,
                        directory: HostPath(text("/a")),
                        model: None,
                        effort: None,
                        status: SessionStatus::Idle,
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
                                    state: RequestState::Resolved(Resolution::Decided {
                                        decision: Decision::Approve,
                                        by: ClientName(text("phone")),
                                    }),
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
                                    state: RequestState::Resolved(Resolution::Withdrawn),
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
                        provider: Provider::Claude,
                        directory: HostPath(text("/b")),
                        model: None,
                        effort: None,
                        status: SessionStatus::Closed(CloseReason::Exited { status: text("1") }),
                        items: Vec::from([Item::Error(text("odd"))]),
                        workspace: None,
                        checkpoints: Vec::new(),
                        undo: UndoState::Ready,
                        turn_starts: BTreeMap::new(),
                        activity: HostSeq(15),
                    },
                ),
            ])
        );
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

    fn asked(call: &str, request: u64, state: ToolState, approval: RequestState) -> Item {
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
    fn the_end_of_a_turn_or_session_stops_its_tools_and_withdraws_its_requests() {
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
    fn the_list_puts_the_latest_activity_first_and_suggests_each_directory_once() {
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
    fn reasoning_streams_into_one_item_that_its_text_replaces_and_a_close_is_kept() {
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
    fn a_running_turn_shows_it_is_working_until_something_live_ends_the_transcript() {
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
        let cases = Vec::from([
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
                events: Vec::from([user.clone(), EventKind::ReasoningDelta { text: text("Hm") }]),
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
        ]);
        let (got, want) = cases
            .into_iter()
            .map(|case| {
                let view = fold(
                    Vec::from([created("/a"), EventKind::SessionReady])
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
    fn rv_no_thinking_under_an_open_approval_after_a_parallel_tool_finished() {
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
        let session = view.sessions.values().next().unwrap();
        assert_eq!(session.attention(), Attention::Requests(1));
        assert_eq!(session.working(), Working::Hidden);
    }

    #[test]
    fn rv_no_thinking_under_an_open_approval_after_a_commentary_reply() {
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
        let session = view.sessions.values().next().unwrap();
        assert_eq!(session.attention(), Attention::Requests(1));
        assert_eq!(session.working(), Working::Hidden);
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

    fn questions(view: &HostView, session: u64) -> Vec<(RequestId, RequestState)> {
        view.sessions
            .get(&SessionId(u128::from(session)))
            .map(|session| {
                session
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
                        | Item::Error(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
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
                    (RequestId(2), RequestState::Resolved(Resolution::Withdrawn)),
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

    fn numbered(kinds: Vec<EventKind>) -> Vec<Event> {
        kinds
            .into_iter()
            .enumerate()
            .map(|(index, kind)| Event {
                seq: HostSeq(u64::try_from(index).unwrap().saturating_add(1)),
                session: SessionId(1),
                kind,
            })
            .collect()
    }

    #[test]
    fn undo_removes_selected_and_later_turns_on_live_fold_replay_and_reconnect() {
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
        let live = events
            .clone()
            .into_iter()
            .fold(HostView::empty(), HostView::apply);
        let reconnect = events
            .iter()
            .take(15)
            .cloned()
            .fold(HostView::empty(), HostView::apply);
        let reconnect = events.into_iter().skip(15).fold(reconnect, HostView::apply);
        assert_eq!(live, reconnect);
        let session = live.sessions.get(&SessionId(1)).unwrap();
        assert_eq!(session.items.len(), 3);
        assert_eq!(session.checkpoints.len(), 1);
        assert_eq!(
            session.checkpoints.first().unwrap().checkpoint,
            proto::CheckpointId(1)
        );
        assert_eq!(session.undo, super::UndoState::Undone);
        assert_eq!(session.controls().composer, Composer::Open);
    }

    #[test]
    fn in_progress_and_failed_undo_preserve_transcript_and_block_controls() {
        let original = numbered(checkpoint_events())
            .into_iter()
            .fold(HostView::empty(), HostView::apply);
        let pending = original.clone().apply(Event {
            seq: HostSeq(15),
            session: SessionId(1),
            kind: EventKind::UndoStarted {
                checkpoint: proto::CheckpointId(2),
                restore: proto::CheckpointId(1),
                history: proto::HistoryAnchor::Fresh,
            },
        });
        let session = pending.sessions.get(&SessionId(1)).unwrap();
        assert_eq!(session.items.len(), 9);
        assert_eq!(
            session.controls(),
            super::Controls {
                composer: Composer::Busy,
                interrupt: Interrupt::Hidden,
                resume: Resume::Hidden
            }
        );
        let failed = pending.apply(Event {
            seq: HostSeq(16),
            session: SessionId(1),
            kind: EventKind::UndoFailed {
                checkpoint: proto::CheckpointId(2),
                reason: text("restore failed"),
            },
        });
        let session = failed.sessions.get(&SessionId(1)).unwrap();
        assert_eq!(
            session.items,
            original.sessions.get(&SessionId(1)).unwrap().items
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
    }

    #[test]
    fn legacy_session_stays_usable_without_workspace_or_checkpoint_controls() {
        let view = fold(Vec::from([
            (1, created("/legacy")),
            (1, EventKind::SessionReady),
        ]));
        let session = view.sessions.get(&SessionId(1)).unwrap();
        assert_eq!(session.workspace, None);
        assert_eq!(session.checkpoints, Vec::new());
        assert_eq!(session.controls().composer, Composer::Open);
    }

    #[test]
    fn autonomous_checkpoint_has_a_turn_boundary_but_initial_checkpoint_does_not() {
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
        let view = events.into_iter().fold(HostView::empty(), HostView::apply);
        let session = view.sessions.get(&SessionId(1)).unwrap();
        assert_eq!(session.checkpoints.len(), 1);
        assert_eq!(session.checkpoints.first().unwrap().start, 0);
        assert_eq!(session.checkpoints.first().unwrap().end, 1);
    }
}
