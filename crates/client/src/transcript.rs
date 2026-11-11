//! A host's transcript folded from its events: what the UI renders. No UI
//! types, so it is unit-testable and shared by every client target.

use std::cmp::Ordering;

use proto::{Decision, EventKind, RequestId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Reply {
        text: String,
        state: ReplyState,
    },
    Approval {
        id: RequestId,
        tool: String,
        detail: String,
        outcome: Option<Outcome>,
    },
    Message(String),
    TurnFinished,
}

/// A reply streams until something else follows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyState {
    Streaming,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub decision: Decision,
    pub by: String,
}

impl Transcript {
    pub fn empty() -> Transcript {
        Transcript { items: Vec::new() }
    }

    pub fn apply(self, kind: EventKind) -> Transcript {
        match kind {
            EventKind::TranscriptDelta { text } => self.append(text),
            EventKind::ApprovalRequested { id, tool, detail } => {
                self.close_reply().push(Item::Approval {
                    id,
                    tool,
                    detail,
                    outcome: None,
                })
            }
            EventKind::ApprovalResolved { id, decision, by } => {
                self.close_reply().resolve(id, Outcome { decision, by })
            }
            EventKind::AssistantMessage { text } => self.close_reply().push(Item::Message(text)),
            EventKind::TurnFinished => self.close_reply().push(Item::TurnFinished),
        }
    }

    fn append(self, text: String) -> Transcript {
        let mut items = self.items;
        match items.pop() {
            Some(Item::Reply {
                text: open,
                state: ReplyState::Streaming,
            }) => items.push(Item::Reply {
                text: format!("{open}{text}"),
                state: ReplyState::Streaming,
            }),
            Some(last) => {
                items.push(last);
                items.push(Item::Reply {
                    text,
                    state: ReplyState::Streaming,
                });
            }
            None => items.push(Item::Reply {
                text,
                state: ReplyState::Streaming,
            }),
        }
        Transcript { items }
    }

    fn close_reply(self) -> Transcript {
        let mut items = self.items;
        match items.pop() {
            Some(Item::Reply { text, state: _ }) => items.push(Item::Reply {
                text,
                state: ReplyState::Done,
            }),
            Some(last) => items.push(last),
            None => {}
        }
        Transcript { items }
    }

    fn push(self, item: Item) -> Transcript {
        let mut items = self.items;
        items.push(item);
        Transcript { items }
    }

    fn resolve(self, id: RequestId, outcome: Outcome) -> Transcript {
        Transcript {
            items: self
                .items
                .into_iter()
                .map(|item| item.resolved(id, &outcome))
                .collect(),
        }
    }
}

impl Item {
    fn resolved(self, id: RequestId, outcome: &Outcome) -> Item {
        match self {
            Item::Approval {
                id: open,
                tool,
                detail,
                outcome: current,
            } => Item::Approval {
                id: open,
                tool,
                detail,
                outcome: match open.0.cmp(&id.0) {
                    Ordering::Equal => Some(outcome.clone()),
                    Ordering::Less | Ordering::Greater => current,
                },
            },
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use proto::{Decision, EventKind, RequestId};

    use super::{Item, Outcome, ReplyState, Transcript};

    #[test]
    fn deltas_join_into_one_reply_until_the_approval() {
        let events = Vec::from([
            EventKind::TranscriptDelta {
                text: String::from("Hel"),
            },
            EventKind::TranscriptDelta {
                text: String::from("lo"),
            },
            EventKind::ApprovalRequested {
                id: RequestId(1),
                tool: String::from("Bash"),
                detail: String::from("ls"),
            },
            EventKind::ApprovalResolved {
                id: RequestId(1),
                decision: Decision::Deny,
                by: String::from("phone"),
            },
            EventKind::AssistantMessage {
                text: String::from("ok"),
            },
            EventKind::TurnFinished,
            EventKind::TranscriptDelta {
                text: String::from("Next"),
            },
        ]);
        let transcript = events
            .into_iter()
            .fold(Transcript::empty(), Transcript::apply);
        assert_eq!(
            transcript.items,
            [
                Item::Reply {
                    text: String::from("Hello"),
                    state: ReplyState::Done,
                },
                Item::Approval {
                    id: RequestId(1),
                    tool: String::from("Bash"),
                    detail: String::from("ls"),
                    outcome: Some(Outcome {
                        decision: Decision::Deny,
                        by: String::from("phone"),
                    }),
                },
                Item::Message(String::from("ok")),
                Item::TurnFinished,
                Item::Reply {
                    text: String::from("Next"),
                    state: ReplyState::Streaming,
                },
            ]
        );
    }
}
