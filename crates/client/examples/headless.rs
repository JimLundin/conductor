//! Connects to a host with a ticket, sends one prompt, answers its approval and
//! prints every update until the turn finishes.
//!
//! `cargo run -p client --example headless -- <ticket> [--reconnect-after N]`

use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use clap::Parser;
use client::{HostLink, LinkOptions, LinkState, Status, Update};
use proto::{CommandBody, Decision, EventKind, HostSeq, Ticket};

#[derive(Parser)]
struct Args {
    #[arg(value_parser = Ticket::parse)]
    ticket: Ticket,
    #[arg(long, default_value = "what is the answer?")]
    prompt: String,
    #[arg(long, value_parser = ["approve", "deny"], default_value = "approve")]
    decision: String,
    /// Drop the connection after this many transcript deltas of our turn,
    /// to show the resubscribe with `after_seq`.
    #[arg(long)]
    reconnect_after: Option<usize>,
    #[arg(long, default_value_t = 60)]
    timeout_secs: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
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
    let log = |line: String| println!("[{:7.3}s] {line}", start.elapsed().as_secs_f64());
    let decision = match args.decision.as_str() {
        "approve" => Decision::Approve,
        _ => Decision::Deny,
    };

    let endpoint = client::platform::bind_endpoint().await?;
    log(format!("client endpoint {}", endpoint.id().fmt_short()));
    let (link, mut updates) = HostLink::spawn(
        endpoint.clone(),
        (&args.ticket).into(),
        LinkOptions::new("headless"),
    );

    // Events up to this seq predate our prompt.
    let mut prompted_after: Option<HostSeq> = None;
    let mut last_seq = HostSeq(0);
    let mut deltas = 0usize;
    let mut reconnected = false;
    let mut times_live = 0;
    let mut was_live = false;

    while let Some(update) = updates.recv().await {
        match update {
            Update::Status(Status {
                state, path, error, ..
            }) => {
                log(format!(
                    "status {state:?} path={path:?}{}",
                    error.map(|e| format!(" error={e}")).unwrap_or_default()
                ));
                let live = state == LinkState::Live;
                if live && !was_live {
                    times_live += 1;
                    log(format!(
                        "live #{times_live} after {:.3}s",
                        start.elapsed().as_secs_f64()
                    ));
                    if prompted_after.is_none() {
                        prompted_after = Some(last_seq);
                        let result = link
                            .send(CommandBody::Prompt {
                                text: args.prompt.clone(),
                            })
                            .await?;
                        log(format!("prompt {:?} -> {result:?}", args.prompt));
                    }
                }
                was_live = live;
            }
            Update::Reset(epoch) => {
                log(format!("reset to epoch {}", epoch.0));
                last_seq = HostSeq(0);
            }
            Update::Event(event) => {
                last_seq = event.seq;
                let ours = prompted_after.is_some_and(|after| event.seq.0 > after.0);
                log(format!(
                    "event #{} {}{:?}",
                    event.seq.0,
                    if ours { "" } else { "(history) " },
                    event.kind
                ));
                if !ours {
                    continue;
                }
                match event.kind {
                    EventKind::TranscriptDelta { .. } => {
                        deltas += 1;
                        if args.reconnect_after == Some(deltas) && !reconnected {
                            reconnected = true;
                            log(format!(
                                "dropping the connection mid-turn at seq {}",
                                event.seq.0
                            ));
                            link.reconnect().await;
                        }
                    }
                    EventKind::ApprovalRequested { id, .. } => {
                        let result = link
                            .send(CommandBody::Resolve {
                                approval: id,
                                decision,
                            })
                            .await?;
                        log(format!("resolve {id:?} {decision:?} -> {result:?}"));
                    }
                    EventKind::TurnFinished => {
                        log(format!("turn finished; path={:?}", link.status().path));
                        drop(link);
                        endpoint.close().await;
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
    }
    bail!("the link stopped")
}
