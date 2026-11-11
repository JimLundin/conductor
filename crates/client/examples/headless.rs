//! Connects to a host with a ticket, sends one prompt, answers its approval and
//! prints every update until the turn finishes.
//!
//! `cargo run -p client --example headless -- <ticket> [--reconnect-after N]`

use std::cmp::Ordering;
use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::{Parser, ValueEnum};
use client::{HostLink, LinkOptions, LinkState, Status, Target, Update};
use proto::{CommandBody, Decision, Event, EventKind, HostSeq, Ticket};
use tokio::sync::mpsc;

#[derive(Parser)]
struct Args {
    #[arg(value_parser = Ticket::parse)]
    ticket: Ticket,
    #[arg(long, default_value = "what is the answer?")]
    prompt: String,
    #[arg(long, value_enum, default_value_t = Choice::Approve)]
    decision: Choice,
    /// Drop the connection after this many transcript deltas of our turn,
    /// to show the resubscribe with `after_seq`.
    #[arg(long)]
    reconnect_after: Option<usize>,
    #[arg(long, default_value_t = 60)]
    timeout_secs: u64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Choice {
    Approve,
    Deny,
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
        .context("timed out before the turn finished")?
}

async fn run(args: Args) -> anyhow::Result<()> {
    let start = Instant::now();
    let endpoint = client::platform::bind_endpoint().await?;
    log(
        start,
        format!("client endpoint {}", endpoint.id().fmt_short()),
    )?;
    let (link, updates) = HostLink::spawn(
        endpoint.clone(),
        Target::of_ticket(&args.ticket),
        LinkOptions::new(String::from("headless")),
    );
    let decision = match args.decision {
        Choice::Approve => Decision::Approve,
        Choice::Deny => Decision::Deny,
    };
    let session = Session {
        start,
        prompt: args.prompt,
        decision,
        reconnect_after: args.reconnect_after,
        link,
        updates,
        prompted_after: None,
        last_seq: HostSeq(0),
        deltas: 0,
        reconnected: Reconnected::No,
        times_live: 0,
        was: Liveness::NotLive,
    };
    let link = session.run().await?;
    drop(link);
    endpoint.close().await;
    Ok(())
}

fn log(start: Instant, line: String) -> std::io::Result<()> {
    writeln!(
        std::io::stdout(),
        "[{:7.3}s] {line}",
        start.elapsed().as_secs_f64()
    )
}

#[derive(Debug, Clone, Copy)]
enum Liveness {
    Live,
    NotLive,
}

#[derive(Debug, Clone, Copy)]
enum Reconnected {
    No,
    Yes,
}

/// Whether an event came after our prompt.
enum Origin {
    Ours,
    History,
}

enum Flow {
    Next,
    Finished,
}

/// The client's side of one turn, driven by the link's updates.
struct Session {
    start: Instant,
    prompt: String,
    decision: Decision,
    reconnect_after: Option<usize>,
    link: HostLink,
    updates: mpsc::Receiver<Update>,
    /// Events up to this seq predate our prompt.
    prompted_after: Option<HostSeq>,
    last_seq: HostSeq,
    deltas: usize,
    reconnected: Reconnected,
    times_live: u32,
    was: Liveness,
}

impl Session {
    /// Hands the link back once the turn finished.
    async fn run(mut self) -> anyhow::Result<HostLink> {
        loop {
            let update = self.updates.recv().await;
            match self.on(update).await? {
                Flow::Next => {}
                Flow::Finished => break Ok(self.link),
            }
        }
    }

    async fn on(&mut self, update: Option<Update>) -> anyhow::Result<Flow> {
        match update {
            Some(Update::Status(status)) => self.on_status(status).await,
            Some(Update::Reset(epoch)) => {
                self.log(format!("reset to epoch {}", epoch.0))?;
                self.last_seq = HostSeq(0);
                Ok(Flow::Next)
            }
            Some(Update::Event(event)) => self.on_event(event).await,
            None => Err(anyhow::Error::msg("the link stopped")),
        }
    }

    async fn on_status(&mut self, status: Status) -> anyhow::Result<Flow> {
        let Status {
            state, path, error, ..
        } = status;
        let error = error.map(|e| format!(" error={e}")).unwrap_or_default();
        self.log(format!("status {state:?} path={path:?}{error}"))?;
        let now = match state {
            LinkState::Live => Liveness::Live,
            LinkState::Disconnected
            | LinkState::Connecting
            | LinkState::Authenticating
            | LinkState::CatchingUp => Liveness::NotLive,
        };
        let was = self.was;
        self.was = now;
        match (now, was) {
            (Liveness::Live, Liveness::NotLive) => self.went_live().await,
            (Liveness::Live, Liveness::Live) | (Liveness::NotLive, _) => Ok(Flow::Next),
        }
    }

    async fn went_live(&mut self) -> anyhow::Result<Flow> {
        self.times_live = self.times_live.saturating_add(1);
        self.log(format!(
            "live #{} after {:.3}s",
            self.times_live,
            self.start.elapsed().as_secs_f64()
        ))?;
        match self.prompted_after {
            Some(_) => Ok(Flow::Next),
            None => {
                self.prompted_after = Some(self.last_seq);
                let text = self.prompt.clone();
                let result = self.link.send(CommandBody::Prompt { text }).await?;
                self.log(format!("prompt {:?} -> {result:?}", self.prompt))?;
                Ok(Flow::Next)
            }
        }
    }

    async fn on_event(&mut self, event: Event) -> anyhow::Result<Flow> {
        self.last_seq = event.seq;
        let origin = match self.prompted_after.map(|after| event.seq.0.cmp(&after.0)) {
            Some(Ordering::Greater) => Origin::Ours,
            Some(Ordering::Less | Ordering::Equal) | None => Origin::History,
        };
        let marker = match origin {
            Origin::Ours => "",
            Origin::History => "(history) ",
        };
        self.log(format!("event #{} {marker}{:?}", event.seq.0, event.kind))?;
        match origin {
            Origin::Ours => self.on_our_event(event).await,
            Origin::History => Ok(Flow::Next),
        }
    }

    async fn on_our_event(&mut self, event: Event) -> anyhow::Result<Flow> {
        match event.kind {
            EventKind::TranscriptDelta { .. } => {
                self.deltas = self.deltas.saturating_add(1);
                let due = self
                    .reconnect_after
                    .and_then(|after| after.eq(&self.deltas).then_some(after));
                match (due, self.reconnected) {
                    (Some(_), Reconnected::No) => {
                        self.reconnected = Reconnected::Yes;
                        self.log(format!(
                            "dropping the connection mid-turn at seq {}",
                            event.seq.0
                        ))?;
                        self.link.reconnect().await;
                        Ok(Flow::Next)
                    }
                    (_, Reconnected::No | Reconnected::Yes) => Ok(Flow::Next),
                }
            }
            EventKind::ApprovalRequested { id, .. } => {
                let decision = self.decision;
                let result = self
                    .link
                    .send(CommandBody::Resolve {
                        approval: id,
                        decision,
                    })
                    .await?;
                self.log(format!("resolve {id:?} {decision:?} -> {result:?}"))?;
                Ok(Flow::Next)
            }
            EventKind::TurnFinished => {
                self.log(format!("turn finished; path={:?}", self.link.status().path))?;
                Ok(Flow::Finished)
            }
            EventKind::AssistantMessage { .. } | EventKind::ApprovalResolved { .. } => {
                Ok(Flow::Next)
            }
        }
    }

    fn log(&self, line: String) -> std::io::Result<()> {
        log(self.start, line)
    }
}
