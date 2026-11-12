//! A host's sessions folded from its events: what the UI renders. No UI
//! types, so it is unit-testable and shared by every client target.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use proto::{
    ClientName, CloseReason, Event, EventKind, HostPath, HostSeq, Provider, RequestId, Resolution,
    SessionId, ToolCallId, ToolKind, ToolOutcome, ToolPhase, TurnOutcome,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostView {
    pub sessions: BTreeMap<SessionId, SessionView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    pub provider: Provider,
    pub directory: HostPath,
    pub status: SessionStatus,
    pub items: Vec<Item>,
    /// The last event that touched this session.
    pub activity: HostSeq,
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
    /// A tool call, with the approval it asked for, if any.
    Tool {
        call: ToolCallId,
        tool: ToolKind,
        state: ToolState,
        approval: Approval,
    },
    TurnCompleted(TurnOutcome),
    Error(String),
}

/// A reply streams until its final text arrives or something else follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyState {
    Streaming,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Finished(ToolOutcome),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Approval {
    NotAsked,
    Asked {
        request: RequestId,
        state: RequestState,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestState {
    Open,
    Resolved(Resolution),
}

/// An in-place change to an earlier tool call. Asking, resolving and
/// finishing are the same shape, so they are one `map` over the items.
enum Change {
    Ask(ToolCallId, RequestId),
    Resolve(RequestId, Resolution),
    Finish(ToolCallId, ToolOutcome),
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
                    ..
                },
            ) => Some(SessionView {
                provider,
                directory,
                status: SessionStatus::Starting,
                items: Vec::new(),
                activity: event.seq,
            }),
            (Some(view), kind) => Some(view.apply(kind)),
            (None, _) => None,
        };
        sessions.extend(session.map(|view| {
            (
                event.session,
                SessionView {
                    activity: event.seq,
                    ..view
                },
            )
        }));
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
        let (composer, interrupt) = match self.status {
            SessionStatus::Starting => (Composer::Busy, Interrupt::Hidden),
            SessionStatus::Idle => (Composer::Open, Interrupt::Hidden),
            SessionStatus::Running => (Composer::Busy, Interrupt::Offered),
            SessionStatus::Closed(_) => (Composer::Closed, Interrupt::Hidden),
        };
        Controls {
            composer,
            interrupt,
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
                    }
                )
            })
            .count();
        match open {
            0 => Attention::Quiet,
            open => Attention::Requests(open),
        }
    }

    fn apply(self, kind: EventKind) -> SessionView {
        match kind {
            EventKind::SessionCreated { .. } => self,
            EventKind::SessionReady => self.status(SessionStatus::Idle),
            EventKind::SessionClosed { reason } => self.status(SessionStatus::Closed(reason)),
            EventKind::UserMessage { text, by } => self
                .status(SessionStatus::Running)
                .push(Item::User { text, by }),
            EventKind::AssistantDelta { text } => self.reply(text, ReplyState::Streaming),
            EventKind::AssistantMessage { text } => self.reply(text, ReplyState::Done),
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
            } => self.ask(call, request, tool),
            EventKind::RequestResolved {
                request,
                resolution,
            } => self.change(Change::Resolve(request, resolution)),
            EventKind::TurnCompleted { outcome } => self
                .status(SessionStatus::Idle)
                .push(Item::TurnCompleted(outcome)),
            EventKind::Error { message } => self.push(Item::Error(message)),
        }
    }

    fn status(self, status: SessionStatus) -> SessionView {
        SessionView { status, ..self }
    }

    /// A delta joins the reply that is still streaming; the final text
    /// replaces it, so replayed deltas never show twice.
    fn reply(self, text: String, state: ReplyState) -> SessionView {
        let mut items = self.items;
        let open = match items.pop() {
            Some(Item::Reply {
                text,
                state: ReplyState::Streaming,
            }) => Some(text),
            last => {
                items.extend(last);
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

    /// Adds an item after closing a reply that was still streaming.
    fn push(self, item: Item) -> SessionView {
        let mut items = self.items;
        let last = items.pop().map(|last| match last {
            Item::Reply { text, .. } => Item::Reply {
                text,
                state: ReplyState::Done,
            },
            other => other,
        });
        items.extend(last);
        items.push(item);
        SessionView { items, ..self }
    }

    /// A request joins its tool call's card. A request for a call this view
    /// never saw started gets a card of its own.
    fn ask(self, call: ToolCallId, request: RequestId, tool: ToolKind) -> SessionView {
        match self
            .items
            .iter()
            .filter_map(Item::call)
            .find(|seen| seen.0 == call.0)
        {
            Some(_) => self.change(Change::Ask(call, request)),
            None => self.push(Item::Tool {
                call,
                tool,
                state: ToolState::Running,
                approval: Approval::Asked {
                    request,
                    state: RequestState::Open,
                },
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
    fn call(&self) -> Option<&ToolCallId> {
        match self {
            Item::Tool { call, .. } => Some(call),
            Item::User { .. } | Item::Reply { .. } | Item::TurnCompleted(_) | Item::Error(_) => {
                None
            }
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
            item => item,
        }
    }
}

impl ToolState {
    fn changed(self, call: &ToolCallId, change: &Change) -> ToolState {
        match change {
            Change::Finish(id, outcome) => match call.eq(id).then_some(outcome).copied() {
                Some(outcome) => ToolState::Finished(outcome),
                None => self,
            },
            Change::Ask(_, _) | Change::Resolve(_, _) => self,
        }
    }
}

impl Approval {
    fn changed(self, call: &ToolCallId, change: &Change) -> Approval {
        match (self, change) {
            (approval, Change::Ask(id, request)) => match call.eq(id).then_some(request).copied() {
                Some(request) => Approval::Asked {
                    request,
                    state: RequestState::Open,
                },
                None => approval,
            },
            (Approval::Asked { request, state }, Change::Resolve(id, resolution)) => {
                Approval::Asked {
                    state: match request.eq(id).then_some(resolution) {
                        Some(resolution) => RequestState::Resolved(resolution.clone()),
                        None => state,
                    },
                    request,
                }
            }
            (approval, Change::Resolve(_, _) | Change::Finish(_, _)) => approval,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proto::{
        ClientName, CloseReason, Decision, Event, EventKind, HostPath, HostSeq, Provider,
        RequestId, Resolution, SessionId, ToolCallId, ToolKind, ToolOutcome, ToolPhase,
        TurnOutcome,
    };

    use super::{
        Approval, Attention, HostView, Item, ReplyState, RequestState, SessionStatus, SessionView,
        ToolState,
    };

    fn text(text: &str) -> String {
        String::from(text)
    }

    fn created(directory: &str) -> EventKind {
        EventKind::SessionCreated {
            provider: Provider::Claude,
            directory: HostPath(text(directory)),
            by: ClientName(text("desk")),
        }
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
                                state: ToolState::Running,
                                approval: Approval::Asked {
                                    request: RequestId(2),
                                    state: RequestState::Resolved(Resolution::Withdrawn),
                                },
                            },
                            Item::Reply {
                                text: text("Done"),
                                state: ReplyState::Done,
                            },
                            Item::TurnCompleted(TurnOutcome::Completed),
                        ]),
                        activity: HostSeq(15),
                    },
                ),
                (
                    SessionId(2),
                    SessionView {
                        provider: Provider::Claude,
                        directory: HostPath(text("/b")),
                        status: SessionStatus::Closed(CloseReason::Exited { status: text("1") }),
                        items: Vec::from([Item::Error(text("odd"))]),
                        activity: HostSeq(17),
                    },
                ),
            ])
        );
    }

    fn fold(events: Vec<(u64, EventKind)>) -> HostView {
        events
            .into_iter()
            .enumerate()
            .map(|(index, (session, kind))| Event {
                seq: HostSeq(u64::try_from(index).unwrap()),
                session: SessionId(session),
                kind,
            })
            .fold(HostView::empty(), HostView::apply)
    }

    fn opened(request: u64) -> EventKind {
        EventKind::RequestOpened {
            request: RequestId(request),
            call: ToolCallId(format!("toolu_{request}")),
            tool: ToolKind::Command {
                command: text("ls"),
            },
        }
    }

    #[test]
    fn the_list_puts_the_latest_activity_first_and_suggests_each_directory_once() {
        let view = fold(Vec::from([
            (1, created("/a")),
            (2, created("/b")),
            (3, created("/a")),
            (1, EventKind::SessionReady),
            (2, delta("x")),
        ]));
        assert_eq!(
            view.recent(),
            Vec::from([SessionId(2), SessionId(1), SessionId(3)])
        );
        assert_eq!(
            view.directories(),
            Vec::from([HostPath(text("/b")), HostPath(text("/a"))])
        );
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
}
