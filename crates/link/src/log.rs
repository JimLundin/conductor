//! The client link's rules on the host's log (architecture §8).
//!
//! Which commands run, wait or answer from their receipt, by command id;
//! when a command's events commit with its receipt; the `Welcome` a
//! connection starts with; and the catch-up a `Subscribe` asks for. The seq
//! of each event, and which are stored, are the event log's (`host::log`).
//!
//! The shell owns the database and the channels. It hands [`Log`] what
//! happened and performs each [`Effect`] in order.

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use core::mem;

use home::Home;
use host::log::Effect as Logged;
use proto::{
    Catalog, CommandId, Epoch, Event, EventKind, HostSeq, Instances, SessionId,
};

use crate::{CommandResult, Subscribe, Welcome};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Begun {
    /// The command already ran: answer its receipt.
    Cached(CommandResult),
    /// Run the command, then finish it.
    Execute(CommandId),
}

/// What the shell does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// What the event log decided.
    Log(Logged),
    /// Write the command's receipt into the open transaction, if there is
    /// one.
    Receipt(CommandId, CommandResult),
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
    events: host::log::Log,
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

impl Log {
    /// The log of `name` in `epoch`, holding the stored `events`.
    #[must_use]
    pub const fn new(name: String, epoch: Epoch, events: Vec<Event>) -> Self {
        Self {
            name,
            epoch,
            events: host::log::Log::new(events),
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
        self.events.history()
    }

    #[must_use]
    pub fn running_sessions(&self) -> BTreeSet<SessionId> {
        self.events.running_sessions()
    }

    #[must_use]
    pub fn current_history(&self, session: SessionId) -> Vec<Event> {
        self.events.current_history(session)
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
            head_seq: self.events.head(),
            instances,
            catalog,
            home,
        }
    }

    /// The stored events after `after`.
    #[must_use]
    pub fn replay(&self, after: HostSeq) -> Vec<Event> {
        self.events.replay(after)
    }

    /// The replay a subscription catches up with, or `None` for a cursor
    /// from another epoch, which the host refuses.
    #[must_use]
    pub fn resume(&self, subscribe: &Subscribe) -> Option<Vec<Event>> {
        (subscribe.epoch == self.epoch)
            .then(|| self.replay(subscribe.after_seq))
    }

    /// Records an event, as the event log decides; the active command's
    /// events wait for it to commit.
    pub fn append(
        &mut self,
        session: SessionId,
        kind: EventKind,
    ) -> (Event, Vec<Effect>) {
        let (event, effects) = self.events.append(session, kind);
        (event, logged(effects))
    }

    /// Saves the session's conversation id, in the active command's
    /// transaction if there is one.
    #[must_use]
    pub fn conversation(&self, session: SessionId, id: String) -> Vec<Effect> {
        logged(self.events.conversation(session, id))
    }

    /// Commits what the active command has staged before changing the
    /// workspace, and keeps it active in a new transaction.
    pub fn barrier(&mut self) -> Vec<Effect> {
        logged(self.events.barrier())
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
                        effects.push(Effect::Log(self.events.open()));
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
            effects.push(Effect::Receipt(finish.key, finish.result.clone()));
        }
        effects.extend(logged(self.events.close()));
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
            effects.extend([
                Effect::Log(Logged::Open),
                Effect::Receipt(finish.key, finish.result.clone()),
                Effect::Log(Logged::Commit),
            ]);
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

    const fn ticket(&mut self) -> Ticket {
        self.tickets = self.tickets.wrapping_add(1);
        Ticket(self.tickets)
    }
}

fn logged(effects: Vec<Logged>) -> Vec<Effect> {
    effects.into_iter().map(Effect::Log).collect()
}

#[cfg(test)]
mod tests;
