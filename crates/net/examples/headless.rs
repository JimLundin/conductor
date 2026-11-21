//! `cargo run -p net --example headless -- <ticket> --directory DIR [--reconnect-after N]`
//!
//! `--register env` sets the host's credential from the client environment.
//! `--then-continue` sends a second prompt after the agent exits and resumes.
//! `--then-undo` edits on a second turn, checks its diff, undoes it, and checks
//! the retained amber/discarded unknown codeword history.

use std::collections::BTreeSet;
use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::{Parser, ValueEnum};
use client::host_fold::HostFold;
use client::host_view::{
    Approval, Attention, Composer, Interrupt, Item, ReplyState, Resume,
    UndoState,
};
use net::{HostLink, LinkOptions, LinkState, Status, Target, Update};
use proto::{
    Answer, Auth, Availability, Change, CheckpointId, ClientName, CommandBody,
    CommandResult, Credential, Decision, Event, EventKind, Feed, HostPath,
    InstanceId, Provider, RequestId, SessionId, StartAt, Ticket, TurnOutcome,
};
use tokio::sync::mpsc;

#[cfg(test)]
#[path = "headless/fixture.rs"]
mod fixture;

#[derive(Parser)]
struct Args {
    #[arg(value_parser = net::parse)]
    ticket: Ticket,
    #[arg(long, value_enum, default_value_t = ProviderChoice::Claude)]
    provider: ProviderChoice,
    /// The session's directory on the host.
    #[arg(long)]
    directory: String,
    /// The instance account to use when keeping the host's credential.
    #[arg(long)]
    account: Option<String>,
    /// Request a model listed by the chosen instance.
    #[arg(long)]
    model: Option<String>,
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
    /// Continue the session after it closes and send this prompt.
    #[arg(long)]
    then_continue: Option<String>,
    /// Send this second turn, undo it, then check retained conversation history.
    #[arg(long, conflicts_with = "then_continue")]
    then_undo: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum Stage {
    FirstTurn,
    /// Waiting for the session to close.
    Open,
    /// `Continue` sent; waiting for `SessionReady`.
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
    let target = Target::of_ticket(&args.ticket)?;
    let options = LinkOptions::new(ClientName(String::from("headless")));
    run_to_deadline(args, target, options).await
}

async fn run_to_deadline(
    args: Args,
    target: Target,
    options: LinkOptions,
) -> anyhow::Result<()> {
    let deadline = Duration::from_secs(args.timeout_secs);
    tokio::time::timeout(deadline, run(args, target, options))
        .await
        .context("timed out before the turn finished")?
}

async fn run(
    args: Args,
    target: Target,
    options: LinkOptions,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let (link, updates) = HostLink::spawn(target, options).await?;
    let mut session = Session {
        args,
        start,
        host: HostFold::default(),
        answered: BTreeSet::new(),
        turn_start: 0,
        stage: Stage::FirstTurn,
        link,
        updates,
        ours: None,
        instance: None,
        deltas: 0,
        times_live: 0,
        live: false,
    };
    let result = session.drive().await;
    let stopped = session.link.stop().await;
    result?;
    stopped.map_err(anyhow::Error::from)
}

enum Flow {
    Next,
    Finished,
}

struct Session {
    args: Args,
    start: Instant,
    host: HostFold,
    answered: BTreeSet<RequestId>,
    turn_start: usize,
    stage: Stage,
    link: HostLink,
    updates: mpsc::Receiver<Update>,
    ours: Option<SessionId>,
    instance: Option<InstanceId>,
    deltas: usize,
    times_live: u32,
    live: bool,
}

impl Session {
    async fn drive(&mut self) -> anyhow::Result<()> {
        while let Some(update) = self.updates.recv().await {
            match update {
                Update::Status(status) => {
                    self.host.apply(Update::Status(status.clone()));
                    self.on_status(status).await?;
                }
                Update::Reset(epoch) => {
                    self.log(&format!("reset to epoch {}", epoch.0))?;
                    anyhow::ensure!(
                        self.ours.is_none(),
                        "the host reset during the run"
                    );
                    self.host.apply(Update::Reset(epoch));
                }
                Update::Feed(Feed::Event(event)) => {
                    if matches!(self.on_event(event).await?, Flow::Finished) {
                        return Ok(());
                    }
                }
                other => self.host.apply(other),
            }
        }
        anyhow::bail!("the link stopped")
    }

    async fn on_status(&mut self, status: Status) -> anyhow::Result<()> {
        let Status { state, error, .. } = status;
        let detail = error.map_or(String::new(), |e| format!(" error={e}"));
        self.log(&format!("status {state:?}{detail}"))?;
        let live = matches!(state, LinkState::Live);
        let was_live = std::mem::replace(&mut self.live, live);
        if !live || was_live {
            return Ok(());
        }
        self.times_live = self.times_live.saturating_add(1);
        let elapsed = self.start.elapsed().as_secs_f64();
        self.log(&format!("live #{} after {elapsed:.3}s", self.times_live))?;
        if self.ours.is_none() {
            let session = self.start_session().await?;
            self.ours = Some(session);
            self.message(session, self.args.prompt.clone()).await?;
            if matches!(self.args.interrupt, When::AtSend) {
                self.interrupt(session).await?;
            }
        }
        Ok(())
    }

    async fn start_session(&mut self) -> anyhow::Result<SessionId> {
        let provider = match self.args.provider {
            ProviderChoice::Claude => Provider::Claude,
            ProviderChoice::Codex => Provider::Codex,
        };
        let instance = match self.args.register {
            Register::Keep => self.args.account.as_ref().map_or_else(
                || InstanceId::own(provider),
                |account| InstanceId::named(provider, account.trim()),
            ),
            Register::Env => self.register(provider).await?,
        };
        let directory = &self.args.directory;
        let result = self
            .link
            .send(CommandBody::StartSession {
                instance: instance.clone(),
                at: StartAt::Directory(HostPath(directory.clone())),
                model: self.args.model.clone().map(proto::ModelId),
                effort: None,
            })
            .await?;
        self.log(&format!("start {directory:?} -> {result:?}"))?;
        let CommandResult::Started { session } = result else {
            anyhow::bail!("the host did not start a session: {result:?}");
        };
        self.instance = Some(instance);
        Ok(session)
    }

    async fn register(&self, provider: Provider) -> anyhow::Result<InstanceId> {
        let (url, chosen) = match provider {
            Provider::Claude => {
                let url = var("ANTHROPIC_BASE_URL")?;
                let chosen = credential(
                    "gateway",
                    [
                        ("base_url", url.clone()),
                        ("token", var("ANTHROPIC_AUTH_TOKEN")?),
                    ],
                );
                (url, chosen)
            }
            Provider::Codex => {
                let url = var("CONDUCTOR_CODEX_BASE_URL")?;
                let chosen = credential(
                    "provider",
                    [
                        ("name", var("CONDUCTOR_CODEX_NAME")?),
                        ("base_url", url.clone()),
                        ("key", var("CONDUCTOR_CODEX_KEY")?),
                        ("model", var("CONDUCTOR_CODEX_MODEL")?),
                    ],
                );
                (url, chosen)
            }
        };
        let instance = InstanceId::named(provider, url.trim());
        let result = self
            .link
            .send(CommandBody::SetCredential {
                provider,
                credential: chosen,
            })
            .await?;
        let instances = match result {
            CommandResult::Instances(instances) => instances,
            CommandResult::Rejected(rejection) => {
                anyhow::bail!("{}", rejection.label())
            }
            _ => anyhow::bail!("the host gave an unexpected answer"),
        };
        anyhow::ensure!(
            matches!(
                instances.availability(&instance),
                Some(Availability::Available {
                    auth: Auth::Stored { .. },
                    ..
                })
            ),
            "the host does not report the gateway"
        );
        self.log("registered the credential -> gateway")?;
        Ok(instance)
    }

    async fn on_event(&mut self, event: Event) -> anyhow::Result<Flow> {
        let ours = self.ours == Some(event.session);
        if let EventKind::SessionCreated { instance, .. } = &event.kind {
            if ours {
                anyhow::ensure!(
                    self.instance.as_ref() == Some(instance),
                    "the host started this session on an unexpected instance"
                );
                let selected = if self.args.account.is_some()
                    || matches!(self.args.register, Register::Env)
                {
                    "gateway"
                } else {
                    "own"
                };
                self.log(&format!("confirmed session instance {selected}"))?;
            }
        } else if let Some(line) = transcript_line(self.ours, &event) {
            self.log(&line)?;
        }
        let delta = matches!(event.kind, EventKind::AssistantDelta { .. });
        let session = event.session;
        let seq = event.seq;
        self.host.apply(Update::Feed(Feed::Event(event)));
        if !ours {
            return Ok(Flow::Next);
        }
        if delta
            && self.host.view.sessions.get(&session).is_some_and(|view| {
                view.controls().interrupt == Interrupt::Offered
            })
        {
            self.deltas = self.deltas.saturating_add(1);
            if matches!(
                (self.args.interrupt, self.deltas),
                (When::FirstDelta, 1)
            ) {
                self.interrupt(session).await?;
            }
            if self.args.reconnect_after == Some(self.deltas) {
                self.args.reconnect_after = None;
                self.log(&format!(
                    "dropping the connection mid-turn at seq {}",
                    seq.0
                ))?;
                self.link.reconnect().await;
            }
            return Ok(Flow::Next);
        }
        self.advance(session).await
    }

    async fn advance(&mut self, session: SessionId) -> anyhow::Result<Flow> {
        self.answer_requests(session).await?;
        let view = self
            .host
            .view
            .sessions
            .get(&session)
            .context("the session is missing")?;
        let controls = view.controls();
        if let Some(reason) = view.closed_reason() {
            if matches!(self.stage, Stage::Open)
                && controls.resume == Resume::Offered
            {
                let result =
                    self.link.send(CommandBody::Continue { session }).await?;
                self.log(&format!("continue after {reason:?} -> {result:?}"))?;
                accepted(&result)?;
                self.stage = Stage::Continuing;
            } else if !matches!(self.stage, Stage::Continuing) {
                anyhow::bail!("the session closed: {reason:?}");
            }
            return Ok(Flow::Next);
        }
        match (&self.stage, &view.undo, controls.composer) {
            (Stage::Undoing, UndoState::Failed { reason, .. }, _) => {
                anyhow::bail!("{reason}")
            }
            (Stage::Undoing, UndoState::Undone, Composer::Open) => {
                self.stage = Stage::VerifyTurn;
                let text = String::from(
                    "What are my retained and discarded codewords? If a codeword was never supplied, use unknown. Reply with exactly retained=WORD discarded=WORD, lowercase, no other text, and use no tools.",
                );
                self.message(session, text).await?;
            }
            (Stage::Continuing, _, Composer::Open) => {
                self.stage = Stage::SecondTurn;
                let text = self
                    .args
                    .then_continue
                    .clone()
                    .context("no continuation prompt")?;
                self.message(session, text).await?;
            }
            (
                Stage::FirstTurn | Stage::SecondTurn | Stage::VerifyTurn,
                _,
                Composer::Open,
            ) => {
                return self.completed(session).await;
            }
            _ => {}
        }
        Ok(Flow::Next)
    }

    async fn answer_requests(
        &mut self,
        session: SessionId,
    ) -> anyhow::Result<()> {
        let view = self
            .host
            .view
            .sessions
            .get(&session)
            .context("the session is missing")?;
        if view.attention() == Attention::Quiet {
            return Ok(());
        }
        let decision = match self.args.decision {
            Choice::Approve => Decision::Approve,
            Choice::Deny => Decision::Deny,
        };
        for item in &view.items {
            match item {
                Item::Tool {
                    approval: Approval::Asked { request, state, .. },
                    ..
                } if state.is_open() && !self.answered.contains(request) => {
                    let result = self
                        .link
                        .send(CommandBody::Resolve {
                            session,
                            request: *request,
                            decision,
                        })
                        .await?;
                    self.log(&format!(
                        "resolve {request:?} {decision:?} -> {result:?}"
                    ))?;
                    record_answer(&mut self.answered, *request, &result)?;
                }
                Item::Question {
                    request,
                    questions,
                    state,
                } if state.is_open() && !self.answered.contains(request) => {
                    let answers = questions
                        .iter()
                        .map(|question| {
                            Answer(vec![question.options.first().map_or_else(
                                || String::from("yes"),
                                |option| option.label.clone(),
                            )])
                        })
                        .collect();
                    let result = self
                        .link
                        .send(CommandBody::Answer {
                            session,
                            request: *request,
                            answers,
                        })
                        .await?;
                    self.log(&format!("answer {request:?} -> {result:?}"))?;
                    record_answer(&mut self.answered, *request, &result)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn completed(&mut self, session: SessionId) -> anyhow::Result<Flow> {
        let view = self
            .host
            .view
            .sessions
            .get(&session)
            .context("the session is missing")?;
        let Some(turn) = view.completed_turn_since(self.turn_start) else {
            return Ok(Flow::Next);
        };
        if let TurnOutcome::Failed { message } = turn.outcome {
            anyhow::bail!("the turn failed: {message}");
        }
        if let (Stage::FirstTurn, Some(workspace)) =
            (self.stage, &view.workspace)
        {
            self.log(&format!("workspace {}", workspace.directory.0))?;
        }
        if let Some(then) = &self.args.then_undo {
            anyhow::ensure!(
                *turn.outcome == TurnOutcome::Completed,
                "the undo smoke turn did not complete"
            );
            match self.stage {
                Stage::FirstTurn => {
                    self.stage = Stage::SecondTurn;
                    self.message(session, then.clone()).await?;
                }
                Stage::SecondTurn => {
                    if let Some(checkpoint) = turn.checkpoint {
                        self.undo(session, checkpoint).await?;
                    }
                }
                Stage::VerifyTurn => {
                    let reply = turn
                        .items
                        .iter()
                        .filter_map(|item| match item {
                            Item::Reply {
                                text,
                                state: ReplyState::Done,
                            } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<String>();
                    anyhow::ensure!(
                        reply.trim() == "retained=amber discarded=unknown",
                        "undo history check failed: {reply}"
                    );
                    self.log("undo verified retained history")?;
                    return Ok(Flow::Finished);
                }
                _ => {}
            }
        } else if matches!(self.stage, Stage::FirstTurn)
            && self.args.then_continue.is_some()
        {
            self.stage = Stage::Open;
            self.log("turn completed; waiting for the agent to exit")?;
        } else {
            self.log("turn completed")?;
            return Ok(Flow::Finished);
        }
        Ok(Flow::Next)
    }

    async fn message(
        &mut self,
        session: SessionId,
        text: String,
    ) -> anyhow::Result<()> {
        self.turn_start = self
            .host
            .view
            .sessions
            .get(&session)
            .map_or(0, |view| view.items.len());
        let result = self
            .link
            .send(CommandBody::SendMessage {
                session,
                text: text.clone(),
            })
            .await?;
        self.log(&format!("message {text:?} -> {result:?}"))?;
        accepted(&result)
    }

    async fn undo(
        &mut self,
        session: SessionId,
        checkpoint: CheckpointId,
    ) -> anyhow::Result<()> {
        let result = self
            .link
            .send(CommandBody::GetDiff {
                session,
                checkpoint,
            })
            .await?;
        let CommandResult::Diff(diff) = result else {
            anyhow::bail!("diff failed: {result:?}");
        };
        anyhow::ensure!(
            diff.files.iter().any(|file| file.path == "undo.txt"),
            "the second turn diff has no undo.txt"
        );
        anyhow::ensure!(
            diff.files
                .iter()
                .any(|file| file.path == "discarded-only.txt"
                    && matches!(file.change, Change::Write)),
            "the second turn diff has no added discarded-only.txt"
        );
        self.log(&format!(
            "undo diff checkpoint={} files={:?}",
            checkpoint.0, diff.files
        ))?;
        let result = self
            .link
            .send(CommandBody::UndoTurn {
                session,
                checkpoint,
            })
            .await?;
        accepted(&result)?;
        self.stage = Stage::Undoing;
        Ok(())
    }

    async fn interrupt(&self, session: SessionId) -> anyhow::Result<()> {
        let result = self.link.send(CommandBody::Interrupt { session }).await?;
        self.log(&format!("interrupt -> {result:?}"))?;
        accepted(&result)
    }

    fn log(&self, line: &str) -> std::io::Result<()> {
        let elapsed = self.start.elapsed().as_secs_f64();
        writeln!(std::io::stdout(), "[{elapsed:7.3}s] {line}")
    }
}

/// An environment variable the credential needs. Its error names the
/// variable only.
fn var(name: &str) -> anyhow::Result<String> {
    std::env::var(name).with_context(|| format!("{name} is not set"))
}

fn transcript_line(ours: Option<SessionId>, event: &Event) -> Option<String> {
    (ours == Some(event.session))
        .then(|| format!("event #{} {:?}", event.seq.0, event.kind))
}

fn record_answer(
    answered: &mut BTreeSet<RequestId>,
    request: RequestId,
    result: &CommandResult,
) -> anyhow::Result<()> {
    accepted(result)?;
    answered.insert(request);
    Ok(())
}

fn accepted(result: &CommandResult) -> anyhow::Result<()> {
    anyhow::ensure!(
        matches!(result, CommandResult::Accepted),
        "command was not accepted: {result:?}"
    );
    Ok(())
}

fn credential<const N: usize>(
    kind: &str,
    values: [(&str, String); N],
) -> Credential {
    Credential {
        kind: String::from(kind),
        values: values
            .into_iter()
            .map(|(id, value)| (String::from(id), value))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use proto::{
        CommandResult, Event, EventKind, HostSeq, Rejection, RequestId,
        SessionId,
    };

    use super::{record_answer, transcript_line};

    #[test]
    fn transcript_only_emits_events_from_the_selected_session() {
        let ours = SessionId(1);
        let theirs = SessionId(2);
        let events = [
            Event {
                seq: HostSeq(2),
                session: theirs,
                kind: EventKind::AssistantDelta {
                    text: String::from("unrelated"),
                },
            },
            Event {
                seq: HostSeq(2),
                session: ours,
                kind: EventKind::AssistantDelta {
                    text: String::from("selected"),
                },
            },
        ];
        let transcript = events
            .iter()
            .filter_map(|event| transcript_line(Some(ours), event))
            .collect::<Vec<_>>();
        assert_eq!(
            transcript,
            vec![String::from(
                "event #2 AssistantDelta { text: \"selected\" }"
            )]
        );
    }

    #[test]
    fn rejected_answers_fail_and_are_left_unhandled() {
        let mut answered = BTreeSet::new();
        let request = RequestId(4);
        let rejected = CommandResult::Rejected(Rejection::UnknownRequest);
        assert!(record_answer(&mut answered, request, &rejected).is_err());
        assert!(!answered.contains(&request));
    }
}
