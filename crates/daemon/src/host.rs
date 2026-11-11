//! The host's event log and its simulated agent. One task owns the log, the
//! running turn and the open approvals; [`Host`] handles talk to it over a
//! channel, so nothing is shared behind a lock.

use std::collections::HashMap;
use std::time::Duration;

use proto::{
    CommandBody, CommandResult, Decision, Epoch, Event, EventKind, HostSeq, Rejection, RequestId,
    Welcome,
};
use tokio::sync::{broadcast, mpsc, oneshot};

/// A subscriber this far behind the live feed is dropped and resubscribes
/// from its cursor (§8).
const LIVE_BUFFER: usize = 1024;

#[derive(Debug, Clone)]
pub struct HostOptions {
    pub name: String,
    pub epoch: Epoch,
    /// Pause between the simulated agent's transcript chunks.
    pub delta_interval: Duration,
}

#[derive(Debug, thiserror::Error)]
#[error("the host task has stopped")]
pub struct Stopped;

/// What a new subscriber needs: the log after its cursor, and the live feed,
/// attached before the replay was taken so nothing falls between them.
#[derive(Debug)]
pub struct Subscription {
    pub replay: Vec<Event>,
    pub live: broadcast::Receiver<Event>,
}

#[derive(Debug, Clone)]
pub struct Host {
    messages: mpsc::Sender<Msg>,
}

#[derive(Debug)]
enum Msg {
    Welcome {
        reply: oneshot::Sender<Welcome>,
    },
    Subscribe {
        after_seq: HostSeq,
        reply: oneshot::Sender<Subscription>,
    },
    Command {
        body: CommandBody,
        by: String,
        reply: oneshot::Sender<CommandResult>,
    },
    /// From the simulated turn.
    Emit(EventKind),
    OpenApproval {
        tool: String,
        detail: String,
        decided: oneshot::Sender<Decision>,
    },
}

enum Approval {
    Open(oneshot::Sender<Decision>),
    Resolved { by: String },
}

impl Host {
    pub fn spawn(opts: HostOptions) -> Self {
        let (messages, inbox) = mpsc::channel(64);
        let (live, _) = broadcast::channel(LIVE_BUFFER);
        let actor = Actor {
            opts,
            log: Vec::new(),
            live,
            turn_running: false,
            approvals: HashMap::new(),
            next_request: 1,
        };
        let host = Self { messages };
        tokio::spawn(actor.run(inbox, host.clone()));
        host
    }

    pub async fn welcome(&self) -> Result<Welcome, Stopped> {
        self.ask(|reply| Msg::Welcome { reply }).await
    }

    pub async fn subscribe(&self, after_seq: HostSeq) -> Result<Subscription, Stopped> {
        self.ask(|reply| Msg::Subscribe { after_seq, reply }).await
    }

    /// `by` names the client, for `ApprovalResolved`.
    pub async fn command(&self, body: CommandBody, by: String) -> Result<CommandResult, Stopped> {
        self.ask(|reply| Msg::Command { body, by, reply }).await
    }

    async fn emit(&self, kind: EventKind) -> Result<(), Stopped> {
        self.messages
            .send(Msg::Emit(kind))
            .await
            .map_err(|_| Stopped)
    }

    async fn ask<T>(&self, msg: impl FnOnce(oneshot::Sender<T>) -> Msg) -> Result<T, Stopped> {
        let (reply, answer) = oneshot::channel();
        self.messages.send(msg(reply)).await.map_err(|_| Stopped)?;
        answer.await.map_err(|_| Stopped)
    }
}

struct Actor {
    opts: HostOptions,
    log: Vec<Event>,
    live: broadcast::Sender<Event>,
    turn_running: bool,
    approvals: HashMap<RequestId, Approval>,
    next_request: u64,
}

impl Actor {
    /// Holds a clone of its own handle to give to simulated turns, so it runs
    /// until the process exits.
    async fn run(mut self, mut inbox: mpsc::Receiver<Msg>, host: Host) {
        while let Some(msg) = inbox.recv().await {
            match msg {
                Msg::Welcome { reply } => {
                    reply.send(self.welcome()).ok();
                }
                Msg::Subscribe { after_seq, reply } => {
                    let live = self.live.subscribe();
                    let replay = self
                        .log
                        .iter()
                        .filter(|e| e.seq.0 > after_seq.0)
                        .cloned()
                        .collect();
                    reply.send(Subscription { replay, live }).ok();
                }
                Msg::Command { body, by, reply } => {
                    reply.send(self.command(body, by, &host)).ok();
                }
                Msg::Emit(kind) => {
                    if kind == EventKind::TurnFinished {
                        self.turn_running = false;
                    }
                    self.append(kind);
                }
                Msg::OpenApproval {
                    tool,
                    detail,
                    decided,
                } => {
                    let id = RequestId(self.next_request);
                    self.next_request += 1;
                    self.approvals.insert(id, Approval::Open(decided));
                    self.append(EventKind::ApprovalRequested { id, tool, detail });
                }
            }
        }
    }

    fn welcome(&self) -> Welcome {
        Welcome {
            host: self.opts.name.clone(),
            epoch: self.opts.epoch,
            head_seq: self.head(),
        }
    }

    fn head(&self) -> HostSeq {
        self.log.last().map_or(HostSeq(0), |e| e.seq)
    }

    fn append(&mut self, kind: EventKind) {
        let event = Event {
            seq: HostSeq(self.head().0 + 1),
            kind,
        };
        self.log.push(event.clone());
        // No receivers is fine: nobody is subscribed yet.
        self.live.send(event).ok();
    }

    fn command(&mut self, body: CommandBody, by: String, host: &Host) -> CommandResult {
        match body {
            CommandBody::Prompt { text } => {
                if self.turn_running {
                    return CommandResult::Rejected(Rejection::TurnInProgress);
                }
                self.turn_running = true;
                tokio::spawn(simulate_turn(host.clone(), text, self.opts.delta_interval));
                CommandResult::Accepted
            }
            CommandBody::Resolve { approval, decision } => {
                let Some(slot) = self.approvals.get_mut(&approval) else {
                    return CommandResult::Rejected(Rejection::UnknownApproval);
                };
                let open = match std::mem::replace(slot, Approval::Resolved { by: by.clone() }) {
                    Approval::Open(open) => open,
                    Approval::Resolved { by: first } => {
                        *slot = Approval::Resolved { by: first.clone() };
                        return CommandResult::Rejected(Rejection::AlreadyResolved { by: first });
                    }
                };
                self.append(EventKind::ApprovalResolved {
                    id: approval,
                    decision,
                    by,
                });
                open.send(decision).ok();
                CommandResult::Accepted
            }
        }
    }
}

/// Stands in for a real agent turn: stream a reply, ask for one approval,
/// then finish according to the decision.
async fn simulate_turn(host: Host, prompt: String, interval: Duration) {
    let result: Result<(), Stopped> = async {
        let reply = format!("Looking into \"{prompt}\". I need to run one command to answer.");
        for chunk in reply.split_inclusive(' ') {
            tokio::time::sleep(interval).await;
            host.emit(EventKind::TranscriptDelta { text: chunk.into() })
                .await?;
        }
        let (decided, decision) = oneshot::channel();
        host.messages
            .send(Msg::OpenApproval {
                tool: "Bash".into(),
                detail: format!("echo {prompt:?}"),
                decided,
            })
            .await
            .map_err(|_| Stopped)?;
        let text = match decision.await.map_err(|_| Stopped)? {
            Decision::Approve => format!("Ran it. The answer to \"{prompt}\" is 42."),
            Decision::Deny => "Understood, I did not run the command.".to_string(),
        };
        host.emit(EventKind::AssistantMessage { text }).await?;
        host.emit(EventKind::TurnFinished).await
    }
    .await;
    if result.is_err() {
        tracing::warn!("simulated turn stopped early: the host task is gone");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Host {
        Host::spawn(HostOptions {
            name: "test".into(),
            epoch: Epoch(5),
            delta_interval: Duration::ZERO,
        })
    }

    async fn next(live: &mut broadcast::Receiver<Event>) -> Event {
        live.recv().await.unwrap()
    }

    async fn prompt_until_approval(
        host: &Host,
        live: &mut broadcast::Receiver<Event>,
    ) -> RequestId {
        let prompt = CommandBody::Prompt { text: "hi".into() };
        assert_eq!(
            host.command(prompt, "a".into()).await.unwrap(),
            CommandResult::Accepted
        );
        loop {
            if let EventKind::ApprovalRequested { id, .. } = next(live).await.kind {
                return id;
            }
        }
    }

    #[tokio::test]
    async fn turn_streams_asks_and_finishes_on_approval() {
        let host = host();
        let mut live = host.subscribe(HostSeq(0)).await.unwrap().live;
        let id = prompt_until_approval(&host, &mut live).await;
        let resolve = CommandBody::Resolve {
            approval: id,
            decision: Decision::Approve,
        };
        assert_eq!(
            host.command(resolve, "phone".into()).await.unwrap(),
            CommandResult::Accepted
        );
        let mut tail = Vec::new();
        for _ in 0..3 {
            tail.push(next(&mut live).await.kind);
        }
        assert_eq!(
            tail,
            [
                EventKind::ApprovalResolved {
                    id,
                    decision: Decision::Approve,
                    by: "phone".into()
                },
                EventKind::AssistantMessage {
                    text: "Ran it. The answer to \"hi\" is 42.".into()
                },
                EventKind::TurnFinished,
            ]
        );
        let replay = host.subscribe(HostSeq(0)).await.unwrap().replay;
        let text: String = replay
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::TranscriptDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            text,
            "Looking into \"hi\". I need to run one command to answer."
        );
        let seqs: Vec<u64> = replay.iter().map(|e| e.seq.0).collect();
        assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn first_decision_wins_and_later_ones_name_the_winner() {
        let host = host();
        let mut live = host.subscribe(HostSeq(0)).await.unwrap().live;
        let id = prompt_until_approval(&host, &mut live).await;
        let resolve = |decision| CommandBody::Resolve {
            approval: id,
            decision,
        };
        assert_eq!(
            host.command(resolve(Decision::Deny), "desk".into())
                .await
                .unwrap(),
            CommandResult::Accepted
        );
        assert_eq!(
            host.command(resolve(Decision::Approve), "phone".into())
                .await
                .unwrap(),
            CommandResult::Rejected(Rejection::AlreadyResolved { by: "desk".into() })
        );
        assert_eq!(
            host.command(
                CommandBody::Resolve {
                    approval: RequestId(99),
                    decision: Decision::Approve
                },
                "phone".into()
            )
            .await
            .unwrap(),
            CommandResult::Rejected(Rejection::UnknownApproval)
        );
    }

    #[tokio::test]
    async fn a_second_prompt_during_a_turn_is_rejected() {
        let host = host();
        let mut live = host.subscribe(HostSeq(0)).await.unwrap().live;
        prompt_until_approval(&host, &mut live).await;
        assert_eq!(
            host.command(
                CommandBody::Prompt {
                    text: "again".into()
                },
                "a".into()
            )
            .await
            .unwrap(),
            CommandResult::Rejected(Rejection::TurnInProgress)
        );
    }

    #[tokio::test]
    async fn replay_starts_after_the_cursor_and_welcome_reports_the_head() {
        let host = host();
        let mut live = host.subscribe(HostSeq(0)).await.unwrap().live;
        prompt_until_approval(&host, &mut live).await;
        let welcome = host.welcome().await.unwrap();
        assert_eq!(welcome.epoch, Epoch(5));
        let head = welcome.head_seq.0;
        let replay = host.subscribe(HostSeq(head - 2)).await.unwrap().replay;
        let seqs: Vec<u64> = replay.iter().map(|e| e.seq.0).collect();
        assert_eq!(seqs, [head - 1, head]);
    }
}
