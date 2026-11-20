//! Connects to a host with a ticket, starts a session, sends one message,
//! answers its requests and prints every update until the turn completes.
//! What it knows of the session is `client::host_view`'s fold of the
//! updates; it decides only what to send.
//!
//! `cargo run -p net --example headless -- <ticket> --directory DIR [--reconnect-after N]`
//!
//! `--register env` first sets the provider's credential on the host from
//! this process's environment (`Register`).
//!
//! `--then-continue PROMPT` waits after the turn for the session to close,
//! as when something kills its CLI, then continues the session and sends
//! `PROMPT` as a second turn.
//!
//! `--then-undo PROMPT` sends a second edit turn, reads its diff and undoes
//! it, then checks the retained amber/discarded unknown codeword history.

use std::collections::BTreeSet;
use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::{Parser, ValueEnum};
use client::host_view::{
    Approval, Attention, Composer, HostView, Item, RequestState, Resume,
    SessionStatus, SessionView, Turn, UndoState,
};
use net::{HostLink, LinkOptions, LinkState, Status, Target, Update};
use proto::{
    Answer, Auth, Availability, Change, CheckpointId, ClientName, CommandBody,
    CommandResult, Credential, Decision, Event, EventKind, HostPath, HostSeq,
    InstanceId, Provider, Question, RequestId, SessionId, StartAt, Ticket,
    TurnOutcome,
};
use tokio::sync::mpsc;

#[derive(Parser)]
struct Args {
    #[arg(value_parser = net::parse)]
    ticket: Ticket,
    #[arg(long, value_parser = provider, default_value = "claude")]
    provider: Provider,
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
    /// After the turn, wait for the session to close, continue it and send
    /// this.
    #[arg(long)]
    then_continue: Option<String>,
    /// Send this second turn, undo it, then check retained conversation history.
    #[arg(long, conflicts_with = "then_continue")]
    then_undo: Option<String>,
}

/// What follows the first turn.
enum Then {
    Finish,
    Continue,
    Undo,
}

#[derive(Debug, Clone, Copy)]
enum Stage {
    FirstTurn,
    /// Waiting for the session to close.
    Open,
    /// `Continue` sent; waiting for the session to take a message.
    Continuing,
    SecondTurn,
    Undoing,
    VerifyTurn,
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

enum Flow {
    Next,
    Finished,
}

const VERIFY: &str = "What are my retained and discarded codewords? If a codeword was never supplied, use unknown. Reply with exactly retained=WORD discarded=WORD, lowercase, no other text, and use no tools.";

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
    let (link, updates) = HostLink::spawn(
        Target::of_ticket(&args.ticket)?,
        LinkOptions::new(ClientName(String::from("headless"))),
    )
    .await?;
    let (then, second) = match (args.then_continue, args.then_undo) {
        (Some(text), _) => (Then::Continue, text),
        (None, Some(text)) => (Then::Undo, text),
        (None, None) => (Then::Finish, String::new()),
    };
    let session = Session {
        start: Instant::now(),
        provider: args.provider,
        directory: HostPath(args.directory),
        prompt: args.prompt,
        decision: match args.decision {
            Choice::Approve => Decision::Approve,
            Choice::Deny => Decision::Deny,
        },
        reconnect_after: args.reconnect_after,
        interrupt: args.interrupt,
        register: args.register,
        then,
        second,
        stage: Stage::FirstTurn,
        link,
        updates,
        status: None,
        view: HostView::empty(),
        ours: None,
        before: None,
        answered: BTreeSet::new(),
        deltas: 0,
    };
    let (link, result) = session.run().await;
    link.stop().await?;
    result
}

/// The client's side of one session, driven by the link's updates.
struct Session {
    start: Instant,
    provider: Provider,
    directory: HostPath,
    prompt: String,
    decision: Decision,
    reconnect_after: Option<usize>,
    interrupt: When,
    register: Register,
    then: Then,
    /// The message of the turn after the first.
    second: String,
    stage: Stage,
    link: HostLink,
    updates: mpsc::Receiver<Update>,
    status: Option<Status>,
    view: HostView,
    ours: Option<SessionId>,
    /// The turn that was latest when we last sent a message: the one we
    /// wait on comes after it.
    before: Option<HostSeq>,
    answered: BTreeSet<RequestId>,
    deltas: usize,
}

impl Session {
    /// Hands the link back however the session ended, so it can be stopped.
    async fn run(mut self) -> (HostLink, anyhow::Result<()>) {
        let result = loop {
            let next = self.updates.recv().await;
            match self.on(next).await {
                Ok(Flow::Next) => {}
                Ok(Flow::Finished) => break Ok(()),
                Err(error) => break Err(error),
            }
        };
        (self.link, result)
    }

    async fn on(&mut self, next: Option<Update>) -> anyhow::Result<Flow> {
        let update = next.context("the link stopped")?;
        self.log(&self.said(&update))?;
        let delta = matches!(
            &update,
            Update::Event(Event {
                kind: EventKind::AssistantDelta { .. },
                session,
                ..
            }) if Some(*session) == self.ours
        );
        let (was, had) = (self.live(), self.workspace());
        let (status, view) = net::shown_after(
            self.status.take(),
            std::mem::replace(&mut self.view, HostView::empty()),
            update,
        );
        self.status = status;
        self.view = view;
        if let Some(directory) =
            self.workspace().filter(|now| Some(now) != had.as_ref())
        {
            self.log(&format!("workspace {directory}"))?;
        }
        if !was && self.live() {
            self.went_live().await?;
        }
        if delta {
            self.delta().await?;
        }
        match self.ours().cloned() {
            Some(view) => self.step(&view).await,
            None => Ok(Flow::Next),
        }
    }

    fn live(&self) -> bool {
        self.status.as_ref().map(|status| status.state) == Some(LinkState::Live)
    }

    fn workspace(&self) -> Option<String> {
        let workspace = self.ours()?.workspace.as_ref()?;
        Some(workspace.directory.0.clone())
    }

    fn ours(&self) -> Option<&SessionView> {
        self.ours
            .and_then(|session| self.view.sessions.get(&session))
    }

    fn said(&self, update: &Update) -> String {
        match update {
            Update::Status(Status { state, error, .. }) => {
                let detail = error
                    .as_ref()
                    .map_or(String::new(), |e| format!(" error={e}"));
                format!("status {state:?}{detail}")
            }
            Update::Reset(epoch) => format!("reset to epoch {}", epoch.0),
            Update::Event(event) => {
                let marker = if Some(event.session) == self.ours {
                    ""
                } else {
                    "(history) "
                };
                format!("event #{} {marker}{:?}", event.seq.0, event.kind)
            }
        }
    }

    async fn went_live(&mut self) -> anyhow::Result<()> {
        let elapsed = self.start.elapsed().as_secs_f64();
        self.log(&format!("live after {elapsed:.3}s"))?;
        if self.ours.is_none() {
            self.register().await?;
            let start = CommandBody::StartSession {
                instance: self.instance(),
                at: StartAt::Directory(self.directory.clone()),
                model: None,
                effort: None,
            };
            let said = format!("start {:?}", self.directory.0);
            let CommandResult::Started { session } =
                self.send(&said, start).await?
            else {
                anyhow::bail!("the host did not start a session");
            };
            self.ours = Some(session);
            self.message(session, self.prompt.clone()).await?;
            if matches!(self.interrupt, When::AtSend) {
                self.send("interrupt", CommandBody::Interrupt { session })
                    .await?;
            }
        }
        Ok(())
    }

    async fn delta(&mut self) -> anyhow::Result<()> {
        self.deltas = self.deltas.saturating_add(1);
        let session = self.ours.context("a delta before our session")?;
        if matches!((self.interrupt, self.deltas), (When::FirstDelta, 1)) {
            self.send("interrupt", CommandBody::Interrupt { session })
                .await?;
        }
        if self.reconnect_after == Some(self.deltas) {
            self.reconnect_after = None;
            self.log("dropping the connection mid-turn")?;
            self.link.reconnect().await;
        }
        Ok(())
    }

    /// What to send next, from what the view of our session now says.
    async fn step(&mut self, view: &SessionView) -> anyhow::Result<Flow> {
        let session = self.ours.context("a step before our session")?;
        self.answer_requests(session, view).await?;
        if let UndoState::Failed { reason, .. } = &view.undo {
            anyhow::bail!("undo failed: {reason}");
        }
        let controls = view.controls();
        match (self.stage, &view.status) {
            (Stage::Open, SessionStatus::Closed(reason))
                if controls.resume == Resume::Offered =>
            {
                self.stage = Stage::Continuing;
                let said = format!("continue after {reason:?}");
                self.send(&said, CommandBody::Continue { session }).await?;
            }
            (Stage::Continuing, _) if controls.composer == Composer::Open => {
                self.stage = Stage::SecondTurn;
                self.message(session, self.second.clone()).await?;
            }
            (Stage::Open | Stage::Continuing, _) => {}
            (_, SessionStatus::Closed(reason)) => {
                anyhow::bail!("the session closed: {reason:?}")
            }
            (Stage::Undoing, _) => {
                if view.undo == UndoState::Undone {
                    self.stage = Stage::VerifyTurn;
                    self.message(session, String::from(VERIFY)).await?;
                }
            }
            (Stage::FirstTurn | Stage::SecondTurn | Stage::VerifyTurn, _) => {
                let ours = view
                    .last_turn()
                    .filter(|turn| Some(turn.started) > self.before);
                if let Some(turn) = ours {
                    return self.turn_moved(session, turn).await;
                }
            }
        }
        Ok(Flow::Next)
    }

    async fn turn_moved(
        &mut self,
        session: SessionId,
        turn: Turn<'_>,
    ) -> anyhow::Result<Flow> {
        let Some(outcome) = turn.outcome() else {
            return Ok(Flow::Next);
        };
        match (&self.then, self.stage, outcome) {
            (Then::Undo, Stage::FirstTurn, TurnOutcome::Completed) => {
                self.stage = Stage::SecondTurn;
                self.message(session, self.second.clone()).await?;
                Ok(Flow::Next)
            }
            (Then::Undo, Stage::SecondTurn, TurnOutcome::Completed) => {
                if let Some(checkpoint) = turn.checkpoint {
                    self.undo(session, checkpoint).await?;
                }
                Ok(Flow::Next)
            }
            (Then::Undo, Stage::VerifyTurn, TurnOutcome::Completed) => {
                self.verify(turn.items)
            }
            (Then::Undo, _, _) => {
                anyhow::bail!("the undo smoke turn did not complete")
            }
            (Then::Continue, Stage::FirstTurn, _) => {
                self.stage = Stage::Open;
                self.log("turn completed; waiting for the agent to exit")?;
                Ok(Flow::Next)
            }
            (Then::Continue | Then::Finish, _, _) => {
                self.log("turn completed")?;
                Ok(Flow::Finished)
            }
        }
    }

    fn verify(&self, items: &[Item]) -> anyhow::Result<Flow> {
        let replies = items
            .iter()
            .filter_map(|item| match item {
                Item::Reply { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        let reply = replies.trim();
        anyhow::ensure!(
            reply == "retained=amber discarded=unknown",
            "undo history check failed: {reply}"
        );
        self.log("undo verified retained history")?;
        Ok(Flow::Finished)
    }

    /// Approves or denies each tool call as `--decision` says, and gives
    /// each question its first option, or "yes" if it has none.
    async fn answer_requests(
        &mut self,
        session: SessionId,
        view: &SessionView,
    ) -> anyhow::Result<()> {
        if view.attention() == Attention::Quiet {
            return Ok(());
        }
        for item in &view.items {
            let (request, command, said) = match item {
                Item::Tool {
                    approval:
                        Approval::Asked {
                            request,
                            state: RequestState::Open,
                            ..
                        },
                    ..
                } => (
                    *request,
                    CommandBody::Resolve {
                        session,
                        request: *request,
                        decision: self.decision,
                    },
                    format!("resolve {request:?} {:?}", self.decision),
                ),
                Item::Question {
                    request,
                    questions,
                    state: RequestState::Open,
                } => {
                    let answers = questions.iter().map(first_or_yes).collect();
                    let said = format!("answer {request:?} {answers:?}");
                    let answer = CommandBody::Answer {
                        session,
                        request: *request,
                        answers,
                    };
                    (*request, answer, said)
                }
                _ => continue,
            };
            if self.answered.insert(request) {
                self.send(&said, command).await?;
            }
        }
        Ok(())
    }

    /// Reads the turn's diff, which must hold the files it wrote, then
    /// undoes the turn.
    async fn undo(
        &mut self,
        session: SessionId,
        checkpoint: CheckpointId,
    ) -> anyhow::Result<()> {
        let get = CommandBody::GetDiff {
            session,
            checkpoint,
        };
        let result = self.link.send(get).await?;
        let CommandResult::Diff(diff) = result else {
            anyhow::bail!("diff failed: {result:?}");
        };
        let wrote = |path: &str, added: bool| {
            diff.files.iter().any(|file| {
                file.path == path
                    && (!added || matches!(file.change, Change::Write))
            })
        };
        anyhow::ensure!(
            wrote("undo.txt", false),
            "the second turn diff has no undo.txt"
        );
        anyhow::ensure!(
            wrote("discarded-only.txt", true),
            "the second turn diff has no added discarded-only.txt"
        );
        self.log(&format!(
            "undo diff checkpoint={} files={:?}",
            checkpoint.0, diff.files
        ))?;
        self.stage = Stage::Undoing;
        let undo = CommandBody::UndoTurn {
            session,
            checkpoint,
        };
        accepted(&self.send("undo", undo).await?)
    }

    async fn message(
        &mut self,
        session: SessionId,
        text: String,
    ) -> anyhow::Result<()> {
        let last = self.ours().and_then(SessionView::last_turn);
        self.before = last.map(|turn| turn.started);
        let said = format!("message {text:?}");
        let send = CommandBody::SendMessage { session, text };
        accepted(&self.send(&said, send).await?)
    }

    /// Sends `command` and logs its answer after `said`.
    async fn send(
        &self,
        said: &str,
        command: CommandBody,
    ) -> anyhow::Result<CommandResult> {
        let result = self.link.send(command).await?;
        self.log(&format!("{said} -> {result:?}"))?;
        Ok(result)
    }

    /// The instance its sessions run on: the account of the gateway it
    /// registers, which the gateway's URL names, else the provider's own.
    fn instance(&self) -> InstanceId {
        let base_url = match self.provider {
            Provider::Claude => "ANTHROPIC_BASE_URL",
            Provider::Codex => "CONDUCTOR_CODEX_BASE_URL",
        };
        match (self.register, std::env::var(base_url)) {
            (Register::Env, Ok(url)) => {
                InstanceId::named(self.provider, url.trim())
            }
            (Register::Env | Register::Keep, _) => {
                InstanceId::own(self.provider)
            }
        }
    }

    /// Logs only the kind the host now reports, never a value.
    async fn register(&self) -> anyhow::Result<()> {
        let (kind, fields): (&str, &[(&str, &str)]) =
            match (self.register, self.provider) {
                (Register::Keep, _) => return Ok(()),
                (Register::Env, Provider::Claude) => (
                    "gateway",
                    &[
                        ("base_url", "ANTHROPIC_BASE_URL"),
                        ("token", "ANTHROPIC_AUTH_TOKEN"),
                    ],
                ),
                (Register::Env, Provider::Codex) => (
                    "provider",
                    &[
                        ("name", "CONDUCTOR_CODEX_NAME"),
                        ("base_url", "CONDUCTOR_CODEX_BASE_URL"),
                        ("key", "CONDUCTOR_CODEX_KEY"),
                        ("model", "CONDUCTOR_CODEX_MODEL"),
                    ],
                ),
            };
        let credential = Credential {
            kind: String::from(kind),
            values: fields
                .iter()
                .map(|(id, name)| Ok((String::from(*id), var(name)?)))
                .collect::<anyhow::Result<_>>()?,
        };
        let set = CommandBody::SetCredential {
            provider: self.provider,
            credential,
        };
        let result = self.link.send(set).await?;
        let CommandResult::Instances(instances) = &result else {
            anyhow::bail!("the host did not take the credential: {result:?}");
        };
        anyhow::ensure!(
            matches!(
                instances.availability(&self.instance()),
                Some(Availability::Available {
                    auth: Auth::Stored { .. },
                    ..
                })
            ),
            "the host does not report the gateway"
        );
        self.log("registered the credential -> gateway")?;
        Ok(())
    }

    fn log(&self, line: &str) -> std::io::Result<()> {
        let elapsed = self.start.elapsed().as_secs_f64();
        writeln!(std::io::stdout(), "[{elapsed:7.3}s] {line}")
    }
}

fn first_or_yes(question: &Question) -> Answer {
    let first = question.options.first().map(|option| option.label.clone());
    Answer(Vec::from([first.unwrap_or_else(|| String::from("yes"))]))
}

fn provider(name: &str) -> Result<Provider, String> {
    Provider::named(name).ok_or_else(|| format!("no provider is called {name}"))
}

/// An environment variable the credential needs. Its error names the
/// variable only.
fn var(name: &str) -> anyhow::Result<String> {
    std::env::var(name).with_context(|| format!("{name} is not set"))
}

fn accepted(result: &CommandResult) -> anyhow::Result<()> {
    anyhow::ensure!(
        *result == CommandResult::Accepted,
        "command was not accepted: {result:?}"
    );
    Ok(())
}
