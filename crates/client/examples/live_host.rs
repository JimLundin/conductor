//! Drives a real host through three Claude turns and prints every update:
//! an approved file write, an interrupt on the first delta, and an interrupt
//! sent with the message, before the turn starts. `tools/live-host.sh` runs it.
//!
//! `cargo run -p client --example live_host -- <ticket> --directory DIR`

use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Parser;
use client::{HostLink, LinkOptions, LinkState, Status, Target, Update};
use proto::{
    ClientName, CommandBody, CommandResult, Decision, Event, EventKind, HostPath, Provider,
    SessionId, Ticket, TurnOutcome,
};
use tokio::sync::mpsc;

const WRITE: &str = "Use the Write tool to create a.txt in the current directory containing exactly the word hello. Then say done.";
const COUNT: &str = "Count from 1 to 300, one number per line, with no other text.";

#[derive(Parser)]
struct Args {
    #[arg(value_parser = Ticket::parse)]
    ticket: Ticket,
    /// The session's directory on the host.
    #[arg(long)]
    directory: String,
    #[arg(long, default_value_t = 180)]
    timeout_secs: u64,
}

/// What the driver does during a turn.
#[derive(Debug, Clone, Copy)]
enum Plan {
    Approve,
    InterruptOnFirstDelta,
    InterruptAtSend,
}

/// What one turn did, as the client saw it.
#[derive(Debug, Clone)]
struct Tally {
    deltas: usize,
    approved: usize,
    interrupt: Option<CommandResult>,
    outcome: Option<TurnOutcome>,
}

const FRESH: Tally = Tally {
    deltas: 0,
    approved: 0,
    interrupt: None,
    outcome: None,
};

enum Flow {
    Next,
    Ended,
}

#[derive(Debug, Clone, Copy)]
enum Verdict {
    Pass,
    Fail,
}

const PLANS: &[Plan] = &[
    Plan::Approve,
    Plan::InterruptOnFirstDelta,
    Plan::InterruptAtSend,
];

/// The driver's loop state: the turn in progress and the finished ones.
struct Live {
    start: Instant,
    link: HostLink,
    updates: mpsc::Receiver<Update>,
    session: SessionId,
    plan: Plan,
    tally: Tally,
    done: Vec<Tally>,
}

struct Ended {
    link: HostLink,
    result: anyhow::Result<Vec<Tally>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    let args = Args::parse();
    let deadline = Duration::from_secs(args.timeout_secs);
    tokio::time::timeout(deadline, run(args))
        .await
        .context("timed out")?
}

async fn run(args: Args) -> anyhow::Result<()> {
    let start = Instant::now();
    let (link, mut updates) = HostLink::spawn(
        Target::of_ticket(&args.ticket),
        LinkOptions::new(ClientName(String::from("live"))),
    )
    .await?;
    until_live(start, &mut updates).await?;
    let started = link
        .send(CommandBody::StartSession {
            provider: Provider::Claude,
            directory: HostPath(args.directory),
        })
        .await?;
    log(start, format!("start -> {started:?}"))?;
    let session = match started {
        CommandResult::Started { session } => Ok(session),
        CommandResult::Accepted | CommandResult::Rejected(_) => {
            Err(anyhow::Error::msg("the host did not start a session"))
        }
    }?;
    let interrupt = send_turn(&link, start, session, Plan::Approve).await?;
    let live = Live {
        start,
        link,
        updates,
        session,
        plan: Plan::Approve,
        tally: Tally { interrupt, ..FRESH },
        done: Vec::new(),
    };
    let ended = live.run().await;
    ended.link.stop().await?;
    let done = ended.result?;
    let checks = PLANS
        .iter()
        .copied()
        .zip(done.iter())
        .map(|(plan, tally)| (plan, verdict(plan, tally)))
        .collect::<Vec<(Plan, Verdict)>>();
    checks
        .iter()
        .try_for_each(|(plan, verdict)| log(start, format!("check {verdict:?}: {plan:?}")))?;
    match checks
        .iter()
        .find(|(_, verdict)| matches!(verdict, Verdict::Fail))
        .map(|(plan, _)| plan)
        .copied()
        .or(PLANS.get(done.len()).copied())
    {
        Some(plan) => Err(anyhow::Error::msg(format!("failed: {plan:?}"))),
        None => Ok(()),
    }
}

/// Turn 1 streams, is approved and completes. Turn 2 is interrupted after
/// its first delta. Turn 3 is interrupted at send.
fn verdict(plan: Plan, tally: &Tally) -> Verdict {
    let passed = match plan {
        Plan::Approve => Some(tally).filter(|tally| {
            tally.deltas > 0 && tally.approved > 0 && tally.outcome == Some(TurnOutcome::Completed)
        }),
        Plan::InterruptOnFirstDelta => Some(tally).filter(|tally| {
            tally.deltas > 0
                && tally.interrupt == Some(CommandResult::Accepted)
                && tally.outcome == Some(TurnOutcome::Interrupted)
        }),
        Plan::InterruptAtSend => Some(tally).filter(|tally| {
            tally.interrupt == Some(CommandResult::Accepted)
                && tally.outcome == Some(TurnOutcome::Interrupted)
        }),
    };
    passed.map_or(Verdict::Fail, |_| Verdict::Pass)
}

/// Sends the plan's message, and the interrupt that goes with it at once.
async fn send_turn(
    link: &HostLink,
    start: Instant,
    session: SessionId,
    plan: Plan,
) -> anyhow::Result<Option<CommandResult>> {
    let text = match plan {
        Plan::Approve => WRITE,
        Plan::InterruptOnFirstDelta | Plan::InterruptAtSend => COUNT,
    };
    let sent = link
        .send(CommandBody::SendMessage {
            session,
            text: String::from(text),
        })
        .await?;
    log(start, format!("message {text:?} -> {sent:?}"))?;
    match plan {
        Plan::InterruptAtSend => Ok(Some(interrupt(link, start, session).await?)),
        Plan::Approve | Plan::InterruptOnFirstDelta => Ok(None),
    }
}

async fn interrupt(
    link: &HostLink,
    start: Instant,
    session: SessionId,
) -> anyhow::Result<CommandResult> {
    let result = link.send(CommandBody::Interrupt { session }).await?;
    log(start, format!("interrupt -> {result:?}"))?;
    Ok(result)
}

async fn until_live(start: Instant, updates: &mut mpsc::Receiver<Update>) -> anyhow::Result<()> {
    loop {
        match updates.recv().await {
            Some(Update::Status(Status {
                state: LinkState::Live,
                path,
                ..
            })) => break log(start, format!("live path={path:?}")).map_err(anyhow::Error::from),
            Some(update) => log(start, format!("{update:?}"))?,
            None => break Err(anyhow::Error::msg("the link stopped")),
        }
    }
}

fn log(start: Instant, line: String) -> std::io::Result<()> {
    writeln!(
        std::io::stdout(),
        "[{:7.3}s] {line}",
        start.elapsed().as_secs_f64()
    )
}

impl Live {
    /// Hands the link back however the turns ended, so it can be stopped.
    async fn run(mut self) -> Ended {
        loop {
            let update = self.updates.recv().await;
            match self.on(update).await {
                Ok(Flow::Next) => {}
                Ok(Flow::Ended) => {
                    break Ended {
                        link: self.link,
                        result: Ok(self.done),
                    };
                }
                Err(err) => {
                    break Ended {
                        link: self.link,
                        result: Err(err),
                    };
                }
            }
        }
    }

    async fn on(&mut self, update: Option<Update>) -> anyhow::Result<Flow> {
        match update {
            Some(Update::Event(event)) => self.on_event(event).await,
            Some(update) => {
                self.log(format!("{update:?}"))?;
                Ok(Flow::Next)
            }
            None => Err(anyhow::Error::msg("the link stopped")),
        }
    }

    async fn on_event(&mut self, event: Event) -> anyhow::Result<Flow> {
        self.log(format!("event #{} {:?}", event.seq.0, event.kind))?;
        match (event.kind, self.plan) {
            (EventKind::AssistantDelta { .. }, plan) => {
                self.tally.deltas = self.tally.deltas.saturating_add(1);
                match (plan, self.tally.deltas) {
                    (Plan::InterruptOnFirstDelta, 1) => {
                        let result = interrupt(&self.link, self.start, self.session).await?;
                        self.tally.interrupt = Some(result);
                    }
                    (Plan::InterruptOnFirstDelta | Plan::Approve | Plan::InterruptAtSend, _) => {}
                }
                Ok(Flow::Next)
            }
            (EventKind::RequestOpened { request, .. }, plan) => {
                let decision = match plan {
                    Plan::Approve => Decision::Approve,
                    Plan::InterruptOnFirstDelta | Plan::InterruptAtSend => Decision::Deny,
                };
                let session = self.session;
                let result = self
                    .link
                    .send(CommandBody::Resolve {
                        session,
                        request,
                        decision,
                    })
                    .await?;
                self.log(format!("resolve {request:?} {decision:?} -> {result:?}"))?;
                self.tally.approved = match (decision, result) {
                    (Decision::Approve, CommandResult::Accepted) => {
                        self.tally.approved.saturating_add(1)
                    }
                    (Decision::Approve | Decision::Deny, _) => self.tally.approved,
                };
                Ok(Flow::Next)
            }
            (EventKind::TurnCompleted { outcome }, _) => {
                self.done.push(Tally {
                    outcome: Some(outcome),
                    ..self.tally.clone()
                });
                self.next_turn().await
            }
            (EventKind::SessionClosed { reason }, _) => Err(anyhow::Error::msg(format!(
                "the session closed: {reason:?}"
            ))),
            (
                EventKind::SessionCreated { .. }
                | EventKind::SessionReady
                | EventKind::UserMessage { .. }
                | EventKind::AssistantMessage { .. }
                | EventKind::ToolCall { .. }
                | EventKind::RequestResolved { .. }
                | EventKind::Error { .. },
                _,
            ) => Ok(Flow::Next),
        }
    }

    async fn next_turn(&mut self) -> anyhow::Result<Flow> {
        match PLANS.get(self.done.len()).copied() {
            Some(plan) => {
                let interrupt = send_turn(&self.link, self.start, self.session, plan).await?;
                self.plan = plan;
                self.tally = Tally { interrupt, ..FRESH };
                Ok(Flow::Next)
            }
            None => Ok(Flow::Ended),
        }
    }

    fn log(&self, line: String) -> std::io::Result<()> {
        log(self.start, line)
    }
}
