//! The event log's rules (architecture §8).
//!
//! Which seq each event takes, which events are stored and which only go
//! live, and which wait in a command's open transaction until it commits.
//! Which commands run, wait or answer from their receipt is the client
//! link's (`link::log`), which opens and commits that transaction.
//!
//! The shell owns the database and the channels. It hands [`Log`] what
//! happened and performs each [`Effect`] in order.

use alloc::collections::BTreeSet;
use alloc::format;
use core::mem;

use agents::Output;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::session::{EventKind, SessionId};

use crate::prelude::*;

/// The event log's format. A database of another format does not open.
pub const FORMAT: u32 = 10;

/// Whether a database opens, and whether its format is to be written: a
/// `created` one takes [`FORMAT`], and one whose `stored` format is another
/// is refused.
///
/// # Errors
/// Why the database doesn't open.
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

/// One write into the open transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    Event(Event),
    Conversation(SessionId, String),
}

/// Routing metadata for a newly opened approval or question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreshRequest {
    pub session: SessionId,
    pub seq: HostSeq,
}

/// What the shell does next. Without a database, `Open`, `Store` and
/// `Commit` do nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Open a write transaction.
    Open,
    /// Write into the open transaction, if there is one.
    Store(Write),
    /// Commit the open transaction, if there is one.
    Commit,
    /// Send the event to live clients.
    Publish(Event),
    /// Announce a newly opened request outside the client event stream.
    RequestOpened(FreshRequest),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Log {
    events: Vec<Event>,
    staged: Vec<Event>,
    open: bool,
}

/// A streaming delta goes live with the last stored event's seq and is
/// never stored (see `Output::AssistantDelta`).
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
    /// The log holding the stored `events`.
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

    /// Admission in the session actor also sees the changes it just staged
    /// for a command whose receipt has yet to commit them.
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

    /// The stored events after `after`.
    #[must_use]
    pub fn replay(&self, after: HostSeq) -> Vec<Event> {
        self.events
            .iter()
            .filter(|event| event.seq > after)
            .cloned()
            .collect()
    }

    /// The last stored event's seq, or 0 before the first.
    #[must_use]
    pub fn head(&self) -> HostSeq {
        self.events
            .last()
            .map_or(HostSeq::BEFORE_FIRST, |event| event.seq)
    }

    /// Records an event: stored with the next seq, or live only with the
    /// last. An open command's events wait for it to commit.
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

    /// Saves the session's conversation id, in the open command's
    /// transaction if there is one.
    #[must_use]
    pub fn conversation(&self, session: SessionId, id: String) -> Vec<Effect> {
        let write = Write::Conversation(session, id);
        if self.open {
            Vec::from([Effect::Store(write)])
        } else {
            alone(write)
        }
    }

    /// A command's transaction opens: its events wait for it to commit.
    pub const fn open(&mut self) -> Effect {
        self.open = true;
        Effect::Open
    }

    /// Commits what the open command has staged before changing the
    /// workspace, and keeps it open in a new transaction.
    pub fn barrier(&mut self) -> Vec<Effect> {
        let mut effects = self.commit();
        if self.open {
            effects.push(Effect::Open);
        }
        effects
    }

    /// Commits the transaction and publishes what it staged; the command
    /// stays open, if one was.
    pub fn commit(&mut self) -> Vec<Effect> {
        let mut effects = Vec::from([Effect::Commit]);
        for event in mem::take(&mut self.staged) {
            self.publish_fresh(event, &mut effects);
        }
        effects
    }

    /// Commits the command's transaction and closes it.
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

/// The host's event cursor. Monotonic per [`Epoch`]; the first event is 1.
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

/// Identifies one incarnation of a host's event log. A cursor from another
/// epoch is meaningless, so the client starts over from seq 0 (§8).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct Epoch(
    #[serde(with = "crate::json::decimal")]
    #[schemars(with = "String")]
    pub u64,
);

/// Every event names its session. A stored event has a seq of its own; an
/// `AssistantDelta` repeats the last one's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    pub seq: HostSeq,
    pub session: SessionId,
    pub kind: EventKind,
}

#[cfg(test)]
mod tests;
