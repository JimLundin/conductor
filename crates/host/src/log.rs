//! The event log's rules (architecture §8).
//!
//! Which seq each event takes, which events are stored and which only go
//! live, when a command's events commit, and which commands run, wait or
//! answer from their receipt.
//!
//! The shell owns the database and the channels. It hands [`Log`] what
//! happened and performs each [`Effect`] in order.

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::format;
use core::mem;

use home::Home;
use proto::{
    Catalog, CommandId, CommandResult, Epoch, Event, EventKind, HostSeq,
    Instances, SessionId, Subscribe, Welcome,
};

use crate::prelude::*;

/// Format 8 derives each session's provider from its instance. A database
/// of another format does not open.
pub const FORMAT: u32 = 9;

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

/// Names a `begin` or a `finish` the shell answers later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ticket(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Runs alone; its events and receipt commit together.
    Session,
    /// Runs beside session commands and commits its receipt on its own.
    Receipt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completion {
    Final(CommandResult),
    Retryable(CommandResult),
}

impl Completion {
    #[must_use]
    pub fn into_result(self) -> CommandResult {
        match self {
            Self::Final(result) | Self::Retryable(result) => result,
        }
    }
}

impl From<CommandResult> for Completion {
    fn from(result: CommandResult) -> Self {
        Self::Final(result)
    }
}

/// One write into the open transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    Event(Event),
    Receipt(CommandId, CommandResult),
    Conversation(SessionId, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Begun {
    /// The command already ran: answer its receipt.
    Cached(CommandResult),
    /// Run the command, then finish it.
    Execute(CommandId),
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
    /// Read the command's stored receipt and hand it to [`Log::receipt`].
    Lookup(CommandId),
    Begun(Ticket, Begun),
    Finished(Ticket),
    /// Stop the log: a command was dropped unfinished, so its effects may
    /// have happened without its receipt.
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Start {
    ticket: Ticket,
    key: CommandId,
    kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Finish {
    ticket: Ticket,
    key: CommandId,
    keep_receipt: bool,
    result: CommandResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Log {
    name: String,
    epoch: Epoch,
    /// The stored events, in seq order.
    events: Vec<Event>,
    /// The active command's events, published when it commits.
    staged: Vec<Event>,
    active: Option<CommandId>,
    active_waiters: Vec<Ticket>,
    waiting: VecDeque<Start>,
    /// The start whose receipt the shell is reading.
    starting: Option<Start>,
    /// Each running receipt command, and the retries waiting for it.
    receipts: BTreeMap<CommandId, Vec<Ticket>>,
    /// Credential finishes held until the active command commits, since
    /// only one transaction can be open.
    held: VecDeque<Finish>,
    tickets: u64,
}

/// A streaming delta goes live with the last stored event's seq and is
/// never stored (see `EventKind::AssistantDelta`).
#[must_use]
pub const fn live_only(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::AssistantDelta { .. } | EventKind::ReasoningDelta { .. }
    )
}

impl Log {
    /// The log of `name` in `epoch`, holding the stored `events`.
    #[must_use]
    pub const fn new(name: String, epoch: Epoch, events: Vec<Event>) -> Self {
        Self {
            name,
            epoch,
            events,
            staged: Vec::new(),
            active: None,
            active_waiters: Vec::new(),
            waiting: VecDeque::new(),
            starting: None,
            receipts: BTreeMap::new(),
            held: VecDeque::new(),
            tickets: 0,
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

    #[must_use]
    pub fn welcome(
        &self,
        instances: Instances,
        catalog: Catalog,
        home: Option<Home>,
    ) -> Welcome {
        Welcome {
            host: self.name.clone(),
            epoch: self.epoch,
            head_seq: self.head(),
            instances,
            catalog,
            home,
        }
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

    /// The replay a subscription catches up with, or `None` for a cursor
    /// from another epoch, which the host refuses.
    #[must_use]
    pub fn resume(&self, subscribe: &Subscribe) -> Option<Vec<Event>> {
        (subscribe.epoch == self.epoch)
            .then(|| self.replay(subscribe.after_seq))
    }

    /// Records an event: stored with the next seq, or live only with the
    /// last. The active command's events wait for it to commit.
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
        let effects = match (self.active, live) {
            (Some(_), true) => {
                self.staged.push(event.clone());
                Vec::new()
            }
            (Some(_), false) => {
                self.staged.push(event.clone());
                Vec::from([Effect::Store(Write::Event(event.clone()))])
            }
            (None, _) => {
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

    /// Saves the session's conversation id, in the active command's
    /// transaction if there is one.
    #[must_use]
    pub fn conversation(&self, session: SessionId, id: String) -> Vec<Effect> {
        let write = Write::Conversation(session, id);
        match self.active {
            Some(_) => Vec::from([Effect::Store(write)]),
            None => alone(write),
        }
    }

    /// Commits what the active command has staged before changing the workspace,
    /// and keeps it active in a new transaction.
    pub fn barrier(&mut self) -> Vec<Effect> {
        let mut effects = self.commit();
        if self.active.is_some() {
            effects.push(Effect::Open);
        }
        effects
    }

    /// A command started: it waits behind the active command, or the shell
    /// looks up its receipt.
    pub fn begin(
        &mut self,
        key: CommandId,
        kind: Kind,
    ) -> (Ticket, Vec<Effect>) {
        let start = Start {
            ticket: self.ticket(),
            key,
            kind,
        };
        let effects = match (self.active, self.receipts.get_mut(&key)) {
            (Some(active), _) if active == key => {
                self.active_waiters.push(start.ticket);
                Vec::new()
            }
            (Some(_), Some(waiters)) => {
                waiters.push(start.ticket);
                Vec::new()
            }
            (Some(_), None) => {
                self.waiting.push_back(start);
                Vec::new()
            }
            (None, _) => Vec::from([self.look_up(start)]),
        };
        (start.ticket, effects)
    }

    /// The receipt the shell found for the start it looked up.
    pub fn receipt(&mut self, found: Option<CommandResult>) -> Vec<Effect> {
        let Some(start) = self.starting.take() else {
            return Vec::new();
        };
        let mut effects = Vec::new();
        match (found, self.receipts.get_mut(&start.key)) {
            (Some(result), _) => {
                effects
                    .push(Effect::Begun(start.ticket, Begun::Cached(result)));
            }
            (None, Some(waiters)) => waiters.push(start.ticket),
            (None, None) => {
                match start.kind {
                    Kind::Session => {
                        effects.push(Effect::Open);
                        self.active = Some(start.key);
                        self.waiting.retain(|waiting| {
                            if waiting.key == start.key {
                                self.active_waiters.push(waiting.ticket);
                                false
                            } else {
                                true
                            }
                        });
                    }
                    Kind::Receipt => {
                        self.receipts.insert(start.key, Vec::new());
                    }
                }
                effects.push(Effect::Begun(
                    start.ticket,
                    Begun::Execute(start.key),
                ));
            }
        }
        effects.extend(self.next());
        effects
    }

    /// A command finished. A final result's receipt commits with its
    /// events. A retryable result answers only the current callers.
    pub fn finish(
        &mut self,
        key: CommandId,
        completion: impl Into<Completion>,
    ) -> (Ticket, Vec<Effect>) {
        let outcome = completion.into();
        let finish = Finish {
            ticket: self.ticket(),
            key,
            keep_receipt: matches!(outcome, Completion::Final(_)),
            result: outcome.into_result(),
        };
        let ticket = finish.ticket;
        let effects = match (self.receipts.contains_key(&key), self.active) {
            (true, Some(_)) => {
                self.held.push_back(finish);
                Vec::new()
            }
            (true, None) => self.settle(&finish),
            (false, _) => self.close(&finish),
        };
        (ticket, effects)
    }

    /// A command's lease was dropped unfinished. A running one stops the
    /// log, which rolls back its open transaction.
    #[must_use]
    pub fn abort(&self, key: CommandId) -> Vec<Effect> {
        let running =
            self.receipts.contains_key(&key) || self.active == Some(key);
        running.then_some(Effect::Stop).into_iter().collect()
    }

    fn close(&mut self, finish: &Finish) -> Vec<Effect> {
        let mut effects = Vec::new();
        if self.active.is_some() && finish.keep_receipt {
            effects.push(Effect::Store(Write::Receipt(
                finish.key,
                finish.result.clone(),
            )));
        }
        effects.extend(self.commit());
        self.active = None;
        effects.extend(mem::take(&mut self.active_waiters).into_iter().map(
            |waiter| {
                Effect::Begun(waiter, Begun::Cached(finish.result.clone()))
            },
        ));
        effects.push(Effect::Finished(finish.ticket));
        for held in mem::take(&mut self.held) {
            effects.extend(self.settle(&held));
        }
        effects.extend(self.next());
        effects
    }

    fn settle(&mut self, finish: &Finish) -> Vec<Effect> {
        let mut effects = Vec::new();
        if finish.keep_receipt {
            effects.extend(alone(Write::Receipt(
                finish.key,
                finish.result.clone(),
            )));
        }
        let waiters = self.receipts.remove(&finish.key).unwrap_or_default();
        effects.extend(waiters.into_iter().map(|waiter| {
            Effect::Begun(waiter, Begun::Cached(finish.result.clone()))
        }));
        effects.push(Effect::Finished(finish.ticket));
        effects
    }

    fn next(&mut self) -> Option<Effect> {
        if self.active.is_some() {
            return None;
        }
        let start = self.waiting.pop_front()?;
        Some(self.look_up(start))
    }

    const fn look_up(&mut self, start: Start) -> Effect {
        self.starting = Some(start);
        Effect::Lookup(start.key)
    }

    fn commit(&mut self) -> Vec<Effect> {
        let mut effects = Vec::from([Effect::Commit]);
        for event in mem::take(&mut self.staged) {
            self.publish_fresh(event, &mut effects);
        }
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

    fn head(&self) -> HostSeq {
        self.events.last().map_or(HostSeq(0), |event| event.seq)
    }

    const fn ticket(&mut self) -> Ticket {
        self.tickets = self.tickets.wrapping_add(1);
        Ticket(self.tickets)
    }
}

/// A write in a transaction of its own.
fn alone(write: Write) -> Vec<Effect> {
    Vec::from([Effect::Open, Effect::Store(write), Effect::Commit])
}

#[cfg(test)]
mod tests;
