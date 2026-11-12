//! S2 live checklist: drives the real `codex app-server` through approve,
//! decline, interrupt, interrupt with an approval open, two concurrent
//! threads on one process, resume in a new process, an unknown approval
//! id, questions and approval for the session, a chosen model and effort,
//! reasoning, and a killed app-server resumed, printing PASS/FAIL per step.
//!
//! Usage: s2 [FIXTURE_DIR]
//! Writes raw app-server stdout per process to FIXTURE_DIR/<name>.jsonl when given.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agents::Home;
use agents::codex::{
    ApprovalId, ApprovalKind, ApprovalPolicy, CodexEvent, CodexServer, CodexThread, Decision,
    Error, Item, Sandbox, ServerOptions, ThreadEvent, ThreadId, ThreadOptions, TurnId, TurnOutcome,
};
use agents::install::{Cli, install};
use agents::live::{Cue, Presence, Verdict, fresh_scratch, log_path, presence, report};
use anyhow::{Context, Result};
use n0_future::StreamExt;
use proto::{Answer, Effort};
use serde_json::Value;
use tokio::sync::mpsc;

/// Where the pinned CLIs are installed for the live checks, under this
/// checkout's `target/` rather than the `/tmp` tmpfs. They run with the
/// user's own homes, so the user's existing logins.
const LIVE_DATA: &str = "target/conductor-live";
const SCRATCH: &str = "/tmp/s2-scratch";
const TURN_TIMEOUT: Duration = Duration::from_secs(240);

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let fixtures = std::env::args().nth(1).map(PathBuf::from);
    tokio::runtime::Runtime::new()?.block_on(run(fixtures))
}

struct Live {
    server: CodexServer,
    events: mpsc::Receiver<CodexEvent>,
}

/// How a turn answers approval requests.
#[derive(Clone, Copy)]
enum Policy {
    Approve,
    ApproveForSession,
    Decline,
}

impl Policy {
    /// The answer to a request: a question gets each one's last option.
    fn decision(self, kind: &ApprovalKind) -> Decision {
        match (self, kind) {
            (_, ApprovalKind::UserInput { questions }) => Decision::Answer(
                questions
                    .iter()
                    .map(|question| {
                        Answer(
                            question
                                .options
                                .last()
                                .map(|option| option.label.clone())
                                .into_iter()
                                .collect(),
                        )
                    })
                    .collect(),
            ),
            (Policy::Approve, _) => Decision::Accept,
            (Policy::ApproveForSession, _) => Decision::AcceptForSession,
            (Policy::Decline, _) => Decision::Decline,
        }
    }
}

/// How the turns of a run are driven.
enum Plan {
    Answer(Policy),
    /// Interrupt at the cue; decline any approval request.
    Interrupt(Cue),
    /// Kill the app-server at the cue; the run ends when it exits.
    Kill(Cue),
}

/// Whether every turn of a run has completed.
enum Progress {
    Playing,
    Done,
}

/// What one turn saw.
struct Turn {
    thread: CodexThread,
    id: TurnId,
    approvals: Vec<String>,
    answered: Vec<ApprovalId>,
    cancelled: usize,
    items: Vec<String>,
    text: String,
    /// The text of every `AgentDelta`, joined.
    streamed: String,
    /// The text of each finished reasoning item.
    reasoning: Vec<String>,
    /// The text of every `ReasoningDelta`, joined.
    thought: String,
    outcome: Option<TurnOutcome>,
}

impl Turn {
    fn new(thread: CodexThread, id: TurnId) -> Turn {
        Turn {
            thread,
            id,
            approvals: Vec::new(),
            answered: Vec::new(),
            cancelled: 0,
            items: Vec::new(),
            text: String::new(),
            streamed: String::new(),
            reasoning: Vec::new(),
            thought: String::new(),
            outcome: None,
        }
    }

    fn record(&mut self, event: ThreadEvent) {
        match event {
            ThreadEvent::Started | ThreadEvent::Unknown(_) | ThreadEvent::TurnStarted { .. } => {}
            ThreadEvent::SecretQuestionDeclined { .. } => {
                self.approvals
                    .push(String::from("secret question declined"));
            }
            ThreadEvent::AgentDelta { text, .. } => self.streamed.push_str(&text),
            ThreadEvent::ReasoningDelta { text, .. } => self.thought.push_str(&text),
            ThreadEvent::Reasoning { text, .. } => self.reasoning.push(text),
            ThreadEvent::TurnCompleted { outcome, .. } => self.outcome = Some(outcome),
            ThreadEvent::AgentMessage { text, .. } => {
                self.text = match self.text.as_str() {
                    "" => text,
                    earlier => format!("{earlier}\n{text}"),
                };
            }
            ThreadEvent::Item { phase, item, .. } => self.items.push(item_label(item, phase)),
            ThreadEvent::ApprovalRequested {
                id, kind, options, ..
            } => {
                self.approvals.push(approval_label(&id, kind, &options));
                self.answered.push(id);
            }
            ThreadEvent::ApprovalCancelled { .. } => {
                self.cancelled = self.cancelled.saturating_add(1);
            }
        }
    }
}

/// The turn an event names, if it names one.
fn event_turn(event: &ThreadEvent) -> Option<&TurnId> {
    match event {
        ThreadEvent::TurnStarted { turn }
        | ThreadEvent::TurnCompleted { turn, .. }
        | ThreadEvent::AgentDelta { turn, .. }
        | ThreadEvent::AgentMessage { turn, .. }
        | ThreadEvent::ReasoningDelta { turn, .. }
        | ThreadEvent::Reasoning { turn, .. }
        | ThreadEvent::Item { turn, .. }
        | ThreadEvent::ApprovalRequested { turn, .. }
        | ThreadEvent::SecretQuestionDeclined { turn } => Some(turn),
        ThreadEvent::Started | ThreadEvent::ApprovalCancelled { .. } | ThreadEvent::Unknown(_) => {
            None
        }
    }
}

fn item_label(item: Item, phase: agents::codex::ItemPhase) -> String {
    match item {
        Item::Command { status, .. } => format!("{phase:?} command {status}"),
        Item::FileChange {
            changes, status, ..
        } => format!(
            "{phase:?} fileChange {:?} {status}",
            changes.iter().map(|c| &c.path).collect::<Vec<&String>>()
        ),
    }
}

fn approval_label(id: &ApprovalId, kind: ApprovalKind, options: &[Value]) -> String {
    let what = match kind {
        ApprovalKind::Command { command, .. } => format!("command {command:?}"),
        ApprovalKind::FileChange { reason, .. } => format!("fileChange {reason:?}"),
        ApprovalKind::Permissions { asks, .. } => format!("permissions {asks:?}"),
        ApprovalKind::UserInput { questions } => format!("userInput {questions:?}"),
    };
    // An option is a bare string or a one-key object such as
    // `{"acceptWithExecpolicyAmendment": ...}`.
    let names = options
        .iter()
        .map(|o| match o {
            Value::String(s) => s.clone(),
            Value::Object(m) => m.keys().cloned().collect::<Vec<String>>().join("+"),
            other => other.to_string(),
        })
        .collect::<Vec<String>>();
    format!("{} {what} options=[{}]", id.0, names.join(", "))
}

/// Turns on one or more threads, driven until every one completes.
struct Run {
    live: Live,
    plan: Plan,
    turns: HashMap<ThreadId, Turn>,
    running: usize,
    /// What `interrupt()` returned, once the plan called it.
    interrupted: Option<Result<(), Error>>,
    /// How the app-server exited, under `Plan::Kill`.
    exited: Option<String>,
}

impl Run {
    async fn drive(live: Live, plan: Plan, turns: Vec<Turn>) -> Result<Run> {
        let first = turns.first().map(|turn| turn.thread.id.clone());
        let mut run = Run {
            live,
            plan,
            running: turns.len(),
            turns: turns
                .into_iter()
                .map(|turn| (turn.thread.id.clone(), turn))
                .collect(),
            interrupted: None,
            exited: None,
        };
        if let Some(thread) = first {
            run.cue(&thread, Cue::Send).await?;
        }
        run.run().await
    }

    async fn run(mut self) -> Result<Run> {
        loop {
            let event = self.next().await?;
            match self.on(event).await? {
                Progress::Playing => {}
                Progress::Done => break Ok(self),
            }
        }
    }

    async fn next(&mut self) -> Result<CodexEvent> {
        tokio::time::timeout(TURN_TIMEOUT, self.live.events.recv())
            .await
            .context("turn timed out")?
            .context("event stream closed")
    }

    async fn on(&mut self, event: CodexEvent) -> Result<Progress> {
        match event {
            CodexEvent::Thread { thread, event } => {
                let turn = self.turns.get(&thread).with_context(|| {
                    format!("event for a thread with no turn running: {}", thread.0)
                })?;
                match event_turn(&event) {
                    Some(got) if got != &turn.id => Err(anyhow::Error::msg(format!(
                        "event for turn {} during turn {}",
                        got.0, turn.id.0
                    ))),
                    Some(_) | None => Ok(()),
                }?;
                self.react(&thread, &event).await?;
                self.turns
                    .get_mut(&thread)
                    .context("turn left the run while reacting")?
                    .record(event);
                Ok(match self.running {
                    0 => Progress::Done,
                    _ => Progress::Playing,
                })
            }
            CodexEvent::Exited { status } => match self.plan {
                Plan::Kill(_) => {
                    self.exited = Some(status.describe());
                    Ok(Progress::Done)
                }
                Plan::Answer(_) | Plan::Interrupt(_) => Err(anyhow::Error::msg(format!(
                    "codex exited mid-turn: {}",
                    status.describe()
                ))),
            },
            CodexEvent::Unknown(_) => Ok(Progress::Playing),
        }
    }

    /// Does what the plan asks of an event before its turn records it.
    async fn react(&mut self, thread: &ThreadId, event: &ThreadEvent) -> Result<()> {
        match event {
            ThreadEvent::Started
            | ThreadEvent::Unknown(_)
            | ThreadEvent::AgentMessage { .. }
            | ThreadEvent::ReasoningDelta { .. }
            | ThreadEvent::Reasoning { .. }
            | ThreadEvent::Item { .. }
            | ThreadEvent::ApprovalCancelled { .. }
            | ThreadEvent::SecretQuestionDeclined { .. } => {}
            ThreadEvent::TurnStarted { .. } => self.cue(thread, Cue::Start).await?,
            ThreadEvent::AgentDelta { .. } => self.cue(thread, Cue::Delta).await?,
            ThreadEvent::TurnCompleted { .. } => self.running = self.running.saturating_sub(1),
            ThreadEvent::ApprovalRequested { id, kind, .. } => match &self.plan {
                Plan::Interrupt(Cue::Approval) => self.cue(thread, Cue::Approval).await?,
                Plan::Answer(policy) => {
                    let decision = policy.decision(kind);
                    self.live.server.respond(id.clone(), decision).await?;
                }
                Plan::Interrupt(_) | Plan::Kill(_) => {
                    self.live
                        .server
                        .respond(id.clone(), Decision::Decline)
                        .await?;
                }
            },
        }
        Ok(())
    }

    /// Interrupts once, if the plan waits for this cue.
    async fn cue(&mut self, thread: &ThreadId, cue: Cue) -> Result<()> {
        let turn = self
            .turns
            .get(thread)
            .context("turn left the run while reacting")?;
        match (&self.plan, cue, &self.interrupted) {
            (Plan::Interrupt(Cue::Send), Cue::Send, None)
            | (Plan::Interrupt(Cue::Start), Cue::Start, None)
            | (Plan::Interrupt(Cue::Delta), Cue::Delta, None)
            | (Plan::Interrupt(Cue::Approval), Cue::Approval, None) => {
                self.interrupted = Some(turn.thread.interrupt(turn.id.clone()).await);
            }
            (Plan::Kill(Cue::Delta), Cue::Delta, None) => {
                self.interrupted = Some(Ok(()));
                kill(self.live.server.pid());
            }
            _ => {}
        }
        Ok(())
    }
}

/// A run of one turn, taken apart.
struct Single {
    live: Live,
    turn: Turn,
    interrupted: Option<Result<(), Error>>,
    exited: Option<String>,
}

async fn single(live: Live, thread: &CodexThread, prompt: &str, plan: Plan) -> Result<Single> {
    let id = thread.send_turn(String::from(prompt)).await?;
    let Run {
        live,
        turns,
        interrupted,
        plan: _,
        running: _,
        exited,
    } = Run::drive(live, plan, Vec::from([Turn::new(thread.clone(), id)])).await?;
    let turn = turns
        .into_values()
        .next()
        .context("a run of one turn lost its turn")?;
    Ok(Single {
        live,
        turn,
        interrupted,
        exited,
    })
}

/// Kills the process as a crash would, without closing its stdin.
fn kill(pid: Option<u32>) {
    let killed = pid.map(|pid| {
        std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
    });
    tracing::info!(?pid, ?killed, "killed codex");
}

/// The effort steps 10 to 12 choose, other than the other steps' `low`.
const CHOSEN_EFFORT: Effort = Effort::High;

/// Reasoning text with its whitespace removed. Codex streams the summary
/// parts without the blank line `Reasoning` joins them with.
fn squeezed(text: &str) -> String {
    text.split_whitespace().collect()
}

/// Steps 10 to 12, on a fourth process: a thread started with a chosen
/// model and effort, which Codex reports back; a turn that reasons; and the
/// app-server killed in the middle of a turn, with the thread resumed in a
/// new process that remembers the first turn.
async fn chosen_and_killed(
    codex: &Path,
    fixtures: Option<&Path>,
    thread_opts: ThreadOptions,
) -> Result<(Verdict, Verdict, Verdict)> {
    let chosen = ThreadOptions {
        effort: Some(CHOSEN_EFFORT),
        ..thread_opts
    };
    let log = log_path(fixtures, "server4-chosen-killed");
    let live = spawn(codex, log).await?;
    let a = live.server.start_thread(chosen.clone()).await?;
    let model = report(
        "10 chosen model and effort",
        match (&a.settings.model, &chosen.model, a.settings.effort) {
            (got, Some(asked), Some(Effort::High)) if got == asked => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "asked {:?} {:?}; codex runs {:?}",
            chosen.model, chosen.effort, a.settings
        ),
    )?;
    let s = single(
        live,
        &a,
        "My codeword is quince. Is 1000003 prime? Think it through carefully, then answer yes or no, and use no tools.",
        Plan::Answer(Policy::Decline),
    )
    .await?;
    let t = &s.turn;
    let reasoning = report(
        "11 reasoning",
        match (squeezed(&t.reasoning.concat()), &t.outcome) {
            (joined, Some(TurnOutcome::Completed)) if joined == squeezed(&t.thought) => {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        format!(
            "reasoning items={:?} streamed={:?} reply={:?}",
            t.reasoning, t.thought, t.text
        ),
    )?;
    let killed = single(
        s.live,
        &a,
        "Count from 1 to 300, one number per line.",
        Plan::Kill(Cue::Delta),
    )
    .await?;
    let live = spawn(codex, log_path(fixtures, "server5-resumed")).await?;
    let resumed = live.server.resume_thread(a.id.clone(), chosen).await?;
    let s = single(
        live,
        &resumed,
        "What is my codeword? Answer with just the word and use no tools.",
        Plan::Answer(Policy::Decline),
    )
    .await?;
    let resume = report(
        "12 killed and resumed",
        match (
            &killed.exited,
            s.turn.text.to_lowercase().find("quince"),
            &s.turn.outcome,
        ) {
            (Some(_), Some(_), Some(TurnOutcome::Completed)) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "killed after {:?} exit={:?}; resumed {} -> {} reply={:?}",
            killed.turn.streamed.lines().last(),
            killed.exited,
            a.id.0,
            resumed.id.0,
            s.turn.text
        ),
    )?;
    let status = s.live.server.shutdown().await?;
    writeln!(std::io::stdout(), "exit status {}", status.describe())?;
    Ok((model, reasoning, resume))
}

/// What an interrupt at a cue does to the turn.
enum Expect {
    Interrupted,
    /// An interrupt sent right after `turn/start` races `turn/started`.
    /// Before it, Codex refuses (`no active turn to interrupt`) and the
    /// turn runs to the end; after it, the turn is interrupted.
    Either,
}

/// Interrupts a counting turn at `cue`, then checks what the interrupt did
/// and that the next turn works.
async fn interrupted(
    live: Live,
    thread: &CodexThread,
    step: &str,
    cue: Cue,
    expect: Expect,
) -> Result<(Live, Verdict)> {
    let s = single(
        live,
        thread,
        "Count from 1 to 300, one number per line.",
        Plan::Interrupt(cue),
    )
    .await?;
    let interrupted = s
        .interrupted
        .as_ref()
        .context("turn ended before the interrupt cue")?;
    let t = &s.turn;
    let after = single(
        s.live,
        thread,
        "Reply with just the word pong.",
        Plan::Answer(Policy::Decline),
    )
    .await?;
    let verdict = report(
        step,
        match (
            expect,
            interrupted,
            &t.outcome,
            &after.turn.outcome,
            after.turn.text.to_lowercase().find("pong"),
        ) {
            (
                Expect::Interrupted | Expect::Either,
                Ok(()),
                Some(TurnOutcome::Interrupted),
                Some(TurnOutcome::Completed),
                Some(_),
            )
            | (
                Expect::Either,
                Err(_),
                Some(TurnOutcome::Completed),
                Some(TurnOutcome::Completed),
                Some(_),
            ) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "interrupt()={interrupted:?} streamed={:?} output before the interrupt landed {:?} items={:?} outcome={:?}; next turn outcome={:?} reply={:?}",
            t.streamed, t.text, t.items, t.outcome, after.turn.outcome, after.turn.text
        ),
    )?;
    Ok((after.live, verdict))
}

/// Reads the server's events until Codex withdraws a request.
async fn until_withdrawn(events: &mut mpsc::Receiver<CodexEvent>) -> Option<ApprovalId> {
    loop {
        match events.recv().await {
            Some(CodexEvent::Thread {
                event: ThreadEvent::ApprovalCancelled { id },
                ..
            }) => break Some(id),
            Some(_) => {}
            None => break None,
        }
    }
}

async fn spawn(codex: &Path, stdout_log: Option<PathBuf>) -> Result<Live> {
    let (server, events) = CodexServer::spawn(ServerOptions {
        codex_path: PathBuf::from(codex),
        home: Home::User,
        credential: None,
        cwd: PathBuf::from(SCRATCH),
        stdout_log,
    })
    .await?;
    Ok(Live { server, events })
}

/// Steps 7 to 9, on a third process: a question answered, a command
/// accepted for the session and then run again without asking, and a
/// question withdrawn by an interrupt.
async fn questions(
    codex: &Path,
    scratch: &Path,
    fixtures: Option<&Path>,
    thread_opts: ThreadOptions,
) -> Result<(Verdict, Verdict, Verdict)> {
    let live = spawn(codex, log_path(fixtures, "server3-questions")).await?;
    let a = live.server.start_thread(thread_opts).await?;
    let s = single(
        live,
        &a,
        "Use the request_user_input tool to ask me one question: which color do I prefer, with the options Red and Blue. Then reply with only the color I chose.",
        Plan::Answer(Policy::Decline),
    )
    .await?;
    let t = &s.turn;
    let question = report(
        "7 question",
        match (
            t.approvals.len(),
            t.approvals
                .iter()
                .filter(|label| label.contains("userInput"))
                .count(),
            t.text.find("Blue"),
            &t.outcome,
        ) {
            (1, 1, Some(_), Some(TurnOutcome::Completed)) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "approvals={:?} items={:?} outcome={:?} reply={:?}",
            t.approvals, t.items, t.outcome, t.text
        ),
    )?;

    let s = single(
        s.live,
        &a,
        "Run exactly this shell command and nothing else: touch t1.txt",
        Plan::Answer(Policy::ApproveForSession),
    )
    .await?;
    let first = s.turn.approvals.clone();
    let again = single(
        s.live,
        &a,
        "Run exactly this shell command again, and nothing else: touch t1.txt",
        Plan::Answer(Policy::Decline),
    )
    .await?;
    let for_session = report(
        "8 accept for session",
        match (
            first.len(),
            presence(&scratch.join("t1.txt")),
            again.turn.approvals.first(),
            &again.turn.outcome,
        ) {
            (1, Presence::Present, None, Some(TurnOutcome::Completed)) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "first approvals={first:?} t1.txt {:?}; again approvals={:?} items={:?} outcome={:?}",
            presence(&scratch.join("t1.txt")),
            again.turn.approvals,
            again.turn.items,
            again.turn.outcome
        ),
    )?;

    let s = single(
        again.live,
        &a,
        "Use the request_user_input tool to ask me one question: which fruit do I prefer, with the options Apple and Pear. Then reply with only the fruit I chose.",
        Plan::Interrupt(Cue::Approval),
    )
    .await?;
    let t = &s.turn;
    let Live { server, mut events } = s.live;
    let cancelled = tokio::time::timeout(TURN_TIMEOUT, until_withdrawn(&mut events)).await;
    let late = match t.answered.first() {
        Some(id) => Some(
            server
                .respond(
                    id.clone(),
                    Decision::Answer(Vec::from([Answer(Vec::from([String::from("Pear")]))])),
                )
                .await,
        ),
        None => None,
    };
    let withdrawn = report(
        "9 question withdrawn by interrupt",
        match (
            t.approvals
                .iter()
                .filter(|label| label.contains("userInput"))
                .count(),
            &s.interrupted,
            &cancelled,
            &late,
            &t.outcome,
        ) {
            (1, Some(Ok(())), Ok(Some(_)), Some(Err(_)), Some(TurnOutcome::Interrupted)) => {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        format!(
            "approvals={:?} interrupt()={:?} outcome={:?} after the turn={cancelled:?} respond-after={late:?}",
            t.approvals, s.interrupted, t.outcome
        ),
    )?;
    let status = server.shutdown().await?;
    writeln!(std::io::stdout(), "exit status {}", status.describe())?;
    Ok((question, for_session, withdrawn))
}

async fn run(fixtures: Option<PathBuf>) -> Result<()> {
    fresh_scratch(Path::new(SCRATCH))?;
    let scratch = Path::new(SCRATCH);
    fixtures.as_ref().map(std::fs::create_dir_all).transpose()?;
    let fixtures = fixtures.as_deref();

    let codex = install(
        Cli::Codex,
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(LIVE_DATA),
    )
    .await?;
    let live = spawn(&codex, log_path(fixtures, "server1")).await?;
    let models = live.server.list_models().await?;
    let model = models
        .choices
        .iter()
        .find_map(|m| m.description.strip_prefix("Fast").map(|_| m.id.0.clone()))
        .context("model/list offers no model described as fast")?;
    writeln!(
        std::io::stdout(),
        "codex {} (userAgent {:?}); model {model} from {:?}",
        live.server.codex_version().unwrap_or("?"),
        live.server.user_agent(),
        models
            .choices
            .iter()
            .map(|m| &m.id.0)
            .collect::<Vec<&String>>()
    )?;
    // untrusted asks before every command, even `pwd`, whatever the
    // model thinks of the sandbox, and workspace-write gives it no reason to
    // refuse the write.
    let thread_opts = ThreadOptions {
        cwd: PathBuf::from(SCRATCH),
        model: Some(model),
        effort: Some(Effort::Low),
        approval_policy: ApprovalPolicy::Untrusted,
        sandbox: Sandbox::WorkspaceWrite,
    };

    let a = live.server.start_thread(thread_opts.clone()).await?;
    let s = single(
        live,
        &a,
        "Create the file a.txt in the current directory containing exactly `hello from s2`. Do nothing else.",
        Plan::Answer(Policy::Approve),
    )
    .await?;
    let t = &s.turn;
    let a_txt = scratch.join("a.txt");
    let approve = report(
        "1 approve",
        match (t.approvals.first(), presence(&a_txt), &t.outcome) {
            (Some(_), Presence::Present, Some(TurnOutcome::Completed)) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "approvals={:?} items={:?} a.txt={:?} outcome={:?}",
            t.approvals,
            t.items,
            std::fs::read_to_string(&a_txt).ok(),
            t.outcome
        ),
    )?;
    let answered_in_1 = t.answered.clone();

    let s = single(
        s.live,
        &a,
        "Create the file b.txt in the current directory containing exactly `nope`. If the request is declined, do not retry; just say so.",
        Plan::Answer(Policy::Decline),
    )
    .await?;
    let t = &s.turn;
    let b_txt = scratch.join("b.txt");
    let decline = report(
        "2 decline",
        match (t.approvals.first(), presence(&b_txt), &t.outcome) {
            (Some(_), Presence::Missing, Some(TurnOutcome::Completed)) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "approvals={:?} items={:?} b.txt {:?} outcome={:?} reply={:?}",
            t.approvals,
            t.items,
            presence(&b_txt),
            t.outcome,
            t.text
        ),
    )?;

    let (live, interrupt) = interrupted(
        s.live,
        &a,
        "3 interrupt at start",
        Cue::Start,
        Expect::Interrupted,
    )
    .await?;
    let (live, mid_stream) = interrupted(
        live,
        &a,
        "3c interrupt mid-stream",
        Cue::Delta,
        Expect::Interrupted,
    )
    .await?;
    let (live, at_send) =
        interrupted(live, &a, "3d interrupt at send", Cue::Send, Expect::Either).await?;

    let s = single(
        live,
        &a,
        "Create the file d.txt in the current directory containing exactly `d`. Do nothing else.",
        Plan::Interrupt(Cue::Approval),
    )
    .await?;
    let t = &s.turn;
    // Codex withdraws the request only after the turn has completed.
    let Live { server, mut events } = s.live;
    let withdrawn = tokio::time::timeout(TURN_TIMEOUT, until_withdrawn(&mut events)).await;
    let late = match t.answered.first() {
        Some(id) => Some(server.respond(id.clone(), Decision::Accept).await),
        None => None,
    };
    let withdraw = report(
        "3b interrupt with an approval open",
        match (
            &s.interrupted,
            &withdrawn,
            &late,
            &t.outcome,
            presence(&scratch.join("d.txt")),
        ) {
            (
                Some(Ok(())),
                Ok(Some(_)),
                Some(Err(_)),
                Some(TurnOutcome::Interrupted),
                Presence::Missing,
            ) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "approvals={:?} interrupt()={:?} items={:?} outcome={:?} after the turn={withdrawn:?} respond-after={late:?}",
            t.approvals, s.interrupted, t.items, t.outcome
        ),
    )?;
    let live = Live { server, events };

    let b = live.server.start_thread(thread_opts.clone()).await?;
    let c = live.server.start_thread(thread_opts.clone()).await?;
    let (tb, tc) = n0_future::future::try_zip(
        b.send_turn(String::from(
            "Reply with just the word alpha. Use no tools.",
        )),
        c.send_turn(String::from(
            "Reply with just the word bravo. Use no tools.",
        )),
    )
    .await?;
    let both = Run::drive(
        live,
        Plan::Answer(Policy::Decline),
        Vec::from([
            Turn::new(b.clone(), tb.clone()),
            Turn::new(c.clone(), tc.clone()),
        ]),
    )
    .await?;
    let rb = both.turns.get(&b.id).context("no turn on thread b")?;
    let rc = both.turns.get(&c.id).context("no turn on thread c")?;
    let (b_text, c_text) = (rb.text.to_lowercase(), rc.text.to_lowercase());
    let two_threads = report(
        "4 two threads, one process",
        match (
            &rb.outcome,
            &rc.outcome,
            b_text.find("alpha"),
            b_text.find("bravo"),
            c_text.find("bravo"),
            c_text.find("alpha"),
        ) {
            (
                Some(TurnOutcome::Completed),
                Some(TurnOutcome::Completed),
                Some(_),
                None,
                Some(_),
                None,
            ) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "{} turn {} -> {:?} {:?}; {} turn {} -> {:?} {:?}",
            b.id.0, tb.0, rb.outcome, rb.text, c.id.0, tc.0, rc.outcome, rc.text
        ),
    )?;

    let status = both.live.server.shutdown().await?;
    let live = spawn(&codex, log_path(fixtures, "server2-resume")).await?;
    let resumed = live
        .server
        .resume_thread(a.id.clone(), thread_opts.clone())
        .await?;
    let s = single(
        live,
        &resumed,
        "Which file did you successfully create earlier in this conversation? Answer with just the filename and use no tools.",
        Plan::Answer(Policy::Decline),
    )
    .await?;
    let resume = report(
        "5 resume in a new process",
        match (&resumed.id, s.turn.text.find("a.txt"), &s.turn.outcome) {
            (id, Some(_), Some(TurnOutcome::Completed)) if id == &a.id => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "first exit={} resumed {} -> {} reply={:?}",
            status.describe(),
            a.id.0,
            resumed.id.0,
            s.turn.text
        ),
    )?;

    let server = &s.live.server;
    let bogus = server
        .respond(ApprovalId(String::from("never-sent")), Decision::Accept)
        .await;
    let again = n0_future::stream::iter(answered_in_1)
        .then(|id| server.respond(id, Decision::Accept))
        .collect::<Vec<Result<(), Error>>>()
        .await;
    let ids = report(
        "6 unknown and answered ids",
        match (
            &bogus,
            again.first(),
            again.iter().find_map(|answer| answer.as_ref().ok()),
        ) {
            (Err(_), Some(_), None) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!("respond(never-sent)={bogus:?}; re-answering step 1's ids={again:?}"),
    )?;

    let status = server.shutdown().await?;
    writeln!(std::io::stdout(), "exit status {}", status.describe())?;
    let (question, for_session, withdrawn) =
        questions(&codex, scratch, fixtures, thread_opts.clone()).await?;
    let (chosen, reasoning, resumed) = chosen_and_killed(&codex, fixtures, thread_opts).await?;
    let failed = Vec::from([
        approve,
        decline,
        interrupt,
        mid_stream,
        at_send,
        withdraw,
        two_threads,
        resume,
        ids,
        question,
        for_session,
        withdrawn,
        chosen,
        reasoning,
        resumed,
    ])
    .into_iter()
    .filter(|verdict| matches!(verdict, Verdict::Fail))
    .count();
    match failed {
        0 => Ok(()),
        failed => Err(anyhow::Error::msg(format!("{failed} step(s) failed"))),
    }
}
