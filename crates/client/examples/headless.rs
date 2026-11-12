//! Connects to a host with a ticket, starts a session, sends one message,
//! answers its requests and prints every update until the turn completes.
//!
//! `cargo run -p client --example headless -- <ticket> --directory DIR [--reconnect-after N]`
//!
//! `--register env` first sets the provider's credential on the host from
//! this process's environment (`Register`).

use std::cmp::Ordering;
use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::{Parser, ValueEnum};
use client::{HostLink, LinkOptions, LinkState, Status, Target, Update};
use proto::{
    Auth, Availability, ClaudeCredential, ClientName, CodexCredential, CommandBody, CommandResult,
    Credential, Decision, Event, EventKind, HostPath, HostSeq, Provider, SessionId, Ticket,
};
use tokio::sync::mpsc;

#[derive(Parser)]
struct Args {
    #[arg(value_parser = Ticket::parse)]
    ticket: Ticket,
    #[arg(long, value_enum, default_value_t = ProviderChoice::Claude)]
    provider: ProviderChoice,
    /// The session's directory on the host.
    #[arg(long)]
    directory: String,
    #[arg(long, default_value = "what is the answer?")]
    prompt: String,
    #[arg(long, value_enum, default_value_t = Choice::Approve)]
    decision: Choice,
    /// Drop the connection after this many transcript deltas of our turn,
    /// to show the resubscribe with `after_seq`.
    #[arg(long)]
    reconnect_after: Option<usize>,
    /// Interrupt the turn on its first delta, or right after the message.
    #[arg(long, value_enum, default_value_t = When::Never)]
    interrupt: When,
    #[arg(long, default_value_t = 60)]
    timeout_secs: u64,
    #[arg(long, value_enum, default_value_t = Register::Keep)]
    register: Register,
}

/// Whether to set the provider's credential on the host before the session.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum Register {
    /// Leave the host's credential as it is.
    Keep,
    /// A Claude gateway from `ANTHROPIC_BASE_URL` and `ANTHROPIC_AUTH_TOKEN`,
    /// or a Codex provider from `CONDUCTOR_CODEX_NAME`, `_BASE_URL`,
    /// `_MODEL` and `_KEY`. Nothing of them is printed.
    Env,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ProviderChoice {
    Claude,
    Codex,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum When {
    Never,
    FirstDelta,
    AtSend,
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
    let (link, updates) = HostLink::spawn(
        Target::of_ticket(&args.ticket),
        LinkOptions::new(ClientName(String::from("headless"))),
    )
    .await?;
    let decision = match args.decision {
        Choice::Approve => Decision::Approve,
        Choice::Deny => Decision::Deny,
    };
    let provider = match args.provider {
        ProviderChoice::Claude => Provider::Claude,
        ProviderChoice::Codex => Provider::Codex,
    };
    let session = Session {
        start,
        provider,
        directory: HostPath(args.directory),
        prompt: args.prompt,
        decision,
        reconnect_after: args.reconnect_after,
        interrupt: args.interrupt,
        register: args.register,
        link,
        updates,
        ours: None,
        last_seq: HostSeq(0),
        deltas: 0,
        reconnected: Reconnected::No,
        times_live: 0,
        was: Liveness::NotLive,
    };
    let ended = session.run().await;
    ended.link.stop().await?;
    let _ = ended.result?;
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

struct Ended {
    link: HostLink,
    result: anyhow::Result<Flow>,
}

enum Flow {
    Next,
    Finished,
}

/// Our session, and the seq it started after: earlier events are history.
#[derive(Debug, Clone, Copy)]
struct Ours {
    session: SessionId,
    after: HostSeq,
}

/// The client's side of one turn, driven by the link's updates.
struct Session {
    start: Instant,
    provider: Provider,
    directory: HostPath,
    prompt: String,
    decision: Decision,
    reconnect_after: Option<usize>,
    interrupt: When,
    register: Register,
    link: HostLink,
    updates: mpsc::Receiver<Update>,
    ours: Option<Ours>,
    last_seq: HostSeq,
    deltas: usize,
    reconnected: Reconnected,
    times_live: u32,
    was: Liveness,
}

impl Session {
    /// Hands the link back however the turn ended, so it can be stopped.
    async fn run(mut self) -> Ended {
        loop {
            let update = self.updates.recv().await;
            match self.on(update).await {
                Ok(Flow::Next) => {}
                result => {
                    break Ended {
                        link: self.link,
                        result,
                    };
                }
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
        let Status { state, error, .. } = status;
        let error = error.map(|e| format!(" error={e}")).unwrap_or_default();
        self.log(format!("status {state:?}{error}"))?;
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
        match self.ours {
            Some(_) => Ok(Flow::Next),
            None => {
                self.register().await?;
                let after = self.last_seq;
                let session = self.start_session().await?;
                self.ours = Some(Ours { session, after });
                let text = self.prompt.clone();
                let result = self
                    .link
                    .send(CommandBody::SendMessage { session, text })
                    .await?;
                self.log(format!("message {:?} -> {result:?}", self.prompt))?;
                if let When::AtSend = self.interrupt {
                    self.interrupt(session).await?;
                }
                Ok(Flow::Next)
            }
        }
    }

    /// Logs only the kind the host now reports, never a value.
    async fn register(&self) -> anyhow::Result<()> {
        let credential = match (self.register, self.provider) {
            (Register::Keep, _) => None,
            (Register::Env, Provider::Claude) => {
                Some(Credential::Claude(ClaudeCredential::Gateway {
                    base_url: var("ANTHROPIC_BASE_URL")?,
                    token: var("ANTHROPIC_AUTH_TOKEN")?,
                }))
            }
            (Register::Env, Provider::Codex) => {
                Some(Credential::Codex(CodexCredential::Provider {
                    name: var("CONDUCTOR_CODEX_NAME")?,
                    base_url: var("CONDUCTOR_CODEX_BASE_URL")?,
                    key: var("CONDUCTOR_CODEX_KEY")?,
                    model: var("CONDUCTOR_CODEX_MODEL")?,
                }))
            }
        };
        match credential {
            None => Ok(()),
            Some(credential) => {
                let result = self
                    .link
                    .send(CommandBody::SetCredential { credential })
                    .await?;
                let kind = match &result {
                    CommandResult::Providers(providers) => match providers.of(self.provider) {
                        Availability::Available {
                            auth: Auth::Gateway { .. },
                        } => Ok("gateway"),
                        Availability::Available { .. } | Availability::Unavailable { .. } => {
                            Err(anyhow::Error::msg("the host does not report the gateway"))
                        }
                    },
                    CommandResult::Rejected(rejection) => {
                        Err(anyhow::Error::msg(rejection.label()))
                    }
                    CommandResult::Accepted | CommandResult::Started { .. } => {
                        Err(anyhow::Error::msg("the host gave an unexpected answer"))
                    }
                }?;
                self.log(format!("registered the credential -> {kind}"))?;
                Ok(())
            }
        }
    }

    async fn start_session(&self) -> anyhow::Result<SessionId> {
        let result = self
            .link
            .send(CommandBody::StartSession {
                provider: self.provider,
                directory: self.directory.clone(),
            })
            .await?;
        self.log(format!("start {:?} -> {result:?}", self.directory.0))?;
        match result {
            CommandResult::Started { session } => Ok(session),
            CommandResult::Accepted | CommandResult::Providers(_) | CommandResult::Rejected(_) => {
                Err(anyhow::Error::msg(format!(
                    "the host did not start a session: {result:?}"
                )))
            }
        }
    }

    async fn on_event(&mut self, event: Event) -> anyhow::Result<Flow> {
        self.last_seq = event.seq;
        let origin = match self
            .ours
            .filter(|ours| ours.session == event.session)
            .map(|ours| event.seq.cmp(&ours.after))
        {
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
            EventKind::AssistantDelta { .. } => {
                self.deltas = self.deltas.saturating_add(1);
                if let (When::FirstDelta, 1) = (self.interrupt, self.deltas) {
                    self.interrupt(event.session).await?;
                }
                match (self.reconnect_after, self.reconnected) {
                    (Some(after), Reconnected::No) if after == self.deltas => {
                        self.reconnected = Reconnected::Yes;
                        self.log(format!(
                            "dropping the connection mid-turn at seq {}",
                            event.seq.0
                        ))?;
                        self.link.reconnect().await;
                    }
                    _ => {}
                }
                Ok(Flow::Next)
            }
            EventKind::RequestOpened { request, .. } => {
                let decision = self.decision;
                let result = self
                    .link
                    .send(CommandBody::Resolve {
                        session: event.session,
                        request,
                        decision,
                    })
                    .await?;
                self.log(format!("resolve {request:?} {decision:?} -> {result:?}"))?;
                Ok(Flow::Next)
            }
            EventKind::TurnCompleted { .. } => {
                self.log(String::from("turn completed"))?;
                Ok(Flow::Finished)
            }
            EventKind::SessionClosed { reason } => Err(anyhow::Error::msg(format!(
                "the session closed: {reason:?}"
            ))),
            EventKind::SessionCreated { .. }
            | EventKind::SessionReady
            | EventKind::UserMessage { .. }
            | EventKind::AssistantMessage { .. }
            | EventKind::ToolCall { .. }
            | EventKind::RequestResolved { .. }
            | EventKind::Error { .. } => Ok(Flow::Next),
        }
    }

    async fn interrupt(&self, session: SessionId) -> anyhow::Result<()> {
        let result = self.link.send(CommandBody::Interrupt { session }).await?;
        self.log(format!("interrupt -> {result:?}"))?;
        Ok(())
    }

    fn log(&self, line: String) -> std::io::Result<()> {
        log(self.start, line)
    }
}

/// An environment variable the credential needs. Its error names the
/// variable only.
fn var(name: &str) -> anyhow::Result<String> {
    std::env::var(name).with_context(|| format!("{name} is not set"))
}
