use crate::CodexCredential;
use agents::{Exit, Step};

use crate::children::Children;

use crate::prelude::*;
use crate::{CodexEvent, ThreadEvent, ThreadId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routed {
    Codex(CodexEvent),
    Subagent(Vec<Step>),
    Stopped { reason: String },
}

#[derive(Debug, Default)]
pub struct EarlyEvents {
    by_thread: BTreeMap<ThreadId, Vec<CodexEvent>>,
    count: usize,
    overflowed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overflowed;

impl EarlyEvents {
    pub const LIMIT: usize = 4096;

    pub fn capture(
        &mut self,
        event: CodexEvent,
        opening: bool,
    ) -> Option<CodexEvent> {
        if let CodexEvent::Thread { thread, .. } = &event
            && opening
        {
            if self.count < Self::LIMIT {
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

    pub fn open(
        &mut self,
        thread: &ThreadId,
    ) -> Result<Vec<CodexEvent>, Overflowed> {
        if self.overflowed {
            return Err(Overflowed);
        }
        let events = self.by_thread.remove(thread).unwrap_or_default();
        self.count -= events.len();
        Ok(events)
    }

    pub fn settle(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug)]
pub enum Command<ThreadAsk, ModelsAsk, CredentialReply> {
    Thread(ThreadAsk),
    Models(ModelsAsk),
    Credential {
        credential: Option<CodexCredential>,
        reply: CredentialReply,
    },
}

#[derive(Debug)]
pub enum Input<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError> {
    Command(Command<ThreadAsk, ModelsAsk, CredentialReply>),
    Applied(bool),
    Spawned(Result<(), SpawnError>),
    Opened {
        thread: ThreadId,
        route: Route,
        generation: u64,
    },
    Event(Option<CodexEvent>),
    Gone {
        thread: ThreadId,
        generation: u64,
    },
    Closed,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Ask<ThreadAsk, ModelsAsk> {
    Thread(ThreadAsk),
    Models(ModelsAsk),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError> {
    Spawn(Option<CodexCredential>),
    SpawnConfigured(Option<CodexCredential>),
    Open(ThreadAsk),
    List(ModelsAsk),
    Fail(Ask<ThreadAsk, ModelsAsk>, SpawnError),
    Route {
        thread: ThreadId,
        route: Route,
        routed: Routed,
    },
    Close {
        route: Route,
        routed: Routed,
    },
    Stop,
    Lost(Exit),
    Unrouted(CodexEvent),
    Configured(CredentialReply),
    End,
}

#[derive(Debug, PartialEq, Eq)]
enum State<ThreadAsk, ModelsAsk> {
    Down,
    Starting {
        waiting: Vec<Ask<ThreadAsk, ModelsAsk>>,
        stale: bool,
    },
    Up,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Hub<ThreadAsk, ModelsAsk, Route> {
    credential: Option<CodexCredential>,
    home_configured: bool,
    state: State<ThreadAsk, ModelsAsk>,
    routes: BTreeMap<ThreadId, (u64, Route)>,
    children: Children,
}

impl<ThreadAsk, ModelsAsk, Route: Clone> Hub<ThreadAsk, ModelsAsk, Route> {
    #[must_use]
    pub const fn new(credential: Option<CodexCredential>) -> Self {
        Self {
            credential,
            home_configured: false,
            state: State::Down,
            routes: BTreeMap::new(),
            children: Children::new(),
        }
    }

    #[must_use]
    pub const fn credential(&self) -> Option<&CodexCredential> {
        self.credential.as_ref()
    }

    pub fn step<CredentialReply, SpawnError: Clone>(
        &mut self,
        input: Input<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError>,
    ) -> Vec<Effect<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError>>
    {
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
                    self.children.forget(&thread);
                }
                Vec::new()
            }
            Input::Closed => Vec::from([Effect::End]),
        }
    }

    fn ask<CredentialReply, SpawnError>(
        &mut self,
        ask: Ask<ThreadAsk, ModelsAsk>,
    ) -> Vec<Effect<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError>>
    {
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

    fn launch<CredentialReply, SpawnError>(
        &self,
    ) -> Effect<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError> {
        if self.home_configured {
            Effect::SpawnConfigured(self.credential.clone())
        } else {
            Effect::Spawn(self.credential.clone())
        }
    }

    fn configure<CredentialReply, SpawnError>(
        &mut self,
        credential: Option<CodexCredential>,
        reply: CredentialReply,
    ) -> Vec<Effect<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError>>
    {
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

    fn spawned<CredentialReply, SpawnError>(
        &mut self,
    ) -> Vec<Effect<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError>>
    {
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

    fn unspawned<CredentialReply, SpawnError: Clone>(
        &mut self,
        error: &SpawnError,
    ) -> Vec<Effect<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError>>
    {
        match core::mem::replace(&mut self.state, State::Down) {
            State::Starting { waiting, .. } => waiting
                .into_iter()
                .map(|ask| Effect::Fail(ask, error.clone()))
                .collect(),
            State::Up | State::Down => Vec::new(),
        }
    }

    fn opened<CredentialReply, SpawnError>(
        &mut self,
        thread: ThreadId,
        route: Route,
        generation: u64,
    ) -> Vec<Effect<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError>>
    {
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

    fn route<CredentialReply, SpawnError>(
        &mut self,
        thread: ThreadId,
        notice: ThreadEvent,
    ) -> Vec<Effect<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError>>
    {
        match notice {
            ThreadEvent::Child(child) => self
                .children
                .notice(&thread, child, |id| self.routes.contains_key(id))
                .map_or_else(Vec::new, |(root, steps)| {
                    self.route_child(root, steps)
                }),
            event if !self.routes.contains_key(&thread) => {
                if let Some(root) = self.children.root(&thread).cloned() {
                    let steps = self.children.event(&thread, event);
                    self.route_child(root, steps)
                } else {
                    Vec::from([Effect::Unrouted(CodexEvent::Thread {
                        thread,
                        event,
                    })])
                }
            }
            event => self.deliver(
                thread.clone(),
                Routed::Codex(CodexEvent::Thread { thread, event }),
            ),
        }
    }

    fn route_child<CredentialReply, SpawnError>(
        &self,
        root: ThreadId,
        steps: Vec<Step>,
    ) -> Vec<Effect<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError>>
    {
        if steps.is_empty() {
            Vec::new()
        } else {
            self.deliver(root, Routed::Subagent(steps))
        }
    }

    fn deliver<CredentialReply, SpawnError>(
        &self,
        thread: ThreadId,
        routed: Routed,
    ) -> Vec<Effect<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError>>
    {
        self.routes
            .get(&thread)
            .map_or_else(Vec::new, |(_, route)| {
                Vec::from([Effect::Route {
                    thread,
                    route: route.clone(),
                    routed,
                }])
            })
    }

    fn exited<CredentialReply, SpawnError>(
        &mut self,
        status: Exit,
    ) -> Vec<Effect<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError>>
    {
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

    fn close_all<CredentialReply, SpawnError>(
        &mut self,
        last: &Routed,
    ) -> Vec<Effect<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError>>
    {
        self.children = Children::new();
        core::mem::take(&mut self.routes)
            .into_values()
            .map(|(_, route)| Effect::Close {
                route,
                routed: last.clone(),
            })
            .collect()
    }
}

fn take<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError>(
    ask: Ask<ThreadAsk, ModelsAsk>,
) -> Effect<ThreadAsk, ModelsAsk, CredentialReply, Route, SpawnError> {
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

    use super::{
        Ask, Command, EarlyEvents, Effect, Hub, Input, Overflowed, Routed,
    };
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

    #[test]
    fn early_events_past_the_limit_fail_opens_until_the_batch_settles() {
        let mut early = EarlyEvents::default();
        for _ in 0..EarlyEvents::LIMIT {
            assert_eq!(early.capture(turn("th_1"), true), None);
        }
        assert_eq!(early.open(&id("th_2")), Ok(Vec::new()));
        assert_eq!(early.capture(turn("th_1"), true), None);
        assert_eq!(early.open(&id("th_1")), Err(Overflowed));
        assert_eq!(early.open(&id("th_2")), Err(Overflowed));
        early.settle();
        assert_eq!(early.capture(turn("th_2"), true), None);
        assert_eq!(early.open(&id("th_2")), Ok(Vec::from([turn("th_2")])));
    }
}
