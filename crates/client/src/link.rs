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

use alloc::collections::BTreeMap;
use core::cmp::Ordering;
use core::fmt;
use core::time::Duration;

use crate::host_view::HostView;
use crate::prelude::*;
use proto::{
    Catalog, Epoch, Event, EventKind, Feed as Frame, Hello, HelloReply, Home,
    HostSeq, InstanceId, Instances, SignIn, SignInStep, Subscribe, Welcome,
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
    /// The host's name from its last `Welcome`.
    pub host: Option<String>,
    /// What the host can run, from its last `Welcome` or a later
    /// instance command's answer.
    pub instances: Option<Instances>,
    /// How each provider presents itself, from the last `Welcome`.
    pub catalog: Catalog,
    /// Where each instance's sign-in from the app stands, since the last
    /// `Welcome`. A cancelled one has none.
    pub sign_ins: BTreeMap<InstanceId, SignInStep>,
    /// The host's home and its projects, from the last `Welcome` or a later
    /// `Feed::Home`. `None` on a host given no home.
    pub home: Option<Home>,
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
    Event(Event),
}

/// What the app shows, its link status and host view, once `update`
/// arrives.
#[must_use]
pub fn shown_after(
    status: Option<Status>,
    view: HostView,
    update: Update,
) -> (Option<Status>, HostView) {
    match update {
        Update::Status(next) => (Some(next), view),
        Update::Reset(_) => (status, HostView::empty()),
        Update::Event(event) => (status, view.apply(event)),
    }
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
    Event(Event),
    SignIn(SignIn),
    /// The host's home changed to this.
    Home(Home),
    Lost(Lost),
    /// The random number [`Operation::Draw`] asked for.
    Drawn(u64),
    /// The timer of [`Operation::Sleep`] fired.
    Elapsed,
    /// Asked to drop the connection and connect again at once, as after a
    /// network change.
    Reconnect,
    /// What the host runs now, from a command's answer.
    Instances(Instances),
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
        match frame {
            Some(Frame::Event(event)) => Self::Event(event),
            Some(Frame::SignIn(sign_in)) => Self::SignIn(sign_in),
            Some(Frame::Home(home)) => Self::Home(home),
            None => Self::Lost(Lost::HostClosed(Stream::Subscription)),
        }
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
            (_, Input::Instances(instances)) => self.publish(Status {
                instances: Some(instances),
                ..self.status.clone()
            }),
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
            (Phase::Connected(feed), Input::Event(event)) => {
                self.apply(feed, event)
            }
            (Phase::Connected(_), Input::SignIn(sign_in)) => {
                self.signs_in(sign_in)
            }
            (Phase::Connected(_), Input::Home(home)) => self.publish(Status {
                home: Some(home),
                ..self.status.clone()
            }),
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
        // The subscription starts with the sign-ins running now.
        let shown = self.publish(Status {
            host: Some(welcome.host),
            instances: Some(welcome.instances),
            catalog: welcome.catalog,
            sign_ins: BTreeMap::new(),
            home: welcome.home,
            state: LinkState::CatchingUp,
            ..self.status.clone()
        });
        let after_seq = self.cursor_seq();
        let live = self.catch_up(after_seq, feed.live_at);
        self.phase = Phase::Subscribing(feed);
        join([
            reset,
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
        let order = after.0.checked_add(1).map(|next| event.seq.0.cmp(&next));
        let delta = matches!(
            event.kind,
            EventKind::AssistantDelta { .. } | EventKind::ReasoningDelta { .. }
        );
        match (order, delta) {
            // A delta repeats the seq of the stored event before it, which
            // the stream delivered first (`EventKind::AssistantDelta`).
            (Some(Ordering::Less), true) => {
                Vec::from([Effect::Forward(Update::Event(event))])
            }
            (Some(Ordering::Less), false) => Vec::new(),
            (Some(Ordering::Equal), true)
            | (Some(Ordering::Greater) | None, _) => self.lose(&Lost::Gap {
                after: after.0,
                got: event.seq.0,
            }),
            (Some(Ordering::Equal), false) => {
                let seq = event.seq;
                self.cursor = Some(Cursor {
                    epoch: feed.epoch,
                    seq,
                });
                let live = self.catch_up(seq, feed.live_at);
                join([Vec::from([Effect::Forward(Update::Event(event))]), live])
            }
        }
    }

    /// Shows the sign-in's step. A signed-in one brings what the host runs
    /// now, and a cancelled one leaves none.
    fn signs_in(&mut self, sign_in: SignIn) -> Vec<Effect> {
        let SignIn { instance, step } = sign_in;
        let mut next = self.status.clone();
        if let SignInStep::SignedIn { instances } = &step {
            next.instances = Some(instances.clone());
        }
        if step == SignInStep::Cancelled {
            next.sign_ins.remove(&instance);
        } else {
            next.sign_ins.insert(instance, step);
        }
        self.publish(next)
    }

    /// Goes live once the link has applied the host's head as of `Welcome`.
    fn catch_up(&mut self, seq: HostSeq, live_at: HostSeq) -> Vec<Effect> {
        match (seq.cmp(&live_at), self.status.state) {
            (Ordering::Greater | Ordering::Equal, LinkState::CatchingUp) => {
                self.failures = 0;
                self.publish(Status {
                    state: LinkState::Live,
                    error: None,
                    ..self.status.clone()
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
            ..self.status.clone()
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
    use alloc::collections::BTreeMap;
    use core::time::Duration;

    use proto::{
        AuthToken, Availability, Catalog, ClientName, Epoch, Event, EventKind,
        Feed, Hello, HelloReply, HostSeq, InstanceId, InstanceStatus,
        Instances, Provider, SessionId, SignIn, SignInStep, Subscribe, Welcome,
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
            host: Some(String::from("host")),
            instances: Some(instances("test")),
            catalog: Catalog::default(),
            sign_ins: BTreeMap::new(),
            home: None,
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
                Input::Event(stored(1)),
                Input::Event(stored(2)),
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
                Input::Event(stored(1)),
                Input::Event(stored(2)),
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
                        Effect::Show(status(LinkState::CatchingUp, None)),
                        subscribe(1, 0),
                    ]),
                    Vec::new(),
                    Vec::from([Effect::Forward(Update::Event(stored(1)))]),
                    Vec::from([
                        Effect::Forward(Update::Event(stored(2))),
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
                Input::Event(stored(2)),
                Input::Event(delta(2)),
                Input::Event(stored(4)),
            ]),
        );
        assert_eq!(
            seen,
            Vec::from([
                Vec::new(),
                Vec::from([Effect::Forward(Update::Event(delta(2)))]),
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
        let (_, seen) = walk(live(), Vec::from([Input::Event(delta(3))]));
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
                    ..Status::default()
                }),
                Effect::Start(Operation::Draw),
            ]))
        );
    }

    #[test]
    fn instances_update_the_status_and_stale_inputs_do_nothing() {
        let (link, adopted) =
            live().step(Input::Instances(instances("signed out")));
        let (_, stale) =
            walk(link, Vec::from([Input::Elapsed, Input::Drawn(1)]));
        assert_eq!(
            (adopted, stale),
            (
                Vec::from([Effect::Show(Status {
                    instances: Some(instances("signed out")),
                    ..status(LinkState::Live, None)
                })]),
                Vec::from([Vec::new(), Vec::new()]),
            )
        );
    }

    #[test]
    fn the_home_comes_with_the_welcome_and_each_frame_replaces_it() {
        let home = |sync| proto::Home {
            remote: String::from("file:///home.git"),
            synced: Some(5),
            sync,
            projects: Vec::new(),
        };
        let (link, _) = Link::start(hello(), None);
        let (welcomed, shown) =
            link.step(connected(0)).0.step(Input::Welcomed(Welcome {
                home: Some(home(proto::HomeSync::Syncing)),
                ..welcome(1, 0)
            }));
        let (subscribed, _) = welcomed.step(Input::Subscribed);
        let (_, changed) = subscribed.step(Input::from(Some(
            proto::Feed::Home(home(proto::HomeSync::Synced)),
        )));
        let homes = |effects: &[Effect]| {
            effects
                .iter()
                .filter_map(|effect| match effect {
                    Effect::Show(status) => status.home.clone(),
                    _ => None,
                })
                .map(|known| known.sync)
                .collect::<Vec<proto::HomeSync>>()
        };
        assert_eq!(
            (homes(&shown), homes(&changed)),
            (
                Vec::from([proto::HomeSync::Syncing, proto::HomeSync::Syncing]),
                Vec::from([proto::HomeSync::Synced]),
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
    fn sign_ins_show_in_the_status_until_cancelled_or_the_next_welcome() {
        let id = || InstanceId::own(Provider::Claude);
        let frame = |step| {
            Input::from(Some(Feed::SignIn(SignIn {
                instance: id(),
                step,
            })))
        };
        let open = SignInStep::Open {
            url: String::from("https://sign.example/in"),
            code: None,
            paste: true,
        };
        let signed_in = SignInStep::SignedIn {
            instances: instances("signed in"),
        };
        let (_, seen) = walk(
            live(),
            Vec::from([
                frame(open.clone()),
                frame(signed_in.clone()),
                frame(SignInStep::Cancelled),
                frame(open.clone()),
                Input::Lost(Lost::Wire(String::from("reset"))),
                Input::Drawn(0),
                Input::Elapsed,
                connected(1),
                Input::Welcomed(welcome(1, 2)),
            ]),
        );
        let shown = |instances_reason: &str, sign_ins: &[SignInStep]| Status {
            instances: Some(instances(instances_reason)),
            sign_ins: sign_ins
                .iter()
                .map(|step| (id(), step.clone()))
                .collect(),
            ..status(LinkState::Live, None)
        };
        let show = |status| Some(Vec::from([Effect::Show(status)]));
        assert_eq!(
            (
                seen.first().cloned(),
                seen.get(1).cloned(),
                seen.get(2).cloned(),
                seen.get(3).cloned(),
                seen.get(8).and_then(|effects| effects.first().cloned()),
            ),
            (
                show(shown("test", core::slice::from_ref(&open))),
                show(shown("signed in", &[signed_in])),
                show(shown("signed in", &[])),
                show(shown("signed in", &[open])),
                Some(Effect::Show(Status {
                    state: LinkState::CatchingUp,
                    error: Some(String::from("reset")),
                    ..shown("test", &[])
                })),
            )
        );
    }
}
