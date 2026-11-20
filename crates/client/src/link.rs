//! The decisions of one client's link to one host (architecture §9), as a
//! state machine.
//!
//! The link walks `Disconnected → Connecting → Authenticating → CatchingUp → Live`, falling
//! back to `Disconnected` and retrying with jittered backoff whenever the
//! connection drops. It keeps the cursor (epoch and last applied seq), so
//! every reconnect resubscribes with `after_seq` and the event stream it hands
//! out has no duplicates and no gaps.
//!
//! `net` owns the connection: it performs each [`Effect`] and feeds back what
//! happened as an [`Input`].

use core::cmp::Ordering;
use core::fmt;
use core::time::Duration;

use crate::host_view::is_delta;
use crate::prelude::*;
use proto::{
    CommandResult, Epoch, Event, Feed as Frame, Hello, HelloReply, HostSeq,
    Subscribe, Welcome,
};

const BACKOFF_FLOOR: Duration = Duration::from_millis(250);
const BACKOFF_CEILING: Duration = Duration::from_secs(10);
/// How long a dial may run before a second one starts on a fresh endpoint.
/// A relay connection can stall at the network level for seconds while a
/// fresh one is up in well under a second.
const HEDGE_AFTER: Duration = Duration::from_millis(1500);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkState {
    #[default]
    Disconnected,
    Connecting,
    Authenticating,
    CatchingUp,
    Live,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    pub state: LinkState,
    /// Why the last connection ended, until the next one is live.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub epoch: Epoch,
    pub seq: HostSeq,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    Status(Status),
    /// The events that follow belong to this epoch, starting after seq 0.
    /// Anything folded from an earlier epoch is stale.
    Reset(Epoch),
    Welcome(Welcome),
    Feed(Frame),
    Answer(CommandResult),
}

/// A stream the host may close.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Hello,
    Subscription,
}

impl fmt::Display for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Hello => "hello",
            Self::Subscription => "subscription",
        })
    }
}

/// Why a connection ended. Errors from the transport arrive as their
/// descriptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lost {
    Connect(String),
    Wire(String),
    Refused(String),
    HostClosed(Stream),
    HelloTimeout,
    Gap { after: u64, got: u64 },
}

impl fmt::Display for Lost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(err) => write!(f, "connect failed: {err}"),
            Self::Wire(err) => f.write_str(err),
            Self::Refused(reason) => {
                write!(f, "the host refused us: {reason}")
            }
            Self::HostClosed(stream) => {
                write!(f, "the host closed the {stream} stream")
            }
            Self::HelloTimeout => f.write_str(
                "the host did not complete the hello handshake within 10 \
                 seconds",
            ),
            Self::Gap { after, got } => {
                write!(
                    f,
                    "event {got} arrived after {after}: the feed has a gap"
                )
            }
        }
    }
}

/// One dial of a connection attempt's round. A round dials once from the
/// endpoint the link runs on, and once more from a fresh endpoint if that
/// dial stalls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Attempt(pub u64);

/// Why a dial did not connect. Errors from the transport arrive as their
/// descriptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unreached {
    Connect(String),
    /// No fresh endpoint could be bound to dial from.
    Unbound(String),
}

/// What happened, as the shell saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// The dial `attempt` connected, or why not.
    Dialed {
        attempt: Attempt,
        outcome: Result<(), Unreached>,
    },
    /// The host answered `Hello` with its `Welcome`.
    Welcomed(Welcome),
    /// The subscription is open and its events will follow.
    Subscribed,
    Frame(Frame),
    Answered(CommandResult),
    Lost(Lost),
    /// The random number [`Operation::Draw`] asked for.
    Drawn(u64),
    /// The timer of [`Operation::Sleep`] fired.
    Elapsed,
    /// Asked to drop the connection and connect again at once, as after a
    /// network change.
    Reconnect,
}

impl From<Option<HelloReply>> for Input {
    fn from(reply: Option<HelloReply>) -> Self {
        match reply {
            Some(HelloReply::Welcome(welcome)) => Self::Welcomed(welcome),
            Some(HelloReply::Refused { reason }) => {
                Self::Lost(Lost::Refused(reason))
            }
            None => Self::Lost(Lost::HostClosed(Stream::Hello)),
        }
    }
}

impl From<Option<Frame>> for Input {
    fn from(frame: Option<Frame>) -> Self {
        frame.map_or(
            Self::Lost(Lost::HostClosed(Stream::Subscription)),
            Self::Frame,
        )
    }
}

/// What the shell does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Replace the operation in flight; its outcome is the next input.
    Start(Operation),
    /// Dial the host from the endpoint the link runs on; the outcome is
    /// [`Input::Dialed`].
    Dial(Attempt),
    /// Bind a fresh endpoint and dial the host from it; the outcome is
    /// [`Input::Dialed`].
    Hedge(Attempt),
    /// Run on the attempt's endpoint, and its connection if it made one,
    /// from now on.
    Adopt(Attempt),
    /// Drop the attempt: its dial, its connection and its endpoint.
    Retire(Attempt),
    /// Close the connection.
    Close,
    /// Show this status to every handle, and send it as an update.
    Show(Status),
    Forward(Update),
}

/// The link has one operation in flight at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    /// Send `hello` on the connection; no answer `within` is
    /// [`Lost::HelloTimeout`].
    Hello {
        hello: Hello,
        within: Duration,
    },
    /// Open the subscription; its events follow one by one.
    Subscribe(Subscribe),
    /// Draw a random number for the backoff's jitter.
    Draw,
    Sleep(Duration),
}

/// Where the link is in its walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Waiting,
    Dialing(Round),
    Greeting,
    Subscribing(Feed),
    Connected(Feed),
    BackingOff,
}

/// The epoch a subscription reads and the host's head as of `Welcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Feed {
    epoch: Epoch,
    live_at: HostSeq,
}

/// The dials of one round still in play. A failed `base` stays until the
/// round ends, since the link keeps its endpoint unless another attempt
/// takes over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Round {
    base: Attempt,
    base_dialing: bool,
    /// The fresh endpoint's dial, while it runs.
    hedge: Option<Attempt>,
}

impl Round {
    fn dialing(&self, attempt: Attempt) -> bool {
        (attempt == self.base && self.base_dialing)
            || self.hedge == Some(attempt)
    }

    /// Every attempt of the round but `kept` that still holds an endpoint.
    fn others(&self, kept: Attempt) -> Vec<Effect> {
        [Some(self.base), self.hedge]
            .into_iter()
            .flatten()
            .filter(|attempt| *attempt != kept)
            .map(Effect::Retire)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    hello: Hello,
    cursor: Option<Cursor>,
    failures: u32,
    attempts: u64,
    status: Status,
    phase: Phase,
}

impl Link {
    /// A link that greets with `hello` and dials at once. `resume`
    /// continues an earlier link's cursor, so the first subscription
    /// replays only what that link had not seen.
    #[must_use]
    pub fn start(hello: Hello, resume: Option<Cursor>) -> (Self, Vec<Effect>) {
        let mut link = Self {
            hello,
            cursor: resume,
            failures: 0,
            attempts: 0,
            status: Status::default(),
            phase: Phase::Waiting,
        };
        let effects = link.dial();
        (link, effects)
    }

    #[must_use]
    pub const fn status(&self) -> &Status {
        &self.status
    }

    #[must_use]
    pub const fn cursor(&self) -> Option<Cursor> {
        self.cursor
    }

    /// Whether the link takes requests now. While it connects they wait.
    #[must_use]
    pub const fn takes_requests(&self) -> bool {
        matches!(self.phase, Phase::Waiting | Phase::Connected(_))
    }

    #[must_use]
    pub fn step(mut self, input: Input) -> (Self, Vec<Effect>) {
        let effects = self.on(input);
        (self, effects)
    }

    fn on(&mut self, input: Input) -> Vec<Effect> {
        match (self.phase, input) {
            (Phase::Waiting, Input::Elapsed | Input::Reconnect) => self.dial(),
            (Phase::Connected(_), Input::Reconnect) => {
                let shown = self.disconnected(None);
                join([Vec::from([Effect::Close]), shown, self.dial()])
            }
            (Phase::Dialing(round), Input::Elapsed) => self.hedge(round),
            (Phase::Dialing(round), Input::Dialed { attempt, outcome })
                if round.dialing(attempt) =>
            {
                self.dialed(round, attempt, outcome)
            }
            // A dial the link no longer waits for: drop what it made.
            (_, Input::Dialed { attempt, .. }) => {
                Vec::from([Effect::Retire(attempt)])
            }
            (Phase::Greeting, Input::Welcomed(welcome)) => {
                self.welcome(welcome)
            }
            (Phase::Subscribing(feed), Input::Subscribed) => {
                self.phase = Phase::Connected(feed);
                Vec::new()
            }
            (Phase::Connected(feed), Input::Frame(Frame::Event(event))) => {
                self.apply(feed, event)
            }
            (Phase::Connected(_), Input::Frame(frame)) => {
                Vec::from([Effect::Forward(Update::Feed(frame))])
            }
            (Phase::Connected(_), Input::Answered(answer)) => {
                Vec::from([Effect::Forward(Update::Answer(answer))])
            }
            (
                Phase::Greeting | Phase::Subscribing(_) | Phase::Connected(_),
                Input::Lost(lost),
            ) => self.lose(&lost),
            (Phase::BackingOff, Input::Drawn(random)) => self.wait(random),
            _ => Vec::new(),
        }
    }

    /// Starts a round: a dial now, and a hedge if it hasn't returned after
    /// `HEDGE_AFTER`.
    fn dial(&mut self) -> Vec<Effect> {
        let base = self.attempt();
        self.phase = Phase::Dialing(Round {
            base,
            base_dialing: true,
            hedge: None,
        });
        let shown = self.set_state(LinkState::Connecting);
        join([
            shown,
            Vec::from([
                Effect::Dial(base),
                Effect::Start(Operation::Sleep(HEDGE_AFTER)),
            ]),
        ])
    }

    const fn attempt(&mut self) -> Attempt {
        let attempt = Attempt(self.attempts);
        self.attempts = self.attempts.wrapping_add(1);
        attempt
    }

    fn hedge(&mut self, round: Round) -> Vec<Effect> {
        let fresh = self.attempt();
        self.phase = Phase::Dialing(Round {
            hedge: Some(fresh),
            ..round
        });
        Vec::from([Effect::Hedge(fresh)])
    }

    /// A dial of the round returned. The first to connect wins and the
    /// others are retired. A failure waits for the dial still running; the
    /// last one ends the round, on the endpoint of the last dial that had
    /// one.
    fn dialed(
        &mut self,
        round: Round,
        attempt: Attempt,
        outcome: Result<(), Unreached>,
    ) -> Vec<Effect> {
        let Err(unreached) = outcome else {
            let won = join([
                Vec::from([Effect::Adopt(attempt)]),
                round.others(attempt),
            ]);
            return join([won, self.greet()]);
        };
        match (attempt == round.base, round) {
            // The base's endpoint stays until the hedge returns.
            (true, Round { hedge: Some(_), .. }) => {
                self.phase = Phase::Dialing(Round {
                    base_dialing: false,
                    ..round
                });
                Vec::new()
            }
            (
                false,
                Round {
                    base_dialing: true, ..
                },
            ) => {
                self.phase = Phase::Dialing(Round {
                    hedge: None,
                    ..round
                });
                Vec::from([Effect::Retire(attempt)])
            }
            _ => self.missed(round, attempt, unreached),
        }
    }

    /// The last dial of the round failed too. The link runs on its
    /// endpoint, or on the base's when it had none.
    fn missed(
        &mut self,
        round: Round,
        attempt: Attempt,
        unreached: Unreached,
    ) -> Vec<Effect> {
        let (kept, reason) = match unreached {
            Unreached::Connect(reason) => (attempt, reason),
            Unreached::Unbound(reason) => (round.base, reason),
        };
        join([
            Vec::from([Effect::Adopt(kept)]),
            round.others(kept),
            self.down(&Lost::Connect(reason)),
        ])
    }

    fn greet(&mut self) -> Vec<Effect> {
        self.phase = Phase::Greeting;
        let shown = self.set_state(LinkState::Authenticating);
        join([
            shown,
            Vec::from([Effect::Start(Operation::Hello {
                hello: self.hello.clone(),
                within: HELLO_TIMEOUT,
            })]),
        ])
    }

    fn welcome(&mut self, welcome: Welcome) -> Vec<Effect> {
        let feed = Feed {
            epoch: welcome.epoch,
            live_at: welcome.head_seq,
        };
        let reset = self.adopt(feed.epoch);
        let shown = self.set_state(LinkState::CatchingUp);
        let after_seq = self.cursor_seq();
        let live = self.catch_up(after_seq, feed.live_at);
        self.phase = Phase::Subscribing(feed);
        join([
            reset,
            Vec::from([Effect::Forward(Update::Welcome(welcome))]),
            shown,
            live,
            Vec::from([Effect::Start(Operation::Subscribe(Subscribe {
                epoch: feed.epoch,
                after_seq,
            }))]),
        ])
    }

    /// Keeps the cursor if the host's log is the one it points into;
    /// otherwise starts over from the beginning of the new epoch.
    fn adopt(&mut self, epoch: Epoch) -> Vec<Effect> {
        match self.cursor {
            Some(cursor) if cursor.epoch == epoch => Vec::new(),
            Some(_) | None => {
                self.cursor = Some(Cursor {
                    epoch,
                    seq: HostSeq(0),
                });
                Vec::from([Effect::Forward(Update::Reset(epoch))])
            }
        }
    }

    fn apply(&mut self, feed: Feed, event: Event) -> Vec<Effect> {
        let after = self.cursor_seq();
        let seq = event.seq;
        let delta = is_delta(&event.kind);
        let forwarded =
            || Vec::from([Effect::Forward(Update::Feed(Frame::Event(event)))]);
        let order = after.0.checked_add(1).map(|next| seq.0.cmp(&next));
        match (order, delta) {
            (Some(Ordering::Less), true) => forwarded(),
            (Some(Ordering::Less), false) => Vec::new(),
            (Some(Ordering::Equal), false) => {
                self.cursor = Some(Cursor {
                    epoch: feed.epoch,
                    seq,
                });
                join([forwarded(), self.catch_up(seq, feed.live_at)])
            }
            (Some(Ordering::Equal), true)
            | (Some(Ordering::Greater) | None, _) => self.lose(&Lost::Gap {
                after: after.0,
                got: seq.0,
            }),
        }
    }

    /// Goes live once the link has applied the host's head as of `Welcome`.
    fn catch_up(&mut self, seq: HostSeq, live_at: HostSeq) -> Vec<Effect> {
        match (seq.cmp(&live_at), self.status.state) {
            (Ordering::Greater | Ordering::Equal, LinkState::CatchingUp) => {
                self.failures = 0;
                self.publish(Status {
                    state: LinkState::Live,
                    error: None,
                })
            }
            _ => Vec::new(),
        }
    }

    fn lose(&mut self, lost: &Lost) -> Vec<Effect> {
        join([Vec::from([Effect::Close]), self.down(lost)])
    }

    /// After a connection ended: report it, then back off.
    fn down(&mut self, lost: &Lost) -> Vec<Effect> {
        self.phase = Phase::BackingOff;
        join([
            self.disconnected(Some(format!("{lost}"))),
            Vec::from([Effect::Start(Operation::Draw)]),
        ])
    }

    fn wait(&mut self, random: u64) -> Vec<Effect> {
        self.phase = Phase::Waiting;
        let delay = backoff(self.failures, random);
        self.failures = self.failures.saturating_add(1);
        Vec::from([Effect::Start(Operation::Sleep(delay))])
    }

    fn cursor_seq(&self) -> HostSeq {
        self.cursor.map_or(HostSeq(0), |cursor| cursor.seq)
    }

    fn disconnected(&mut self, error: Option<String>) -> Vec<Effect> {
        self.publish(Status {
            state: LinkState::Disconnected,
            error,
        })
    }

    fn set_state(&mut self, state: LinkState) -> Vec<Effect> {
        self.publish(Status {
            state,
            ..self.status.clone()
        })
    }

    /// Shows `next` if it changed.
    fn publish(&mut self, next: Status) -> Vec<Effect> {
        if next == self.status {
            Vec::new()
        } else {
            self.status = next.clone();
            Vec::from([Effect::Show(next)])
        }
    }
}

fn join<const N: usize>(parts: [Vec<Effect>; N]) -> Vec<Effect> {
    parts.into_iter().flatten().collect()
}

/// Exponential from the floor to the ceiling, with the upper half jittered
/// by `random` so many clients don't retry in step.
fn backoff(failures: u32, random: u64) -> Duration {
    let capped = BACKOFF_FLOOR
        .saturating_mul(1u32.checked_shl(failures).unwrap_or(u32::MAX))
        .min(BACKOFF_CEILING);
    let half = capped.checked_div(2).unwrap_or(capped);
    let span = u64::try_from(half.as_millis())
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let jitter = Duration::from_millis(random.checked_rem(span).unwrap_or(0));
    half.saturating_add(jitter)
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use proto::{
        AuthToken, Availability, Catalog, ClientName, CommandResult, Epoch,
        Event, EventKind, Feed, Hello, HelloReply, HostSeq, InstanceId,
        InstanceStatus, Instances, Provider, SessionId, Subscribe, Welcome,
    };

    use super::{
        Attempt, Cursor, Effect, Input, Link, LinkState, Lost, Operation,
        Status, Stream, Unreached, Update, backoff,
    };

    fn hello() -> Hello {
        Hello {
            client: ClientName(String::from("test")),
            token: AuthToken::from_bytes([7; 16]),
        }
    }

    fn instances(reason: &str) -> Instances {
        Instances(
            Provider::ALL
                .map(|provider| {
                    (
                        InstanceId::own(provider),
                        InstanceStatus {
                            availability: Availability::Unavailable {
                                reason: String::from(reason),
                            },
                        },
                    )
                })
                .into(),
        )
    }

    fn welcome(epoch: u64, head: u64) -> Welcome {
        Welcome {
            host: String::from("host"),
            epoch: Epoch(epoch),
            head_seq: HostSeq(head),
            instances: instances("test"),
            catalog: Catalog::default(),
            home: None,
        }
    }

    fn event(seq: u64, kind: EventKind) -> Event {
        Event {
            seq: HostSeq(seq),
            session: SessionId(1),
            kind,
        }
    }

    fn stored(seq: u64) -> Event {
        event(seq, EventKind::SessionReady)
    }

    fn delta(seq: u64) -> Event {
        event(
            seq,
            EventKind::AssistantDelta {
                text: String::from("…"),
            },
        )
    }

    fn status(state: LinkState, error: Option<&str>) -> Status {
        Status {
            state,
            error: error.map(String::from),
        }
    }

    /// A round's first dial and its hedge timer.
    fn dial(attempt: u64) -> [Effect; 2] {
        [
            Effect::Dial(Attempt(attempt)),
            Effect::Start(Operation::Sleep(Duration::from_millis(1500))),
        ]
    }

    fn connected(attempt: u64) -> Input {
        Input::Dialed {
            attempt: Attempt(attempt),
            outcome: Ok(()),
        }
    }

    fn unreached(attempt: u64, why: Unreached) -> Input {
        Input::Dialed {
            attempt: Attempt(attempt),
            outcome: Err(why),
        }
    }

    fn no_route(attempt: u64) -> Input {
        unreached(attempt, Unreached::Connect(String::from("no route")))
    }

    fn greeting() -> Effect {
        Effect::Start(Operation::Hello {
            hello: hello(),
            within: Duration::from_secs(10),
        })
    }

    fn subscribe(epoch: u64, after: u64) -> Effect {
        Effect::Start(Operation::Subscribe(Subscribe {
            epoch: Epoch(epoch),
            after_seq: HostSeq(after),
        }))
    }

    fn walk(link: Link, inputs: Vec<Input>) -> (Link, Vec<Vec<Effect>>) {
        inputs
            .into_iter()
            .fold((link, Vec::new()), |(at, mut seen), input| {
                let (next, effects) = at.step(input);
                seen.push(effects);
                (next, seen)
            })
    }

    /// A link that is live on epoch 1 with seq 2 applied.
    fn live() -> Link {
        let (link, _) = Link::start(hello(), None);
        walk(
            link,
            Vec::from([
                connected(0),
                Input::Welcomed(welcome(1, 2)),
                Input::Subscribed,
                Input::Frame(Feed::Event(stored(1))),
                Input::Frame(Feed::Event(stored(2))),
            ]),
        )
        .0
    }

    fn failed_dials(count: usize, random: u64) -> Vec<Duration> {
        let (start, _) = Link::start(hello(), None);
        let rounds = (0..count).flat_map(|round| {
            [
                no_route(u64::try_from(round).unwrap_or(u64::MAX)),
                Input::Drawn(random),
                Input::Elapsed,
            ]
        });
        walk(start, rounds.collect())
            .1
            .into_iter()
            .skip(1)
            .step_by(3)
            .flatten()
            .filter_map(|effect| match effect {
                Effect::Start(Operation::Sleep(delay)) => Some(delay),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_link_walks_to_live_and_reconnects_after_the_last_applied_seq() {
        let (link, first) = Link::start(hello(), None);
        let (_, seen) = walk(
            link,
            Vec::from([
                connected(0),
                Input::Welcomed(welcome(1, 2)),
                Input::Subscribed,
                Input::Frame(Feed::Event(stored(1))),
                Input::Frame(Feed::Event(stored(2))),
                Input::Lost(Lost::Wire(String::from("reset"))),
                Input::Drawn(0),
                Input::Elapsed,
                connected(1),
                Input::Welcomed(welcome(1, 3)),
            ]),
        );
        let connecting = Status {
            state: LinkState::Connecting,
            ..Status::default()
        };
        let lost = status(LinkState::Disconnected, Some("reset"));
        assert_eq!(
            (first, seen),
            (
                Vec::from([
                    Effect::Show(connecting.clone()),
                    dial(0)[0].clone(),
                    dial(0)[1].clone(),
                ]),
                Vec::from([
                    Vec::from([
                        Effect::Adopt(Attempt(0)),
                        Effect::Show(Status {
                            state: LinkState::Authenticating,
                            ..connecting
                        }),
                        greeting(),
                    ]),
                    Vec::from([
                        Effect::Forward(Update::Reset(Epoch(1))),
                        Effect::Forward(Update::Welcome(welcome(1, 2))),
                        Effect::Show(status(LinkState::CatchingUp, None)),
                        subscribe(1, 0),
                    ]),
                    Vec::new(),
                    Vec::from([Effect::Forward(Update::Feed(Feed::Event(
                        stored(1)
                    )))]),
                    Vec::from([
                        Effect::Forward(Update::Feed(Feed::Event(stored(2)))),
                        Effect::Show(status(LinkState::Live, None)),
                    ]),
                    Vec::from([
                        Effect::Close,
                        Effect::Show(lost),
                        Effect::Start(Operation::Draw)
                    ]),
                    Vec::from([Effect::Start(Operation::Sleep(
                        Duration::from_millis(125)
                    ))]),
                    Vec::from([
                        Effect::Show(status(
                            LinkState::Connecting,
                            Some("reset")
                        )),
                        dial(1)[0].clone(),
                        dial(1)[1].clone(),
                    ]),
                    Vec::from([
                        Effect::Adopt(Attempt(1)),
                        Effect::Show(status(
                            LinkState::Authenticating,
                            Some("reset")
                        )),
                        greeting(),
                    ]),
                    Vec::from([
                        Effect::Forward(Update::Welcome(welcome(1, 3))),
                        Effect::Show(status(
                            LinkState::CatchingUp,
                            Some("reset")
                        )),
                        subscribe(1, 2),
                    ]),
                ]),
            )
        );
    }

    #[test]
    fn a_resumed_cursor_from_another_epoch_starts_over() {
        let resume = Cursor {
            epoch: Epoch(1),
            seq: HostSeq(9),
        };
        let (start, _) = Link::start(hello(), Some(resume));
        let (link, seen) = walk(
            start,
            Vec::from([connected(0), Input::Welcomed(welcome(2, 0))]),
        );
        assert_eq!(
            (seen.last(), link.cursor()),
            (
                Some(&Vec::from([
                    Effect::Forward(Update::Reset(Epoch(2))),
                    Effect::Forward(Update::Welcome(welcome(2, 0))),
                    Effect::Show(status(LinkState::CatchingUp, None)),
                    Effect::Show(status(LinkState::Live, None)),
                    subscribe(2, 0),
                ])),
                Some(Cursor {
                    epoch: Epoch(2),
                    seq: HostSeq(0),
                }),
            )
        );
    }

    #[test]
    fn a_resumed_cursor_from_the_same_epoch_subscribes_after_its_seq() {
        let resume = Cursor {
            epoch: Epoch(1),
            seq: HostSeq(4),
        };
        let (link, _) = Link::start(hello(), Some(resume));
        let (_, seen) = walk(
            link,
            Vec::from([connected(0), Input::Welcomed(welcome(1, 6))]),
        );
        assert_eq!(
            seen.last(),
            Some(&Vec::from([
                Effect::Forward(Update::Welcome(welcome(1, 6))),
                Effect::Show(status(LinkState::CatchingUp, None)),
                subscribe(1, 4),
            ]))
        );
    }

    #[test]
    fn repeats_are_dropped_deltas_pass_and_a_gap_drops_the_connection() {
        let (_, seen) = walk(
            live(),
            Vec::from([
                Input::Frame(Feed::Event(stored(2))),
                Input::Frame(Feed::Event(delta(2))),
                Input::Frame(Feed::Event(delta(1))),
                Input::Frame(Feed::Event(stored(4))),
            ]),
        );
        assert_eq!(
            seen,
            Vec::from([
                Vec::new(),
                Vec::from([Effect::Forward(Update::Feed(Feed::Event(delta(
                    2
                ))))]),
                Vec::from([Effect::Forward(Update::Feed(Feed::Event(delta(
                    1
                ))))]),
                Vec::from([
                    Effect::Close,
                    Effect::Show(status(
                        LinkState::Disconnected,
                        Some("event 4 arrived after 2: the feed has a gap"),
                    )),
                    Effect::Start(Operation::Draw),
                ]),
            ])
        );
    }

    #[test]
    fn a_delta_for_the_next_seq_is_a_gap() {
        let (_, seen) =
            walk(live(), Vec::from([Input::Frame(Feed::Event(delta(3)))]));
        assert_eq!(
            seen.first().and_then(|effects| effects.first()),
            Some(&Effect::Close)
        );
    }

    #[test]
    fn a_reconnect_closes_the_live_connection_and_dials_at_once() {
        let (_, seen) = walk(live(), Vec::from([Input::Reconnect]));
        assert_eq!(
            seen,
            Vec::from([Vec::from([
                Effect::Close,
                Effect::Show(status(LinkState::Disconnected, None)),
                Effect::Show(status(LinkState::Connecting, None)),
                dial(1)[0].clone(),
                dial(1)[1].clone(),
            ])])
        );
    }

    #[test]
    fn a_reconnect_while_backing_off_dials_without_waiting() {
        let (start, _) = Link::start(hello(), None);
        let (waiting, _) =
            walk(start, Vec::from([no_route(0), Input::Drawn(0)]));
        let took = waiting.takes_requests();
        let (dialing, effects) = waiting.step(Input::Reconnect);
        assert_eq!(
            (took, effects.get(1..), dialing.takes_requests()),
            (true, Some(&dial(1)[..]), false)
        );
    }

    #[test]
    fn every_round_dials_and_arms_its_hedge() {
        let (link, first) = Link::start(hello(), None);
        let (_, seen) = walk(
            link,
            Vec::from([no_route(0), Input::Drawn(0), Input::Elapsed]),
        );
        assert_eq!(
            [
                first.get(1..),
                seen.last().and_then(|effects| effects.get(1..))
            ],
            [Some(&dial(0)[..]), Some(&dial(1)[..])]
        );
    }

    fn hedged(inputs: Vec<Input>) -> Vec<Vec<Effect>> {
        let (link, _) = Link::start(hello(), None);
        let mut all = Vec::from([Input::Elapsed]);
        all.extend(inputs);
        walk(link, all).1
    }

    fn authenticating() -> Effect {
        Effect::Show(Status {
            state: LinkState::Authenticating,
            ..Status::default()
        })
    }

    fn stalled(error: &str) -> [Effect; 2] {
        [
            Effect::Show(Status {
                error: Some(format!("connect failed: {error}")),
                ..Status::default()
            }),
            Effect::Start(Operation::Draw),
        ]
    }

    #[test]
    fn a_stalled_dial_hedges_and_the_first_to_connect_wins() {
        assert_eq!(
            [
                hedged(Vec::from([connected(1), connected(0)])),
                hedged(Vec::from([connected(0), no_route(1)])),
            ],
            [
                Vec::from([
                    Vec::from([Effect::Hedge(Attempt(1))]),
                    Vec::from([
                        Effect::Adopt(Attempt(1)),
                        Effect::Retire(Attempt(0)),
                        authenticating(),
                        greeting(),
                    ]),
                    Vec::from([Effect::Retire(Attempt(0))]),
                ]),
                Vec::from([
                    Vec::from([Effect::Hedge(Attempt(1))]),
                    Vec::from([
                        Effect::Adopt(Attempt(0)),
                        Effect::Retire(Attempt(1)),
                        authenticating(),
                        greeting(),
                    ]),
                    Vec::from([Effect::Retire(Attempt(1))]),
                ]),
            ]
        );
    }

    #[test]
    fn a_failed_dial_waits_for_the_other() {
        let [shown, draw] = stalled("no route");
        assert_eq!(
            [
                hedged(Vec::from([no_route(0), connected(1)])),
                hedged(Vec::from([no_route(1), no_route(0)])),
                hedged(Vec::from([no_route(0), no_route(1)])),
            ],
            [
                Vec::from([
                    Vec::from([Effect::Hedge(Attempt(1))]),
                    Vec::new(),
                    Vec::from([
                        Effect::Adopt(Attempt(1)),
                        Effect::Retire(Attempt(0)),
                        authenticating(),
                        greeting(),
                    ]),
                ]),
                Vec::from([
                    Vec::from([Effect::Hedge(Attempt(1))]),
                    Vec::from([Effect::Retire(Attempt(1))]),
                    Vec::from([
                        Effect::Adopt(Attempt(0)),
                        shown.clone(),
                        draw.clone(),
                    ]),
                ]),
                Vec::from([
                    Vec::from([Effect::Hedge(Attempt(1))]),
                    Vec::new(),
                    Vec::from([
                        Effect::Adopt(Attempt(1)),
                        Effect::Retire(Attempt(0)),
                        shown,
                        draw,
                    ]),
                ]),
            ]
        );
    }

    #[test]
    fn a_hedge_with_no_endpoint_leaves_the_link_on_its_own() {
        let unbound =
            unreached(1, Unreached::Unbound(String::from("no network")));
        let [shown, draw] = stalled("no network");
        assert_eq!(
            [
                hedged(Vec::from([unbound.clone(), connected(0)])),
                hedged(Vec::from([no_route(0), unbound])),
            ],
            [
                Vec::from([
                    Vec::from([Effect::Hedge(Attempt(1))]),
                    Vec::from([Effect::Retire(Attempt(1))]),
                    Vec::from([
                        Effect::Adopt(Attempt(0)),
                        authenticating(),
                        greeting(),
                    ]),
                ]),
                Vec::from([
                    Vec::from([Effect::Hedge(Attempt(1))]),
                    Vec::new(),
                    Vec::from([
                        Effect::Adopt(Attempt(0)),
                        Effect::Retire(Attempt(1)),
                        shown,
                        draw,
                    ]),
                ]),
            ]
        );
    }

    #[test]
    fn hello_replies_and_feed_frames_become_inputs() {
        assert_eq!(
            Vec::from([
                Input::from(Some(HelloReply::Refused {
                    reason: String::from("bad token"),
                })),
                Input::from(None::<HelloReply>),
                Input::from(None::<Feed>),
            ]),
            Vec::from([
                Input::Lost(Lost::Refused(String::from("bad token"))),
                Input::Lost(Lost::HostClosed(Stream::Hello)),
                Input::Lost(Lost::HostClosed(Stream::Subscription)),
            ])
        );
    }

    #[test]
    fn a_failed_hello_closes_the_connection() {
        let (link, _) = Link::start(hello(), None);
        let (_, seen) = walk(
            link,
            Vec::from([connected(0), Input::Lost(Lost::HelloTimeout)]),
        );
        let lost = "the host did not complete the hello handshake within 10 \
                    seconds";
        assert_eq!(
            seen.last(),
            Some(&Vec::from([
                Effect::Close,
                Effect::Show(Status {
                    state: LinkState::Disconnected,
                    error: Some(String::from(lost)),
                }),
                Effect::Start(Operation::Draw),
            ]))
        );
    }

    #[test]
    fn answers_are_forwarded_while_connected() {
        let answer = CommandResult::Instances(instances("signed out"));
        let (link, effects) = live().step(Input::Answered(answer.clone()));
        let (down, _) =
            link.step(Input::Lost(Lost::HostClosed(Stream::Subscription)));
        let (_, stale) = down.step(Input::Answered(answer.clone()));
        assert_eq!(
            (effects, stale),
            (
                Vec::from([Effect::Forward(Update::Answer(answer))]),
                Vec::new(),
            )
        );
    }

    #[test]
    fn backoff_doubles_from_the_floor_to_the_ceiling() {
        assert_eq!(
            failed_dials(8, 0),
            [125, 250, 500, 1000, 2000, 4000, 5000, 5000]
                .map(Duration::from_millis)
        );
    }

    #[test]
    fn jitter_keeps_each_delay_in_the_upper_half_of_its_step() {
        let halves = failed_dials(8, 0);
        let within = [1, 77, 4999, u64::MAX].map(|random| {
            failed_dials(8, random).into_iter().zip(&halves).all(
                |(delay, half)| {
                    (*half..=half.saturating_mul(2)).contains(&delay)
                },
            )
        });
        assert_eq!(
            (within, failed_dials(1, 125)),
            ([true; 4], Vec::from([Duration::from_millis(250)]))
        );
    }

    #[test]
    fn backoff_doubles_to_ten_seconds_with_jitter_in_the_upper_half() {
        let ms = |failures, random| backoff(failures, random).as_millis();
        assert_eq!(
            Vec::from([
                ms(0, 0),
                ms(0, 125),
                ms(1, 0),
                ms(1, 250),
                ms(3, 0),
                ms(6, 0),
                ms(6, 5000),
                ms(40, 0),
            ]),
            Vec::from([125, 250, 250, 500, 1000, 5000, 10_000, 5000])
        );
    }

    #[test]
    fn a_stream_is_named_for_the_error_it_ends_with() {
        assert_eq!(
            [Stream::Hello, Stream::Subscription]
                .map(|stream| format!("{stream}")),
            [String::from("hello"), String::from("subscription")]
        );
    }

    #[test]
    fn a_dial_the_round_never_made_is_retired() {
        let (link, _) = Link::start(hello(), None);
        let (_, effects) = link.step(connected(7));
        assert_eq!(effects, Vec::from([Effect::Retire(Attempt(7))]));
    }

    #[test]
    fn going_live_clears_the_last_error() {
        let (link, _) = Link::start(hello(), None);
        let (live, _) = walk(
            link,
            Vec::from([
                no_route(0),
                Input::Drawn(1),
                Input::Elapsed,
                connected(1),
                Input::Welcomed(welcome(1, 0)),
            ]),
        );
        assert_eq!(live.status(), &status(LinkState::Live, None));
    }
}
