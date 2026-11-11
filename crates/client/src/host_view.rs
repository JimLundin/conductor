//! A host's sessions folded from its events: what the UI renders. No UI
//! types, so it is unit-testable and shared by every client target.

use std::collections::BTreeMap;

use proto::{
    ClientName, CloseReason, Event, EventKind, HostPath, Provider, RequestId, Resolution,
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
    Tool {
        call: ToolCallId,
        tool: ToolKind,
        state: ToolState,
    },
    Request {
        request: RequestId,
        tool: String,
        detail: String,
        state: RequestState,
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
pub enum RequestState {
    Open,
    Resolved(Resolution),
}

/// An in-place change to an earlier item. Resolving a request and finishing
/// a tool call are the same shape, so they are one `map` over the items.
enum Change {
    Resolve(RequestId, Resolution),
    Finish(ToolCallId, ToolOutcome),
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
            }),
            (Some(view), kind) => Some(view.apply(kind)),
            (None, _) => None,
        };
        sessions.extend(session.map(|view| (event.session, view)));
        HostView { sessions }
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
            }),
            EventKind::ToolCall {
                call,
                phase: ToolPhase::Finished { outcome },
            } => self.change(Change::Finish(call, outcome)),
            EventKind::RequestOpened {
                request,
                tool,
                detail,
            } => self.push(Item::Request {
                request,
                tool,
                detail,
                state: RequestState::Open,
            }),
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
    fn changed(self, change: &Change) -> Item {
        match (self, change) {
            (
                Item::Request {
                    request,
                    tool,
                    detail,
                    state,
                },
                Change::Resolve(id, resolution),
            ) => Item::Request {
                state: match request.eq(id).then_some(resolution) {
                    Some(resolution) => RequestState::Resolved(resolution.clone()),
                    None => state,
                },
                request,
                tool,
                detail,
            },
            (Item::Tool { call, tool, state }, Change::Finish(id, outcome)) => Item::Tool {
                state: match call.eq(id).then_some(outcome).copied() {
                    Some(outcome) => ToolState::Finished(outcome),
                    None => state,
                },
                call,
                tool,
            },
            (item, _) => item,
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

    use super::{HostView, Item, ReplyState, RequestState, SessionStatus, SessionView, ToolState};

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
                    tool: text("Bash"),
                    detail: text("ls"),
                },
            ),
            (
                1,
                EventKind::RequestOpened {
                    request: RequestId(2),
                    tool: text("Bash"),
                    detail: text("rm"),
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
        let view = events
            .into_iter()
            .enumerate()
            .map(|(index, (session, kind))| Event {
                seq: HostSeq(u64::try_from(index).unwrap()),
                session: SessionId(session),
                kind,
            })
            .fold(HostView::empty(), HostView::apply);
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
                            },
                            Item::Request {
                                request: RequestId(1),
                                tool: text("Bash"),
                                detail: text("ls"),
                                state: RequestState::Resolved(Resolution::Decided {
                                    decision: Decision::Approve,
                                    by: ClientName(text("phone")),
                                }),
                            },
                            Item::Request {
                                request: RequestId(2),
                                tool: text("Bash"),
                                detail: text("rm"),
                                state: RequestState::Resolved(Resolution::Withdrawn),
                            },
                            Item::Reply {
                                text: text("Done"),
                                state: ReplyState::Done,
                            },
                            Item::TurnCompleted(TurnOutcome::Completed),
                        ]),
                    },
                ),
                (
                    SessionId(2),
                    SessionView {
                        provider: Provider::Claude,
                        directory: HostPath(text("/b")),
                        status: SessionStatus::Closed(CloseReason::Exited { status: text("1") }),
                        items: Vec::from([Item::Error(text("odd"))]),
                    },
                ),
            ])
        );
    }
}
