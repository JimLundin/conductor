//! A host's transcript folded from its events: what the UI renders. No UI
//! types, so it is unit-testable and shared by every client target.

use proto::{Decision, EventKind, RequestId};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transcript {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// Streamed text; `done` once something else follows it.
    Reply {
        text: String,
        done: bool,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub decision: Decision,
    pub by: String,
}

impl Transcript {
    pub fn apply(&mut self, kind: &EventKind) {
        if let EventKind::TranscriptDelta { text } = kind {
            if let Some(Item::Reply { text: open, done }) = self.items.last_mut()
                && !*done
            {
                open.push_str(text);
            } else {
                self.items.push(Item::Reply {
                    text: text.clone(),
                    done: false,
                });
            }
            return;
        }
        if let Some(Item::Reply { done, .. }) = self.items.last_mut() {
            *done = true;
        }
        match kind {
            EventKind::TranscriptDelta { .. } => unreachable!("handled above"),
            EventKind::ApprovalRequested { id, tool, detail } => self.items.push(Item::Approval {
                id: *id,
                tool: tool.clone(),
                detail: detail.clone(),
                outcome: None,
            }),
            EventKind::ApprovalResolved { id, decision, by } => {
                let target = self.items.iter_mut().find_map(|item| match item {
                    Item::Approval {
                        id: open, outcome, ..
                    } if open == id => Some(outcome),
                    _ => None,
                });
                if let Some(outcome) = target {
                    *outcome = Some(Outcome {
                        decision: *decision,
                        by: by.clone(),
                    });
                }
            }
            EventKind::AssistantMessage { text } => self.items.push(Item::Message(text.clone())),
            EventKind::TurnFinished => self.items.push(Item::TurnFinished),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deltas_join_into_one_reply_until_the_approval() {
        let mut t = Transcript::default();
        let events = [
            EventKind::TranscriptDelta { text: "Hel".into() },
            EventKind::TranscriptDelta { text: "lo".into() },
            EventKind::ApprovalRequested {
                id: RequestId(1),
                tool: "Bash".into(),
                detail: "ls".into(),
            },
            EventKind::ApprovalResolved {
                id: RequestId(1),
                decision: Decision::Deny,
                by: "phone".into(),
            },
            EventKind::AssistantMessage { text: "ok".into() },
            EventKind::TurnFinished,
            EventKind::TranscriptDelta {
                text: "Next".into(),
            },
        ];
        for event in &events {
            t.apply(event);
        }
        assert_eq!(
            t.items,
            [
                Item::Reply {
                    text: "Hello".into(),
                    done: true
                },
                Item::Approval {
                    id: RequestId(1),
                    tool: "Bash".into(),
                    detail: "ls".into(),
                    outcome: Some(Outcome {
                        decision: Decision::Deny,
                        by: "phone".into()
                    })
                },
                Item::Message("ok".into()),
                Item::TurnFinished,
                Item::Reply {
                    text: "Next".into(),
                    done: false
                },
            ]
        );
    }
}
