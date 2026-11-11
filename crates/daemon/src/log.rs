//! The host's event log. One task owns it and assigns every event its seq;
//! [`Log`] handles talk to it over a channel, so nothing is shared behind a
//! lock.

use std::convert::Infallible;

use proto::{Epoch, Event, EventKind, HostSeq, SessionId, Welcome};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::Stopped;

/// A subscriber this far behind the live feed is dropped and resubscribes
/// from its cursor (architecture §8).
const LIVE_BUFFER: usize = 1024;

/// What a new subscriber needs: the log after its cursor, and the live feed,
/// attached in the same actor turn as the replay, so nothing falls between.
#[derive(Debug)]
pub struct Subscription {
    pub replay: Vec<Event>,
    pub live: broadcast::Receiver<Event>,
}

#[derive(Debug, Clone)]
pub struct Log {
    inbox: mpsc::Sender<LogMsg>,
}

enum LogMsg {
    Append {
        session: SessionId,
        kind: EventKind,
    },
    Welcome {
        reply: oneshot::Sender<Welcome>,
    },
    Subscribe {
        after: HostSeq,
        reply: oneshot::Sender<Subscription>,
    },
}

struct LogActor {
    name: String,
    epoch: Epoch,
    events: Vec<Event>,
    live: broadcast::Sender<Event>,
}

impl Log {
    pub fn spawn(name: String, epoch: Epoch) -> Log {
        let (inbox, messages) = mpsc::channel(64);
        let (live, _) = broadcast::channel(LIVE_BUFFER);
        let actor = LogActor {
            name,
            epoch,
            events: Vec::new(),
            live,
        };
        tokio::spawn(actor.run(messages));
        Log { inbox }
    }

    /// Fire and forget: the log assigns the seq. Events of one session stay
    /// in order because one task sends them, through one channel.
    pub async fn append(&self, session: SessionId, kind: EventKind) -> Result<(), Stopped> {
        self.inbox
            .send(LogMsg::Append { session, kind })
            .await
            .map_err(|_| Stopped {})
    }

    pub async fn welcome(&self) -> Result<Welcome, Stopped> {
        self.ask(|reply| LogMsg::Welcome { reply }).await
    }

    pub async fn subscribe(&self, after: HostSeq) -> Result<Subscription, Stopped> {
        self.ask(|reply| LogMsg::Subscribe { after, reply }).await
    }

    async fn ask<T>(&self, build: impl FnOnce(oneshot::Sender<T>) -> LogMsg) -> Result<T, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.inbox
            .send(build(reply))
            .await
            .map_err(|_| Stopped {})?;
        answer.await.map_err(|_| Stopped {})
    }
}

impl LogActor {
    async fn run(mut self, mut messages: mpsc::Receiver<LogMsg>) -> Result<Infallible, Stopped> {
        loop {
            let msg = messages.recv().await.ok_or(Stopped {})?;
            self.on(msg);
        }
    }

    fn on(&mut self, msg: LogMsg) {
        match msg {
            LogMsg::Append { session, kind } => {
                let event = Event {
                    seq: self.head().next(),
                    session,
                    kind,
                };
                self.events.push(event.clone());
                // No receivers is fine: nobody is subscribed yet.
                let _ = self.live.send(event);
            }
            LogMsg::Welcome { reply } => {
                let _ = reply.send(Welcome {
                    host: self.name.clone(),
                    epoch: self.epoch,
                    head_seq: self.head(),
                });
            }
            LogMsg::Subscribe { after, reply } => {
                let _ = reply.send(Subscription {
                    live: self.live.subscribe(),
                    replay: self
                        .events
                        .iter()
                        .filter(|event| event.seq > after)
                        .cloned()
                        .collect(),
                });
            }
        }
    }

    fn head(&self) -> HostSeq {
        self.events.last().map_or(HostSeq(0), |event| event.seq)
    }
}

#[cfg(test)]
mod tests {
    use proto::{Epoch, EventKind, HostSeq, SessionId, Welcome};

    use super::Log;

    fn delta(text: &str) -> EventKind {
        EventKind::AssistantDelta {
            text: String::from(text),
        }
    }

    #[tokio::test]
    async fn replay_starts_after_the_cursor_and_welcome_reports_the_head() {
        let log = Log::spawn(String::from("test"), Epoch(5));
        log.append(SessionId(1), EventKind::SessionReady)
            .await
            .unwrap();
        log.append(SessionId(1), delta("a")).await.unwrap();
        log.append(SessionId(1), delta("b")).await.unwrap();
        assert_eq!(
            log.welcome().await.unwrap(),
            Welcome {
                host: String::from("test"),
                epoch: Epoch(5),
                head_seq: HostSeq(3),
            }
        );
        let replay = log.subscribe(HostSeq(1)).await.unwrap().replay;
        assert_eq!(
            replay
                .into_iter()
                .map(|event| (event.seq, event.kind))
                .collect::<Vec<(HostSeq, EventKind)>>(),
            [(HostSeq(2), delta("a")), (HostSeq(3), delta("b"))]
        );
    }
}
