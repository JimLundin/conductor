use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use core::mem;

use crate::CommandId;
use home::Home;
use host::log::Effect as Logged;
use host::log::{Epoch, Event, HostSeq};
use host::registry::{Catalog, Instances};
use host::session::{EventKind, SessionId};

use crate::{CommandResult, Subscribe, Welcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Lease(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Session,
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
    Cached(CommandResult),
    Execute(CommandId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Log(Logged),
    Receipt(CommandId, CommandResult),
    Lookup(CommandId),
    Begun(Lease, Begun),
    Finished(Lease),
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Start {
    lease: Lease,
    key: CommandId,
    kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Finish {
    lease: Lease,
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
    active_waiters: Vec<Lease>,
    waiting: VecDeque<Start>,
    receipt_lookup: Option<Start>,
    receipt_waiters: BTreeMap<CommandId, Vec<Lease>>,
    finishes_after_active_commit: VecDeque<Finish>,
    leases: u64,
}

impl Log {
    #[must_use]
    pub const fn new(name: String, epoch: Epoch, events: Vec<Event>) -> Self {
        Self {
            name,
            epoch,
            events: host::log::Log::new(events),
            active: None,
            active_waiters: Vec::new(),
            waiting: VecDeque::new(),
            receipt_lookup: None,
            receipt_waiters: BTreeMap::new(),
            finishes_after_active_commit: VecDeque::new(),
            leases: 0,
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

    #[must_use]
    pub fn replay(&self, after: HostSeq) -> Vec<Event> {
        self.events.replay(after)
    }

    #[must_use]
    pub fn resume(&self, subscribe: &Subscribe) -> Option<Vec<Event>> {
        (subscribe.epoch == self.epoch)
            .then(|| self.replay(subscribe.after_seq))
    }

    pub fn append(
        &mut self,
        session: SessionId,
        kind: EventKind,
    ) -> (Event, Vec<Effect>) {
        let (event, effects) = self.events.append(session, kind);
        (event, logged(effects))
    }

    #[must_use]
    pub fn conversation(&self, session: SessionId, id: String) -> Vec<Effect> {
        logged(self.events.conversation(session, id))
    }

    pub fn barrier(&mut self) -> Vec<Effect> {
        logged(self.events.barrier())
    }

    pub fn begin(
        &mut self,
        key: CommandId,
        kind: Kind,
    ) -> (Lease, Vec<Effect>) {
        let start = Start {
            lease: self.lease(),
            key,
            kind,
        };
        let effects = match (self.active, self.receipt_waiters.get_mut(&key)) {
            (Some(active), _) if active == key => {
                self.active_waiters.push(start.lease);
                Vec::new()
            }
            (Some(_), Some(waiters)) => {
                waiters.push(start.lease);
                Vec::new()
            }
            (Some(_), None) => {
                self.waiting.push_back(start);
                Vec::new()
            }
            (None, _) => Vec::from([self.look_up(start)]),
        };
        (start.lease, effects)
    }

    pub fn receipt(&mut self, found: Option<CommandResult>) -> Vec<Effect> {
        let Some(start) = self.receipt_lookup.take() else {
            return Vec::new();
        };
        let mut effects = Vec::new();
        match (found, self.receipt_waiters.get_mut(&start.key)) {
            (Some(result), _) => {
                effects.push(Effect::Begun(start.lease, Begun::Cached(result)));
            }
            (None, Some(waiters)) => waiters.push(start.lease),
            (None, None) => {
                match start.kind {
                    Kind::Session => {
                        effects.push(Effect::Log(self.events.open()));
                        self.active = Some(start.key);
                        self.waiting.retain(|waiting| {
                            if waiting.key == start.key {
                                self.active_waiters.push(waiting.lease);
                                false
                            } else {
                                true
                            }
                        });
                    }
                    Kind::Receipt => {
                        self.receipt_waiters.insert(start.key, Vec::new());
                    }
                }
                effects.push(Effect::Begun(
                    start.lease,
                    Begun::Execute(start.key),
                ));
            }
        }
        effects.extend(self.next());
        effects
    }

    pub fn finish(
        &mut self,
        key: CommandId,
        completion: impl Into<Completion>,
    ) -> (Lease, Vec<Effect>) {
        let outcome = completion.into();
        let finish = Finish {
            lease: self.lease(),
            key,
            keep_receipt: matches!(outcome, Completion::Final(_)),
            result: outcome.into_result(),
        };
        let lease = finish.lease;
        let effects =
            match (self.receipt_waiters.contains_key(&key), self.active) {
                (true, Some(_)) => {
                    self.finishes_after_active_commit.push_back(finish);
                    Vec::new()
                }
                (true, None) => self.settle(&finish),
                (false, _) => self.close(&finish),
            };
        (lease, effects)
    }

    #[must_use]
    pub fn abort(&self, key: CommandId) -> Vec<Effect> {
        let running =
            self.receipt_waiters.contains_key(&key) || self.active == Some(key);
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
        effects.push(Effect::Finished(finish.lease));
        for deferred_finish in mem::take(&mut self.finishes_after_active_commit)
        {
            effects.extend(self.settle(&deferred_finish));
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
        let waiters =
            self.receipt_waiters.remove(&finish.key).unwrap_or_default();
        effects.extend(waiters.into_iter().map(|waiter| {
            Effect::Begun(waiter, Begun::Cached(finish.result.clone()))
        }));
        effects.push(Effect::Finished(finish.lease));
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
        self.receipt_lookup = Some(start);
        Effect::Lookup(start.key)
    }

    const fn lease(&mut self) -> Lease {
        self.leases = self.leases.wrapping_add(1);
        Lease(self.leases)
    }
}

fn logged(effects: Vec<Logged>) -> Vec<Effect> {
    effects.into_iter().map(Effect::Log).collect()
}

#[cfg(test)]
mod tests;
