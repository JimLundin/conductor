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

struct Live {
    start: Instant,
    link: HostLink,
    updates: mpsc::Receiver<Update>,
    session: SessionId,
    plan: Plan,
    tally: Tally,
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
    let live = Live {
        start,
        link,
        updates,
        session,
        plan: Plan::Approve,
        tally: FRESH,
    };
    let (write, streamed, held) = live.turns().await?;
    let checks = [
        (
            "turn 1 streamed, was approved and completed",
            Some(&write).filter(|tally| {
                tally.deltas > 0
                    && tally.approved > 0
                    && tally.outcome == Some(TurnOutcome::Completed)
            }),
        ),
        (
            "turn 2 was interrupted after its first delta",
            Some(&streamed).filter(|tally| {
                tally.deltas > 0
                    && tally.interrupt == Some(CommandResult::Accepted)
                    && tally.outcome == Some(TurnOutcome::Interrupted)
            }),
        ),
        (
            "turn 3 was interrupted at send",
            Some(&held).filter(|tally| {
                tally.interrupt == Some(CommandResult::Accepted)
                    && tally.outcome == Some(TurnOutcome::Interrupted)
            }),
        ),
    ]
    .map(|(name, passed)| (name, passed.map_or(Verdict::Fail, |_| Verdict::Pass)));
    checks
        .iter()
        .try_for_each(|(name, verdict)| log(start, format!("check {verdict:?}: {name}")))?;
    match checks
        .iter()
        .find(|(_, verdict)| matches!(verdict, Verdict::Fail))
    {
        Some((name, _)) => Err(anyhow::Error::msg(format!("failed: {name}"))),
        None => Ok(()),
    }
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
    async fn turns(mut self) -> anyhow::Result<(Tally, Tally, Tally)> {
        let write = self.turn(Plan::Approve, WRITE).await?;
        let streamed = self.turn(Plan::InterruptOnFirstDelta, COUNT).await?;
        let held = self.turn(Plan::InterruptAtSend, COUNT).await?;
        self.link.stop().await?;
        Ok((write, streamed, held))
    }

    async fn turn(&mut self, plan: Plan, text: &str) -> anyhow::Result<Tally> {
        self.plan = plan;
        self.tally = FRESH;
        let session = self.session;
        let sent = self
            .link
            .send(CommandBody::SendMessage {
                session,
                text: String::from(text),
            })
            .await?;
        self.log(format!("message {text:?} -> {sent:?}"))?;
        if let Plan::InterruptAtSend = plan {
            self.interrupt().await?;
        }
        self.until_turn_ends().await?;
        Ok(self.tally.clone())
    }

    async fn until_turn_ends(&mut self) -> anyhow::Result<()> {
        loop {
            let update = self.updates.recv().await;
            match self.on(update).await? {
                Flow::Next => {}
                Flow::Ended => break Ok(()),
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
                    (Plan::InterruptOnFirstDelta, 1) => self.interrupt().await?,
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
                self.tally.outcome = Some(outcome);
                Ok(Flow::Ended)
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

    async fn interrupt(&mut self) -> anyhow::Result<()> {
        let session = self.session;
        let result = self.link.send(CommandBody::Interrupt { session }).await?;
        self.log(format!("interrupt -> {result:?}"))?;
        self.tally.interrupt = Some(result);
        Ok(())
    }

    fn log(&self, line: String) -> std::io::Result<()> {
        log(self.start, line)
    }
}
