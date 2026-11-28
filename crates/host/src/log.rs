use alloc::collections::BTreeSet;
use alloc::format;
use core::mem;

use agents::Output;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::session::{EventKind, SessionId};

use crate::prelude::*;

pub const FORMAT: u32 = 11;

pub fn opens(created: bool, stored: Option<u32>) -> Result<bool, String> {
    match (created, stored) {
        (true, _) => Ok(true),
        (false, Some(FORMAT)) => Ok(false),
        (false, other) => Err(format!(
            "the log is format {}, and this host reads format {FORMAT}; move the database aside to start over",
            other.unwrap_or(1)
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    Event(Event),
    Conversation(SessionId, String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreshRequest {
    pub session: SessionId,
    pub seq: HostSeq,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Open,
    Store(Write),
    Commit,
    Publish(Event),
    RequestOpened(FreshRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Log {
    events: Vec<Event>,
    staged: Vec<Event>,
    open: bool,
}

#[must_use]
pub const fn live_only(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Agent(
            Output::AssistantDelta { .. } | Output::ReasoningDelta { .. }
        )
    )
}

impl Log {
    #[must_use]
    pub const fn new(events: Vec<Event>) -> Self {
        Self {
            events,
            staged: Vec::new(),
            open: false,
        }
    }

    #[must_use]
    pub fn history(&self) -> &[Event] {
        &self.events
    }

    #[must_use]
    pub fn running_sessions(&self) -> BTreeSet<SessionId> {
        let mut running = crate::registry::running(&self.events);
        running.extend(crate::registry::running(&self.staged));
        running
    }

    #[must_use]
    pub fn current_history(&self, session: SessionId) -> Vec<Event> {
        let mut history = crate::restore::events_of(session, &self.events);
        history.extend(
            self.staged
                .iter()
                .filter(|event| {
                    event.session == session && !live_only(&event.kind)
                })
                .cloned(),
        );
        history
    }

    #[must_use]
    pub fn replay(&self, after: HostSeq) -> Vec<Event> {
        self.events
            .iter()
            .filter(|event| event.seq > after)
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn head(&self) -> HostSeq {
        self.events
            .last()
            .map_or(HostSeq::BEFORE_FIRST, |event| event.seq)
    }

    pub fn append(
        &mut self,
        session: SessionId,
        kind: EventKind,
    ) -> (Event, Vec<Effect>) {
        let last = self
            .staged
            .last()
            .map_or_else(|| self.head(), |event| event.seq);
        let live = live_only(&kind);
        let seq = if live { last } else { last.next() };
        let event = Event { seq, session, kind };
        let effects = match (self.open, live) {
            (true, true) => {
                self.staged.push(event.clone());
                Vec::new()
            }
            (true, false) => {
                self.staged.push(event.clone());
                Vec::from([Effect::Store(Write::Event(event.clone()))])
            }
            (false, _) => {
                let mut effects = if live {
                    Vec::new()
                } else {
                    alone(Write::Event(event.clone()))
                };
                self.publish_fresh(event.clone(), &mut effects);
                effects
            }
        };
        (event, effects)
    }

    #[must_use]
    pub fn conversation(&self, session: SessionId, id: String) -> Vec<Effect> {
        let write = Write::Conversation(session, id);
        if self.open {
            Vec::from([Effect::Store(write)])
        } else {
            alone(write)
        }
    }

    pub const fn open(&mut self) -> Effect {
        self.open = true;
        Effect::Open
    }

    pub fn barrier(&mut self) -> Vec<Effect> {
        let mut effects = self.commit();
        if self.open {
            effects.push(Effect::Open);
        }
        effects
    }

    pub fn commit(&mut self) -> Vec<Effect> {
        let mut effects = Vec::from([Effect::Commit]);
        for event in mem::take(&mut self.staged) {
            self.publish_fresh(event, &mut effects);
        }
        effects
    }

    pub fn close(&mut self) -> Vec<Effect> {
        let effects = self.commit();
        self.open = false;
        effects
    }

    fn publish_fresh(&mut self, event: Event, effects: &mut Vec<Effect>) {
        let request = event.kind.opened_request().map(|_| FreshRequest {
            session: event.session,
            seq: event.seq,
        });
        effects.push(self.publish(event));
        effects.extend(request.map(Effect::RequestOpened));
    }

    fn publish(&mut self, event: Event) -> Effect {
        if !live_only(&event.kind) {
            self.events.push(event.clone());
        }
        Effect::Publish(event)
    }
}

fn alone(write: Write) -> Vec<Effect> {
    Vec::from([Effect::Open, Effect::Store(write), Effect::Commit])
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
pub struct HostSeq(pub u64);

impl HostSeq {
    pub const BEFORE_FIRST: Self = Self(0);

    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct Epoch(
    #[serde(with = "crate::json::decimal")]
    #[schemars(with = "String")]
    pub u64,
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    pub seq: HostSeq,
    pub session: SessionId,
    pub kind: EventKind,
}

#[cfg(test)]
mod tests;
