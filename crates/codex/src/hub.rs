//! The decisions of the hub that shares the host's one `codex app-server`
//! among every Codex thread, as a state machine.
//!
//! The hub decides when the server starts, which thread each of its events
//! belongs to, what every thread gets when the server exits, and stopping
//! it for a new credential. The server starts with the first thread or model listing asked of it.
//! Asks that arrive while it starts wait for it, and fail with why if it
//! can't. When it exits, every thread gets the exit and the hub forgets the
//! server, so the next ask starts a new one, on which a thread continued
//! after resumes (`thread/resume`). A new credential stops the server the
//! same way, since its key is in the server's environment, fixed at start.
//!
//! `codex-cli` runs the hub's task: it turns each command and server event
//! into an [`Input`] and performs each [`Effect`]. A thread's ask is the
//! opaque `T`, a model listing's `M`, a credential change's answer `C`, and
//! a thread's route to its session `R`; `E` is why the server didn't start.

use crate::CodexCredential;
use agents::Exit;

use crate::prelude::*;
use crate::{CodexEvent, ThreadEvent, ThreadId};

/// What a thread's session gets from the hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routed {
    /// One of the thread's events, or the server's exit.
    Codex(CodexEvent),
    /// The hub stopped the server on purpose, for this reason.
    Stopped { reason: String },
}

/// Unrouted thread events received while open requests are pending. After a
/// lost event, subsequent opens fail until the current batch has settled.
#[derive(Debug, Default)]
pub struct EarlyEvents {
    by_thread: BTreeMap<ThreadId, Vec<CodexEvent>>,
    count: usize,
    overflowed: bool,
}

impl EarlyEvents {
    /// Buffers an opening thread's event, or returns an event for diagnostics.
    pub fn capture(
        &mut self,
        event: CodexEvent,
        opening: bool,
    ) -> Option<CodexEvent> {
        const LIMIT: usize = 4096;
        if let CodexEvent::Thread { thread, .. } = &event
            && opening
        {
            if self.count < LIMIT {
                self.count += 1;
                self.by_thread
                    .entry(thread.clone())
                    .or_default()
                    .push(event);
            } else {
                self.overflowed = true;
            }
            None
        } else {
            Some(event)
        }
    }

    #[must_use]
    pub const fn overflowed(&self) -> bool {
        self.overflowed
    }

    pub fn take(&mut self, thread: &ThreadId) -> Vec<CodexEvent> {
        let events = self.by_thread.remove(thread).unwrap_or_default();
        self.count -= events.len();
        events
    }
}

/// What a handle asks of the hub.
#[derive(Debug)]
pub enum Command<T, M, C> {
    /// Starts or resumes a thread on the server.
    Thread(T),
    /// Lists the server's models.
    Models(M),
    /// Runs the next server with this credential, stopping the current one.
    Credential {
        credential: Option<CodexCredential>,
        reply: C,
    },
}

/// What happened, as the shell saw it.
#[derive(Debug)]
pub enum Input<T, M, C, R, E> {
    Command(Command<T, M, C>),
    /// Whether applying the current provider to its private config succeeded.
    Applied(bool),
    /// How the start [`Effect::Spawn`] asked for went.
    Spawned(Result<(), E>),
    /// The thread an [`Effect::Open`] started, and its route.
    Opened {
        thread: ThreadId,
        route: R,
        generation: u64,
    },
    /// The running server's next event, `None` once its events end.
    Event(Option<CodexEvent>),
    /// The thread's session takes no more events.
    Gone {
        thread: ThreadId,
        generation: u64,
    },
    /// Every handle is gone.
    Closed,
}

/// An ask that needs the server running.
#[derive(Debug, PartialEq, Eq)]
pub enum Ask<T, M> {
    Thread(T),
    Models(M),
}

/// What the shell does next, in order.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect<T, M, C, R, E> {
    /// Start the server with this credential, then report
    /// [`Input::Spawned`].
    Spawn(Option<CodexCredential>),
    /// Start a server whose private config already carries this provider.
    SpawnConfigured(Option<CodexCredential>),
    /// Start the thread on the running server and answer it, then report
    /// [`Input::Opened`] if it started.
    Open(T),
    /// Answer with the running server's models.
    List(M),
    /// Answer an ask: the server didn't start.
    Fail(Ask<T, M>, E),
    /// Pass a thread its event, and report [`Input::Gone`] if its session
    /// takes no more.
    Route {
        thread: ThreadId,
        route: R,
        routed: Routed,
    },
    /// Pass a thread its last update, after which the hub forgets it.
    Close { route: R, routed: Routed },
    /// Shut the running server down and forget it.
    Stop,
    /// The running server exited like this; forget it.
    Lost(Exit),
    /// No thread takes this event.
    Unrouted(CodexEvent),
    /// Answer a credential change.
    Configured(C),
    /// End the hub.
    End,
}

#[derive(Debug, PartialEq, Eq)]
enum State<T, M> {
    /// No server runs: none has started yet, or the last one has gone.
    Down,
    /// A server is starting, and these asks wait for it. A `stale` one
    /// started before the credential changed.
    Starting {
        waiting: Vec<Ask<T, M>>,
        stale: bool,
    },
    Up,
}

/// The hub's state: the credential the next server starts with, whether
/// one runs, and the route of each of its threads.
#[derive(Debug, PartialEq, Eq)]
pub struct Hub<T, M, R> {
    credential: Option<CodexCredential>,
    home_configured: bool,
    state: State<T, M>,
    routes: BTreeMap<ThreadId, (u64, R)>,
}

impl<T, M, R: Clone> Hub<T, M, R> {
    /// A hub with no server yet, whose first one starts with `credential`.
    #[must_use]
    pub const fn new(credential: Option<CodexCredential>) -> Self {
        Self {
            credential,
            home_configured: false,
            state: State::Down,
            routes: BTreeMap::new(),
        }
    }

    #[must_use]
    pub const fn credential(&self) -> Option<&CodexCredential> {
        self.credential.as_ref()
    }

    pub fn step<C, E: Clone>(
        &mut self,
        input: Input<T, M, C, R, E>,
    ) -> Vec<Effect<T, M, C, R, E>> {
        match input {
            Input::Command(Command::Thread(asked)) => {
                self.ask(Ask::Thread(asked))
            }
            Input::Command(Command::Models(asked)) => {
                self.ask(Ask::Models(asked))
            }
            Input::Command(Command::Credential { credential, reply }) => {
                self.configure(credential, reply)
            }
            Input::Applied(applied) => {
                self.home_configured = applied;
                Vec::new()
            }
            Input::Spawned(Ok(())) => self.spawned(),
            Input::Spawned(Err(error)) => self.unspawned(&error),
            Input::Opened {
                thread,
                route,
                generation,
            } => self.opened(thread, route, generation),
            Input::Event(Some(CodexEvent::Thread { thread, event })) => {
                self.route(thread, event)
            }
            Input::Event(Some(CodexEvent::Exited { status })) => {
                self.exited(status)
            }
            Input::Event(Some(unknown @ CodexEvent::Unknown(_))) => {
                Vec::from([Effect::Unrouted(unknown)])
            }
            Input::Event(None) => self.exited(Exit::Unknown(String::from(
                "the app-server's events ended",
            ))),
            Input::Gone { thread, generation } => {
                if self
                    .routes
                    .get(&thread)
                    .is_some_and(|(current, _)| *current == generation)
                {
                    self.routes.remove(&thread);
                }
                Vec::new()
            }
            Input::Closed => Vec::from([Effect::End]),
        }
    }

    /// Takes the ask on the running server, or waits for one to start.
    fn ask<C, E>(&mut self, ask: Ask<T, M>) -> Vec<Effect<T, M, C, R, E>> {
        match &mut self.state {
            State::Up => Vec::from([take(ask)]),
            State::Starting { waiting, .. } => {
                waiting.push(ask);
                Vec::new()
            }
            State::Down => {
                self.state = State::Starting {
                    waiting: Vec::from([ask]),
                    stale: false,
                };
                Vec::from([self.launch()])
            }
        }
    }

    fn launch<C, E>(&self) -> Effect<T, M, C, R, E> {
        if self.home_configured {
            Effect::SpawnConfigured(self.credential.clone())
        } else {
            Effect::Spawn(self.credential.clone())
        }
    }

    /// Keeps the credential for the next server. A running one stops, and
    /// its threads close, saying why; a starting one restarts once up.
    fn configure<C, E>(
        &mut self,
        credential: Option<CodexCredential>,
        reply: C,
    ) -> Vec<Effect<T, M, C, R, E>> {
        self.credential = credential;
        self.home_configured = false;
        let mut effects = match &mut self.state {
            State::Up => {
                self.state = State::Down;
                let mut stopped = Vec::from([Effect::Stop]);
                stopped.extend(self.close_all(&Routed::Stopped {
                    reason: String::from("new Codex credential"),
                }));
                stopped
            }
            State::Starting { stale, .. } => {
                *stale = true;
                Vec::new()
            }
            State::Down => Vec::new(),
        };
        effects.push(Effect::Configured(reply));
        effects
    }

    /// Takes the waiting asks on the new server, unless it started with an
    /// old credential, when it is replaced first.
    fn spawned<C, E>(&mut self) -> Vec<Effect<T, M, C, R, E>> {
        match core::mem::replace(&mut self.state, State::Up) {
            State::Starting {
                waiting,
                stale: true,
            } => {
                self.state = State::Starting {
                    waiting,
                    stale: false,
                };
                Vec::from([Effect::Stop, self.launch()])
            }
            State::Starting {
                waiting,
                stale: false,
            } => waiting.into_iter().map(take).collect(),
            State::Up | State::Down => Vec::new(),
        }
    }

    fn unspawned<C, E: Clone>(
        &mut self,
        error: &E,
    ) -> Vec<Effect<T, M, C, R, E>> {
        match core::mem::replace(&mut self.state, State::Down) {
            State::Starting { waiting, .. } => waiting
                .into_iter()
                .map(|ask| Effect::Fail(ask, error.clone()))
                .collect(),
            State::Up | State::Down => Vec::new(),
        }
    }

    /// Routes the thread's events from now on. A thread opened on a server
    /// that has gone gets the exit at once.
    fn opened<C, E>(
        &mut self,
        thread: ThreadId,
        route: R,
        generation: u64,
    ) -> Vec<Effect<T, M, C, R, E>> {
        match self.state {
            State::Up => {
                self.routes.insert(thread, (generation, route));
                Vec::new()
            }
            State::Starting { .. } | State::Down => {
                Vec::from([Effect::Close {
                    route,
                    routed: Routed::Codex(CodexEvent::Exited {
                        status: Exit::Unknown(String::from(
                            "the app-server had gone",
                        )),
                    }),
                }])
            }
        }
    }

    /// An event for a thread whose session has ended goes nowhere.
    fn route<C, E>(
        &self,
        thread: ThreadId,
        event: ThreadEvent,
    ) -> Vec<Effect<T, M, C, R, E>> {
        let codex = CodexEvent::Thread {
            thread: thread.clone(),
            event,
        };
        match self.routes.get(&thread) {
            Some((_, route)) => Vec::from([Effect::Route {
                thread,
                route: route.clone(),
                routed: Routed::Codex(codex),
            }]),
            None => Vec::from([Effect::Unrouted(codex)]),
        }
    }

    /// Forgets the server and tells every thread it has gone.
    fn exited<C, E>(&mut self, status: Exit) -> Vec<Effect<T, M, C, R, E>> {
        match self.state {
            State::Up => {
                self.state = State::Down;
                let last = Routed::Codex(CodexEvent::Exited {
                    status: status.clone(),
                });
                let mut effects = Vec::from([Effect::Lost(status)]);
                effects.extend(self.close_all(&last));
                effects
            }
            State::Starting { .. } | State::Down => Vec::new(),
        }
    }

    fn close_all<C, E>(&mut self, last: &Routed) -> Vec<Effect<T, M, C, R, E>> {
        core::mem::take(&mut self.routes)
            .into_values()
            .map(|(_, route)| Effect::Close {
                route,
                routed: last.clone(),
            })
            .collect()
    }
}

/// What the running server does for an ask.
fn take<T, M, C, R, E>(ask: Ask<T, M>) -> Effect<T, M, C, R, E> {
    match ask {
        Ask::Thread(asked) => Effect::Open(asked),
        Ask::Models(asked) => Effect::List(asked),
    }
}

#[cfg(test)]
mod tests {
    use crate::CodexCredential;
    use agents::Exit;
    use serde_json::json;

    use super::{Ask, Command, Effect, Hub, Input, Routed};
    use crate::{CodexEvent, ThreadEvent, ThreadId, TurnId};

    type Step = Effect<u8, char, &'static str, u16, &'static str>;
    type Seen = Input<u8, char, &'static str, u16, &'static str>;

    fn walk(hub: &mut Hub<u8, char, u16>, inputs: Vec<Seen>) -> Vec<Vec<Step>> {
        inputs.into_iter().map(|input| hub.step(input)).collect()
    }

    fn thread(asked: u8) -> Seen {
        Input::Command(Command::Thread(asked))
    }

    fn models(asked: char) -> Seen {
        Input::Command(Command::Models(asked))
    }

    fn credential(key: &str, reply: &'static str) -> Seen {
        Input::Command(Command::Credential {
            credential: Some(key_credential(key)),
            reply,
        })
    }

    fn key_credential(key: &str) -> CodexCredential {
        CodexCredential::Provider {
            name: String::from("gateway"),
            base_url: String::from("https://gateway.test"),
            key: String::from(key),
            model: String::from("m"),
        }
    }

    fn id(name: &str) -> ThreadId {
        ThreadId(String::from(name))
    }

    fn opened(name: &str, route: u16) -> Seen {
        Input::Opened {
            thread: id(name),
            route,
            generation: u64::from(route),
        }
    }

    fn turn(name: &str) -> CodexEvent {
        CodexEvent::Thread {
            thread: id(name),
            event: ThreadEvent::TurnStarted {
                turn: TurnId(String::from("tu_1")),
            },
        }
    }

    fn exit() -> Exit {
        Exit::Status(String::from("exit status: 1"))
    }

    fn exited() -> Routed {
        Routed::Codex(CodexEvent::Exited { status: exit() })
    }

    /// A hub whose server runs threads `th_1` on route 1 and `th_2` on 2.
    fn running() -> Hub<u8, char, u16> {
        let mut hub = Hub::new(None);
        let _ = walk(
            &mut hub,
            Vec::from([
                thread(1),
                Input::Spawned(Ok(())),
                opened("th_1", 1),
                thread(2),
                opened("th_2", 2),
            ]),
        );
        hub
    }

    #[test]
    fn asks_wait_for_the_server_the_first_one_starts() {
        let mut hub = Hub::new(Some(key_credential("k1")));
        let seen = walk(
            &mut hub,
            Vec::from([
                thread(1),
                models('a'),
                thread(2),
                Input::Spawned(Ok(())),
                models('b'),
            ]),
        );
        assert_eq!(
            seen,
            [
                Vec::from([Effect::Spawn(Some(key_credential("k1")))]),
                Vec::new(),
                Vec::new(),
                Vec::from([
                    Effect::Open(1),
                    Effect::List('a'),
                    Effect::Open(2)
                ]),
                Vec::from([Effect::List('b')]),
            ]
        );
    }

    #[test]
    fn a_server_that_does_not_start_fails_every_waiting_ask() {
        let mut hub = Hub::new(None);
        let seen = walk(
            &mut hub,
            Vec::from([
                thread(1),
                models('a'),
                Input::Spawned(Err("no codex")),
                thread(2),
            ]),
        );
        assert_eq!(
            &seen[2..],
            [
                Vec::from([
                    Effect::Fail(Ask::Thread(1), "no codex"),
                    Effect::Fail(Ask::Models('a'), "no codex"),
                ]),
                Vec::from([Effect::Spawn(None)]),
            ]
        );
    }

    #[test]
    fn each_event_goes_to_its_thread_until_its_session_is_gone() {
        let mut hub = running();
        let unknown = CodexEvent::Unknown(json!({ "method": "x" }));
        let seen = walk(
            &mut hub,
            Vec::from([
                Input::Event(Some(turn("th_2"))),
                Input::Gone {
                    thread: id("th_2"),
                    generation: 2,
                },
                Input::Event(Some(turn("th_2"))),
                Input::Event(Some(unknown.clone())),
            ]),
        );
        assert_eq!(
            seen,
            [
                Vec::from([Effect::Route {
                    thread: id("th_2"),
                    route: 2,
                    routed: Routed::Codex(turn("th_2")),
                }]),
                Vec::new(),
                Vec::from([Effect::Unrouted(turn("th_2"))]),
                Vec::from([Effect::Unrouted(unknown)]),
            ]
        );
    }

    #[test]
    fn every_thread_gets_the_exit_and_the_next_ask_starts_a_server() {
        let mut hub = running();
        let seen = walk(
            &mut hub,
            Vec::from([
                Input::Event(Some(CodexEvent::Exited { status: exit() })),
                Input::Event(Some(turn("th_1"))),
                thread(3),
            ]),
        );
        assert_eq!(
            seen,
            [
                Vec::from([
                    Effect::Lost(exit()),
                    Effect::Close {
                        route: 1,
                        routed: exited()
                    },
                    Effect::Close {
                        route: 2,
                        routed: exited()
                    },
                ]),
                Vec::from([Effect::Unrouted(turn("th_1"))]),
                Vec::from([Effect::Spawn(None)]),
            ]
        );
    }

    #[test]
    fn events_that_end_count_as_an_exit() {
        let mut hub = running();
        let seen: Vec<Step> = hub.step(Input::Event(None));
        let status =
            Exit::Unknown(String::from("the app-server's events ended"));
        assert_eq!(seen.first(), Some(&Effect::Lost(status)));
    }

    #[test]
    fn a_new_credential_stops_the_server_and_closes_its_threads() {
        let mut hub = running();
        let stopped = || Routed::Stopped {
            reason: String::from("new Codex credential"),
        };
        let seen = walk(
            &mut hub,
            Vec::from([
                credential("k2", "first"),
                credential("k3", "again"),
                Input::Applied(true),
                models('a'),
            ]),
        );
        assert_eq!(
            seen,
            [
                Vec::from([
                    Effect::Stop,
                    Effect::Close {
                        route: 1,
                        routed: stopped()
                    },
                    Effect::Close {
                        route: 2,
                        routed: stopped()
                    },
                    Effect::Configured("first"),
                ]),
                Vec::from([Effect::Configured("again")]),
                Vec::new(),
                Vec::from([Effect::SpawnConfigured(Some(key_credential(
                    "k3"
                )))]),
            ]
        );
    }

    #[test]
    fn a_server_starting_with_an_old_credential_is_replaced_before_use() {
        let mut hub = Hub::new(None);
        let seen = walk(
            &mut hub,
            Vec::from([
                models('a'),
                credential("k2", "set"),
                Input::Spawned(Ok(())),
                Input::Spawned(Ok(())),
            ]),
        );
        assert_eq!(
            &seen[1..],
            [
                Vec::from([Effect::Configured("set")]),
                Vec::from([
                    Effect::Stop,
                    Effect::Spawn(Some(key_credential("k2")))
                ]),
                Vec::from([Effect::List('a')]),
            ]
        );
    }

    #[test]
    fn a_thread_opened_after_its_server_went_gets_the_exit() {
        let mut hub = Hub::<u8, char, u16>::new(None);
        let seen: Vec<Step> = hub.step(opened("th_1", 1));
        assert_eq!(
            seen,
            Vec::from([Effect::Close {
                route: 1,
                routed: Routed::Codex(CodexEvent::Exited {
                    status: Exit::Unknown(String::from(
                        "the app-server had gone"
                    )),
                }),
            }])
        );
    }

    #[test]
    fn closing_the_hub_ends_it() {
        let mut hub = running();
        let seen: Vec<Step> = hub.step(Input::Closed);
        assert_eq!(seen, Vec::from([Effect::End]));
    }
}
