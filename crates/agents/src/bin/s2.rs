//! S2 live checklist: drives the real `codex app-server` through approve,
//! decline, interrupt, two concurrent threads on one process, resume in a new
//! process and an unknown approval id, printing PASS/FAIL per step.
//!
//! Usage: s2 [FIXTURE_DIR]
//! Writes raw app-server stdout per process to FIXTURE_DIR/<name>.jsonl when given.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agents::codex::{
    ApprovalId, ApprovalKind, ApprovalPolicy, CodexEvent, CodexServer, CodexThread, Decision,
    Error, Item, Sandbox, ServerOptions, ThreadEvent, ThreadId, ThreadOptions, TurnId, TurnOutcome,
};
use anyhow::{Context, Result};
use n0_future::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;

const CODEX: &str = "/home/jim/.local/bin/codex";
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
    Decline,
}

impl Policy {
    fn decision(self) -> Decision {
        match self {
            Policy::Approve => Decision::Accept,
            Policy::Decline => Decision::Decline,
        }
    }
}

/// How the turns of a run are driven.
enum Plan {
    Answer(Policy),
    /// Approve, and interrupt after the first agent message or item.
    InterruptOnFirstOutput,
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
            outcome: None,
        }
    }
}

fn pushed(list: Vec<String>, item: String) -> Vec<String> {
    list.into_iter().chain(std::iter::once(item)).collect()
}

/// The turn an event names, if it names one.
fn event_turn(event: &ThreadEvent) -> Option<&TurnId> {
    match event {
        ThreadEvent::TurnStarted { turn }
        | ThreadEvent::TurnCompleted { turn, .. }
        | ThreadEvent::AgentMessage { turn, .. }
        | ThreadEvent::Item { turn, .. }
        | ThreadEvent::ApprovalRequested { turn, .. } => Some(turn),
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
        ApprovalKind::Permissions { requested, .. } => format!("permissions {requested}"),
        ApprovalKind::UserInput { questions } => format!("userInput {questions}"),
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
}

impl Run {
    async fn drive(live: Live, plan: Plan, turns: Vec<Turn>) -> Result<Run> {
        Run {
            live,
            plan,
            running: turns.len(),
            turns: turns
                .into_iter()
                .map(|turn| (turn.thread.id.clone(), turn))
                .collect(),
            interrupted: None,
        }
        .run()
        .await
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
                let turn = self.turns.remove(&thread).with_context(|| {
                    format!("event for a thread with no turn running: {}", thread.0)
                })?;
                match event_turn(&event).and_then(|got| got.ne(&turn.id).then_some(got)) {
                    Some(got) => Err(anyhow::Error::msg(format!(
                        "event for turn {} during turn {}",
                        got.0, turn.id.0
                    ))),
                    None => Ok(()),
                }?;
                let turn = self.apply(turn, event).await?;
                self.turns.insert(thread, turn);
                Ok(match self.running {
                    0 => Progress::Done,
                    _ => Progress::Playing,
                })
            }
            CodexEvent::Exited { status } => Err(anyhow::Error::msg(format!(
                "codex exited mid-turn: {}",
                status.describe()
            ))),
            CodexEvent::Unknown(_) => Ok(Progress::Playing),
        }
    }

    async fn apply(&mut self, turn: Turn, event: ThreadEvent) -> Result<Turn> {
        match event {
            ThreadEvent::Started | ThreadEvent::Unknown(_) | ThreadEvent::TurnStarted { .. } => {
                Ok(turn)
            }
            ThreadEvent::TurnCompleted { outcome, .. } => {
                self.running = self.running.saturating_sub(1);
                Ok(Turn {
                    outcome: Some(outcome),
                    ..turn
                })
            }
            ThreadEvent::AgentMessage { text, .. } => {
                let text = match turn.text.as_str() {
                    "" => text,
                    earlier => format!("{earlier}\n{text}"),
                };
                self.after_output(&turn).await;
                Ok(Turn { text, ..turn })
            }
            ThreadEvent::Item { phase, item, .. } => {
                self.after_output(&turn).await;
                Ok(Turn {
                    items: pushed(turn.items, item_label(item, phase)),
                    ..turn
                })
            }
            ThreadEvent::ApprovalRequested {
                id, kind, options, ..
            } => {
                let label = approval_label(&id, kind, &options);
                let decision = match &self.plan {
                    Plan::Answer(policy) => policy.decision(),
                    Plan::InterruptOnFirstOutput => Decision::Accept,
                };
                self.live.server.respond(id.clone(), decision).await?;
                Ok(Turn {
                    approvals: pushed(turn.approvals, label),
                    answered: turn
                        .answered
                        .into_iter()
                        .chain(std::iter::once(id))
                        .collect(),
                    ..turn
                })
            }
            ThreadEvent::ApprovalCancelled { .. } => Ok(Turn {
                cancelled: turn.cancelled.saturating_add(1),
                ..turn
            }),
        }
    }

    async fn after_output(&mut self, turn: &Turn) {
        match (&self.plan, &self.interrupted) {
            (Plan::InterruptOnFirstOutput, None) => {
                self.interrupted = Some(turn.thread.interrupt(turn.id.clone()).await);
            }
            (Plan::InterruptOnFirstOutput, Some(_)) | (Plan::Answer(_), _) => {}
        }
    }
}

/// A run of one turn, taken apart.
struct Single {
    live: Live,
    turn: Turn,
    interrupted: Option<Result<(), Error>>,
}

async fn single(live: Live, thread: &CodexThread, prompt: &str, plan: Plan) -> Result<Single> {
    let id = thread.send_turn(String::from(prompt)).await?;
    let Run {
        live,
        turns,
        interrupted,
        plan: _,
        running: _,
    } = Run::drive(live, plan, Vec::from([Turn::new(thread.clone(), id)])).await?;
    let turn = turns
        .into_values()
        .next()
        .context("a run of one turn lost its turn")?;
    Ok(Single {
        live,
        turn,
        interrupted,
    })
}

#[derive(Debug, Clone, Copy)]
enum Presence {
    Present,
    Missing,
}

fn presence(path: &Path) -> Presence {
    match std::fs::metadata(path) {
        Ok(_) => Presence::Present,
        Err(_) => Presence::Missing,
    }
}

#[derive(Clone, Copy)]
enum Verdict {
    Pass,
    Fail,
}

fn report(step: &str, verdict: Verdict, detail: String) -> Result<Verdict> {
    let label = match verdict {
        Verdict::Pass => "PASS",
        Verdict::Fail => "FAIL",
    };
    writeln!(std::io::stdout(), "{label} {step}: {detail}")?;
    Ok(verdict)
}

fn log_path(fixtures: Option<&Path>, name: &str) -> Option<PathBuf> {
    fixtures.map(|dir| {
        let path = dir.join(format!("{name}.jsonl"));
        drop(std::fs::remove_file(&path));
        path
    })
}

async fn spawn(stdout_log: Option<PathBuf>) -> Result<Live> {
    let (server, events) = CodexServer::spawn(ServerOptions {
        codex_path: PathBuf::from(CODEX),
        cwd: PathBuf::from(SCRATCH),
        stdout_log,
    })
    .await?;
    Ok(Live { server, events })
}

fn fresh_scratch() -> Result<()> {
    let scratch = Path::new(SCRATCH);
    if let Presence::Present = presence(scratch) {
        std::fs::remove_dir_all(scratch)?;
    }
    std::fs::create_dir_all(scratch)?;
    let git = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(scratch)
        .status()?;
    match git.code() {
        Some(0) => Ok(()),
        _ => Err(anyhow::Error::msg(format!("git init failed in {SCRATCH}"))),
    }
}

async fn run(fixtures: Option<PathBuf>) -> Result<()> {
    fresh_scratch()?;
    let scratch = Path::new(SCRATCH);
    fixtures.as_ref().map(std::fs::create_dir_all).transpose()?;
    let fixtures = fixtures.as_deref();

    let live = spawn(log_path(fixtures, "server1")).await?;
    let models = live.server.list_models().await?;
    let model = models
        .iter()
        .find_map(|m| m.description.strip_prefix("Fast").map(|_| m.id.clone()))
        .context("model/list offers no model described as fast")?;
    writeln!(
        std::io::stdout(),
        "codex {} (userAgent {:?}); model {model} from {:?}",
        live.server.codex_version().unwrap_or("?"),
        live.server.user_agent(),
        models.iter().map(|m| &m.id).collect::<Vec<&String>>()
    )?;
    // on-request + workspace-write ran writes inside the workspace without
    // asking; read-only makes every write an approval request.
    let thread_opts = ThreadOptions {
        cwd: PathBuf::from(SCRATCH),
        model: Some(model),
        effort: Some(String::from("low")),
        approval_policy: ApprovalPolicy::OnRequest,
        sandbox: Sandbox::ReadOnly,
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

    let s = single(
        s.live,
        &a,
        "First say `starting`. Then run the shell command `sleep 60`. Then count from 1 to 300, one number per line.",
        Plan::InterruptOnFirstOutput,
    )
    .await?;
    let interrupted = s
        .interrupted
        .as_ref()
        .context("turn ended before any output")?;
    let t = &s.turn;
    let after = single(
        s.live,
        &a,
        "Reply with just the word pong.",
        Plan::Answer(Policy::Decline),
    )
    .await?;
    let interrupt = report(
        "3 interrupt",
        match (
            interrupted,
            &t.outcome,
            &after.turn.outcome,
            after.turn.text.to_lowercase().find("pong"),
        ) {
            (Ok(()), Some(TurnOutcome::Interrupted), Some(TurnOutcome::Completed), Some(_)) => {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        format!(
            "interrupt()={interrupted:?} after output {:?} outcome={:?}; next turn outcome={:?} reply={:?}",
            t.text, t.outcome, after.turn.outcome, after.turn.text
        ),
    )?;

    let live = after.live;
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
    let live = spawn(log_path(fixtures, "server2-resume")).await?;
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
        match (
            resumed.id.eq(&a.id).then_some(&resumed.id),
            s.turn.text.find("a.txt"),
            &s.turn.outcome,
        ) {
            (Some(_), Some(_), Some(TurnOutcome::Completed)) => Verdict::Pass,
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
    let failed = Vec::from([approve, decline, interrupt, two_threads, resume, ids])
        .into_iter()
        .filter(|verdict| matches!(verdict, Verdict::Fail))
        .count();
    match failed {
        0 => Ok(()),
        failed => Err(anyhow::Error::msg(format!("{failed} step(s) failed"))),
    }
}
