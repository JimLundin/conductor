//! S2 live checklist: drives the real `codex app-server` through approve,
//! decline, interrupt, interrupt with an approval open, two concurrent
//! threads on one process, resume in a new process, an unknown approval
//! id, questions and approval for the session, a chosen model and effort,
//! reasoning, a killed app-server resumed, a plan and the effort changed
//! between turns, printing PASS/FAIL per step.
//!
//! Usage: s2 [`FIXTURE_DIR`]
//! Writes raw app-server stdout per process to `FIXTURE_DIR`/<name>.jsonl when given.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agents::PlanStep;
use anyhow::{Context, Result};
use codex::{
    ApprovalId, ApprovalKind, ApprovalPolicy, CodexEvent, Decision, Sandbox,
    ThreadEvent, ThreadId, ThreadOptions, ThreadSettings, TurnId,
};
use codex_cli::{CodexServer, CodexThread, Error, ServerOptions};
use install::live::{
    Cue, Presence, Verdict, fresh_scratch, log_path, presence, report,
};
use install::{Cli, Home, install};
use n0_future::StreamExt;
use proto::{Answer, Effort, TaskStatus, TurnOutcome};
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
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn".into()),
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
            (Self::Approve, _) => Decision::Accept,
            (Self::ApproveForSession, _) => Decision::AcceptForSession,
            (Self::Decline, _) => Decision::Decline,
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
    /// Each plan the model wrote, in order.
    plans: Vec<Vec<PlanStep>>,
    /// What each `thread/settings/updated` reported.
    settings: Vec<ThreadSettings>,
    outcome: Option<TurnOutcome>,
}

impl Turn {
    const fn new(thread: CodexThread, id: TurnId) -> Self {
        Self {
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
            plans: Vec::new(),
            settings: Vec::new(),
            outcome: None,
        }
    }

    fn record(&mut self, event: ThreadEvent) {
        match event {
            ThreadEvent::Started
            | ThreadEvent::Unknown(_)
            | ThreadEvent::TurnStarted { .. } => {}
            ThreadEvent::SecretQuestionDeclined { .. } => {
                self.approvals
                    .push(String::from("secret question declined"));
            }
            ThreadEvent::AgentDelta { text, .. } => {
                self.streamed.push_str(&text);
            }
            ThreadEvent::ReasoningDelta { text, .. } => {
                self.thought.push_str(&text);
            }
            ThreadEvent::Reasoning { text, .. } => self.reasoning.push(text),
            ThreadEvent::TurnCompleted { outcome, .. } => {
                self.outcome = Some(outcome);
            }
            ThreadEvent::AgentMessage { text, .. } => {
                self.text = match self.text.as_str() {
                    "" => text,
                    earlier => format!("{earlier}\n{text}"),
                };
            }
            ThreadEvent::ToolCall { phase, .. } => {
                self.items.push(format!("{phase:?}"));
            }
            ThreadEvent::ApprovalRequested {
                id, kind, options, ..
            } => {
                self.approvals.push(approval_label(&id, kind, &options));
                self.answered.push(id);
            }
            ThreadEvent::ApprovalCancelled { .. } => {
                self.cancelled = self.cancelled.saturating_add(1);
            }
            ThreadEvent::PlanUpdated { steps, .. } => self.plans.push(steps),
            ThreadEvent::SettingsChanged(settings) => {
                self.settings.push(settings);
            }
        }
    }
}

/// The turn an event names, if it names one.
const fn event_turn(event: &ThreadEvent) -> Option<&TurnId> {
    match event {
        ThreadEvent::TurnStarted { turn }
        | ThreadEvent::TurnCompleted { turn, .. }
        | ThreadEvent::AgentDelta { turn, .. }
        | ThreadEvent::AgentMessage { turn, .. }
        | ThreadEvent::ReasoningDelta { turn, .. }
        | ThreadEvent::Reasoning { turn, .. }
        | ThreadEvent::ToolCall { turn, .. }
        | ThreadEvent::ApprovalRequested { turn, .. }
        | ThreadEvent::PlanUpdated { turn, .. }
        | ThreadEvent::SecretQuestionDeclined { turn } => Some(turn),
        ThreadEvent::Started
        | ThreadEvent::ApprovalCancelled { .. }
        | ThreadEvent::SettingsChanged(_)
        | ThreadEvent::Unknown(_) => None,
    }
}

fn approval_label(
    id: &ApprovalId,
    kind: ApprovalKind,
    options: &[Value],
) -> String {
    let what = match kind {
        ApprovalKind::Command { command, .. } => format!("command {command:?}"),
        ApprovalKind::FileChange { reason, .. } => {
            format!("fileChange {reason:?}")
        }
        ApprovalKind::Permissions { asks, .. } => {
            format!("permissions {asks:?}")
        }
        ApprovalKind::UserInput { questions } => {
            format!("userInput {questions:?}")
        }
    };
    // An option is a bare string or a one-key object such as
    // `{"acceptWithExecpolicyAmendment": ...}`.
    let names = options
        .iter()
        .map(|o| match o {
            Value::String(s) => s.clone(),
            Value::Object(m) => {
                m.keys().cloned().collect::<Vec<String>>().join("+")
            }
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
    async fn drive(live: Live, plan: Plan, turns: Vec<Turn>) -> Result<Self> {
        let first = turns.first().map(|turn| turn.thread.id.clone());
        let mut run = Self {
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

    async fn run(mut self) -> Result<Self> {
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

    async fn on(&mut self, received: CodexEvent) -> Result<Progress> {
        match received {
            CodexEvent::Thread { thread, event } => {
                let turn = self.turns.get(&thread).with_context(|| {
                    format!(
                        "event for a thread with no turn running: {}",
                        thread.0
                    )
                })?;
                match event_turn(&event) {
                    Some(got) if got != &turn.id => {
                        Err(anyhow::Error::msg(format!(
                            "event for turn {} during turn {}",
                            got.0, turn.id.0
                        )))
                    }
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
                Plan::Answer(_) | Plan::Interrupt(_) => {
                    Err(anyhow::Error::msg(format!(
                        "codex exited mid-turn: {}",
                        status.describe()
                    )))
                }
            },
            CodexEvent::Unknown(_) => Ok(Progress::Playing),
        }
    }

    /// Does what the plan asks of an event before its turn records it.
    async fn react(
        &mut self,
        thread: &ThreadId,
        event: &ThreadEvent,
    ) -> Result<()> {
        match event {
            ThreadEvent::Started
            | ThreadEvent::Unknown(_)
            | ThreadEvent::AgentMessage { .. }
            | ThreadEvent::ReasoningDelta { .. }
            | ThreadEvent::Reasoning { .. }
            | ThreadEvent::ToolCall { .. }
            | ThreadEvent::ApprovalCancelled { .. }
            | ThreadEvent::PlanUpdated { .. }
            | ThreadEvent::SettingsChanged(_)
            | ThreadEvent::SecretQuestionDeclined { .. } => {}
            ThreadEvent::TurnStarted { .. } => {
                self.cue(thread, Cue::Start).await?;
            }
            ThreadEvent::AgentDelta { .. } => {
                self.cue(thread, Cue::Delta).await?;
            }
            ThreadEvent::TurnCompleted { .. } => {
                self.running = self.running.saturating_sub(1);
            }
            ThreadEvent::ApprovalRequested { id, kind, .. } => match &self.plan
            {
                Plan::Interrupt(Cue::Approval) => {
                    self.cue(thread, Cue::Approval).await?;
                }
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
                self.interrupted =
                    Some(turn.thread.interrupt(turn.id.clone()).await);
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

async fn single(
    live: Live,
    thread: &CodexThread,
    prompt: &str,
    plan: Plan,
) -> Result<Single> {
    single_at(live, thread, prompt, None, plan).await
}

/// A run of one turn, started at `effort` if given.
async fn single_at(
    live: Live,
    thread: &CodexThread,
    prompt: &str,
    effort: Option<Effort>,
    plan: Plan,
) -> Result<Single> {
    let id = thread.send_turn(String::from(prompt), effort).await?;
    let Run {
        live: driven,
        turns,
        interrupted,
        plan: _,
        running: _,
        exited,
    } = Run::drive(live, plan, Vec::from([Turn::new(thread.clone(), id)]))
        .await?;
    let turn = turns
        .into_values()
        .next()
        .context("a run of one turn lost its turn")?;
    Ok(Single {
        live: driven,
        turn,
        interrupted,
        exited,
    })
}

/// Kills the process as a crash would, without closing its stdin.
fn kill(pid: Option<u32>) {
    let killed = pid.map(|id| {
        std::process::Command::new("kill")
            .args(["-9", &id.to_string()])
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
            (got, Some(asked), Some(Effort::High)) if got == asked => {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        &format!(
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
            (joined, Some(TurnOutcome::Completed))
                if joined == squeezed(&t.thought) =>
            {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        &format!(
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
        &format!(
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

/// The statuses of a plan's steps, in order.
fn statuses(steps: &[PlanStep]) -> Vec<TaskStatus> {
    steps.iter().map(|step| step.status).collect()
}

/// Steps 14 and 15, on a fifth process at the other steps' `low` effort: a
/// turn that keeps a plan with the plan tool, and a turn started at `high`,
/// which Codex then reports as the thread's effort.
async fn plan_and_effort(
    codex: &Path,
    fixtures: Option<&Path>,
    thread_opts: ThreadOptions,
) -> Result<(Verdict, Verdict)> {
    let live = spawn(codex, log_path(fixtures, "server8-plan-effort")).await?;
    let a = live.server.start_thread(thread_opts).await?;
    let planned = single(
        live,
        &a,
        "Use your plan tool (update_plan) to make a plan of exactly three steps: write a.txt, write b.txt, write c.txt. Then update the plan to mark the first step in progress, then update it again marking it completed. Do not run any commands or create files; only update the plan. Then reply done.",
        Plan::Answer(Policy::Decline),
    )
    .await?;
    let t = &planned.turn;
    let plan = report(
        "14 plan",
        match (
            t.plans.get(1),
            t.plans.last().map(|steps| statuses(steps)),
            t.approvals.first(),
            &t.outcome,
        ) {
            (Some(_), Some(last), None, Some(TurnOutcome::Completed))
                if last
                    == [
                        TaskStatus::Completed,
                        TaskStatus::Pending,
                        TaskStatus::Pending,
                    ] =>
            {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        &format!(
            "plans={:?} approvals={:?} items={:?} outcome={:?} reply={:?}",
            t.plans, t.approvals, t.items, t.outcome, t.text
        ),
    )?;
    let raised = single_at(
        planned.live,
        &a,
        "Reply with just the word pong. Use no tools.",
        Some(Effort::High),
        Plan::Answer(Policy::Decline),
    )
    .await?;
    let raised_turn = &raised.turn;
    let effort = report(
        "15 effort changed",
        match (
            raised_turn.settings.last().map(|settings| settings.effort),
            &raised_turn.outcome,
            raised_turn.text.to_lowercase().find("pong"),
        ) {
            (
                Some(Some(Effort::High)),
                Some(TurnOutcome::Completed),
                Some(_),
            ) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        &format!(
            "settings={:?} outcome={:?} reply={:?}",
            raised_turn.settings, raised_turn.outcome, raised_turn.text
        ),
    )?;
    let status = raised.live.server.shutdown().await?;
    writeln!(std::io::stdout(), "exit status {}", status.describe())?;
    Ok((plan, effort))
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
        &format!(
            "interrupt()={interrupted:?} streamed={:?} output before the interrupt landed {:?} items={:?} outcome={:?}; next turn outcome={:?} reply={:?}",
            t.streamed,
            t.text,
            t.items,
            t.outcome,
            after.turn.outcome,
            after.turn.text
        ),
    )?;
    Ok((after.live, verdict))
}

/// Reads the server's events until Codex withdraws a request.
async fn until_withdrawn(
    events: &mut mpsc::Receiver<CodexEvent>,
) -> Option<ApprovalId> {
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
    let (answered, question) = question_answered(live, &a).await?;
    let (accepted, for_session) =
        accepted_for_session(answered, &a, scratch).await?;
    let (after_withdrawal, withdrawn) =
        question_withdrawn(accepted, &a).await?;
    let status = after_withdrawal.server.shutdown().await?;
    writeln!(std::io::stdout(), "exit status {}", status.describe())?;
    Ok((question, for_session, withdrawn))
}

/// Step 7: a question answered.
async fn question_answered(
    live: Live,
    a: &CodexThread,
) -> Result<(Live, Verdict)> {
    let asked = single(
        live,
        a,
        "Use the request_user_input tool to ask me one question: which color do I prefer, with the options Red and Blue. Then reply with only the color I chose.",
        Plan::Answer(Policy::Decline),
    )
    .await?;
    let t = &asked.turn;
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
        &format!(
            "approvals={:?} items={:?} outcome={:?} reply={:?}",
            t.approvals, t.items, t.outcome, t.text
        ),
    )?;

    Ok((asked.live, question))
}

/// Step 8: a command accepted for the session, then run again without
/// asking.
async fn accepted_for_session(
    live: Live,
    a: &CodexThread,
    scratch: &Path,
) -> Result<(Live, Verdict)> {
    let accepted = single(
        live,
        a,
        "Run exactly this shell command and nothing else: touch t1.txt",
        Plan::Answer(Policy::ApproveForSession),
    )
    .await?;
    let first = accepted.turn.approvals.clone();
    let again = single(
        accepted.live,
        a,
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
            (1, Presence::Present, None, Some(TurnOutcome::Completed)) => {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        &format!(
            "first approvals={first:?} t1.txt {:?}; again approvals={:?} items={:?} outcome={:?}",
            presence(&scratch.join("t1.txt")),
            again.turn.approvals,
            again.turn.items,
            again.turn.outcome
        ),
    )?;

    Ok((again.live, for_session))
}

/// Step 9: a question withdrawn by an interrupt.
async fn question_withdrawn(
    live: Live,
    a: &CodexThread,
) -> Result<(Live, Verdict)> {
    let s = single(
        live,
        a,
        "Use the request_user_input tool to ask me one question: which fruit do I prefer, with the options Apple and Pear. Then reply with only the fruit I chose.",
        Plan::Interrupt(Cue::Approval),
    )
    .await?;
    let t = &s.turn;
    let Live { server, mut events } = s.live;
    let cancelled =
        tokio::time::timeout(TURN_TIMEOUT, until_withdrawn(&mut events)).await;
    let late = match t.answered.first() {
        Some(id) => Some(
            server
                .respond(
                    id.clone(),
                    Decision::Answer(Vec::from([Answer(Vec::from([
                        String::from("Pear"),
                    ]))])),
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
            (
                1,
                Some(Ok(())),
                Ok(Some(_)),
                Some(Err(_)),
                Some(TurnOutcome::Interrupted),
            ) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        &format!(
            "approvals={:?} interrupt()={:?} outcome={:?} after the turn={cancelled:?} respond-after={late:?}",
            t.approvals, s.interrupted, t.outcome
        ),
    )?;
    Ok((Live { server, events }, withdrawn))
}

/// A native revert removes the second turn, keeps the thread id, and stays
/// reverted after an app-server restart and resume.
async fn undo_history(
    codex: &Path,
    fixtures: Option<&Path>,
    opts: ThreadOptions,
) -> Result<Verdict> {
    let live =
        spawn(codex, log_path(fixtures, "server6-undo-original")).await?;
    let thread = live.server.start_thread(opts.clone()).await?;
    let first = single(
        live,
        &thread,
        "Remember my retained codeword is amber. Reply only amber. Do not use tools.",
        Plan::Answer(Policy::Approve),
    )
    .await?;
    let first_id = first.turn.id.clone();
    let second = single(
        first.live,
        &thread,
        "Remember my discarded codeword is cobalt. Reply only cobalt. Do not use tools.",
        Plan::Answer(Policy::Approve),
    )
    .await?;
    thread.revert(second.turn.id).await?;
    let retained = thread.history_turns().await?;
    let prompt = "What are my retained and discarded codewords? Reply only <retained>/<discarded>, using unknown for a codeword not supplied in this conversation. Do not use tools.";
    let after =
        single(second.live, &thread, prompt, Plan::Answer(Policy::Approve))
            .await?;
    let text = after.turn.text.clone();
    let expected = Vec::from([
        Vec::from([first_id.clone()]),
        Vec::from([first_id, after.turn.id.clone()]),
    ]);
    let _ = after.live.server.shutdown().await?;
    let live =
        spawn(codex, log_path(fixtures, "server7-undo-continued")).await?;
    let resumed = live.server.resume_thread(thread.id.clone(), opts).await?;
    let prefixes = Vec::from([retained, resumed.history_turns().await?]);
    let continued =
        single(live, &resumed, prompt, Plan::Answer(Policy::Approve)).await?;
    let verdict = report(
        "13 undo native history and Continue",
        match (
            text.trim(),
            continued.turn.text.trim(),
            &continued.turn.outcome,
            prefixes,
        ) {
            (
                "amber/unknown",
                "amber/unknown",
                Some(TurnOutcome::Completed),
                prefix,
            ) if prefix == expected => Verdict::Pass,
            _ => Verdict::Fail,
        },
        &format!(
            "thread={:?} resumed={:?} after={text:?} continued={:?}",
            thread.id, resumed.id, continued.turn.text
        ),
    )?;
    let _ = continued.live.server.shutdown().await?;
    Ok(verdict)
}

async fn run(fixtures_dir: Option<PathBuf>) -> Result<()> {
    fresh_scratch(Path::new(SCRATCH))?;
    let scratch = Path::new(SCRATCH);
    fixtures_dir
        .as_ref()
        .map(std::fs::create_dir_all)
        .transpose()?;
    let fixtures = fixtures_dir.as_deref();

    let codex = install(
        Cli::Codex,
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(LIVE_DATA),
    )
    .await?;
    let live = spawn(&codex, log_path(fixtures, "server1")).await?;
    let model = fast_model(&live).await?;
    // untrusted asks before every command, even `pwd`, whatever the
    // model thinks of the sandbox, and workspace-write gives it no reason to
    // refuse the write.
    let thread_opts = ThreadOptions {
        cwd: String::from(SCRATCH),
        model: Some(model),
        effort: Some(Effort::Low),
        approval_policy: ApprovalPolicy::Untrusted,
        sandbox: Sandbox::WorkspaceWrite,
    };

    let a = live.server.start_thread(thread_opts.clone()).await?;
    let (declined, answered_in_1, approve, decline) =
        approve_and_decline(live, &a, scratch).await?;
    let (after_start, interrupt) = interrupted(
        declined,
        &a,
        "3 interrupt at start",
        Cue::Start,
        Expect::Interrupted,
    )
    .await?;
    let (after_delta, mid_stream) = interrupted(
        after_start,
        &a,
        "3c interrupt mid-stream",
        Cue::Delta,
        Expect::Interrupted,
    )
    .await?;
    let (after_send, at_send) = interrupted(
        after_delta,
        &a,
        "3d interrupt at send",
        Cue::Send,
        Expect::Either,
    )
    .await?;
    let (live, withdraw) = approval_withdrawn(after_send, &a, scratch).await?;
    let (first_exit, two_threads) =
        two_threads_one_process(live, &thread_opts).await?;
    let (resume, ids) = resumed_in_a_new_process(
        &codex,
        fixtures,
        &a,
        thread_opts.clone(),
        &first_exit,
        answered_in_1,
    )
    .await?;
    let (question, for_session, withdrawn) =
        questions(&codex, scratch, fixtures, thread_opts.clone()).await?;
    let (chosen, reasoning, resumed) =
        chosen_and_killed(&codex, fixtures, thread_opts.clone()).await?;
    let undo = undo_history(&codex, fixtures, thread_opts.clone()).await?;
    let (plan, effort) = plan_and_effort(&codex, fixtures, thread_opts).await?;
    let failed = [
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
        undo,
        plan,
        effort,
    ]
    .into_iter()
    .filter(|verdict| matches!(verdict, Verdict::Fail))
    .count();
    match failed {
        0 => Ok(()),
        count => Err(anyhow::Error::msg(format!("{count} step(s) failed"))),
    }
}

/// The first model `model/list` describes as fast, after printing what
/// Codex this is and every model it offers.
async fn fast_model(live: &Live) -> Result<String> {
    let models = live.server.list_models().await?;
    let model = models
        .choices
        .iter()
        .find_map(|m| {
            m.description.strip_prefix("Fast").map(|_| m.id.0.clone())
        })
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
    Ok(model)
}

/// Steps 1 and 2: a write approved, then one declined. Returns the
/// approvals step 1 answered.
async fn approve_and_decline(
    live: Live,
    a: &CodexThread,
    scratch: &Path,
) -> Result<(Live, Vec<ApprovalId>, Verdict, Verdict)> {
    let approved = single(
        live,
        a,
        "Create the file a.txt in the current directory containing exactly `hello from s2`. Do nothing else.",
        Plan::Answer(Policy::Approve),
    )
    .await?;
    let t = &approved.turn;
    let a_txt = scratch.join("a.txt");
    let approve = report(
        "1 approve",
        match (t.approvals.first(), presence(&a_txt), &t.outcome) {
            (Some(_), Presence::Present, Some(TurnOutcome::Completed)) => {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        &format!(
            "approvals={:?} items={:?} a.txt={:?} outcome={:?}",
            t.approvals,
            t.items,
            std::fs::read_to_string(&a_txt).ok(),
            t.outcome
        ),
    )?;
    let answered_in_1 = t.answered.clone();

    let declined = single(
        approved.live,
        a,
        "Create the file b.txt in the current directory containing exactly `nope`. If the request is declined, do not retry; just say so.",
        Plan::Answer(Policy::Decline),
    )
    .await?;
    let declined_turn = &declined.turn;
    let b_txt = scratch.join("b.txt");
    let decline = report(
        "2 decline",
        match (
            declined_turn.approvals.first(),
            presence(&b_txt),
            &declined_turn.outcome,
        ) {
            (Some(_), Presence::Missing, Some(TurnOutcome::Completed)) => {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        &format!(
            "approvals={:?} items={:?} b.txt {:?} outcome={:?} reply={:?}",
            declined_turn.approvals,
            declined_turn.items,
            presence(&b_txt),
            declined_turn.outcome,
            declined_turn.text
        ),
    )?;

    Ok((declined.live, answered_in_1, approve, decline))
}

/// Step 3b: an interrupt with an approval open.
async fn approval_withdrawn(
    live: Live,
    a: &CodexThread,
    scratch: &Path,
) -> Result<(Live, Verdict)> {
    let s = single(
        live,
        a,
        "Create the file d.txt in the current directory containing exactly `d`. Do nothing else.",
        Plan::Interrupt(Cue::Approval),
    )
    .await?;
    let t = &s.turn;
    // Codex withdraws the request only after the turn has completed.
    let Live { server, mut events } = s.live;
    let withdrawn =
        tokio::time::timeout(TURN_TIMEOUT, until_withdrawn(&mut events)).await;
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
        &format!(
            "approvals={:?} interrupt()={:?} items={:?} outcome={:?} after the turn={withdrawn:?} respond-after={late:?}",
            t.approvals, s.interrupted, t.items, t.outcome
        ),
    )?;

    Ok((Live { server, events }, withdraw))
}

/// Step 4: turns on two threads of one process at once. Returns how the
/// process exited after it.
async fn two_threads_one_process(
    live: Live,
    thread_opts: &ThreadOptions,
) -> Result<(String, Verdict)> {
    let b = live.server.start_thread(thread_opts.clone()).await?;
    let c = live.server.start_thread(thread_opts.clone()).await?;
    let (tb, tc) = n0_future::future::try_zip(
        b.send_turn(
            String::from("Reply with just the word alpha. Use no tools."),
            None,
        ),
        c.send_turn(
            String::from("Reply with just the word bravo. Use no tools."),
            None,
        ),
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
    let on_b = both.turns.get(&b.id).context("no turn on thread b")?;
    let on_c = both.turns.get(&c.id).context("no turn on thread c")?;
    let (alpha_reply, bravo_reply) =
        (on_b.text.to_lowercase(), on_c.text.to_lowercase());
    let two_threads = report(
        "4 two threads, one process",
        match (
            &on_b.outcome,
            &on_c.outcome,
            alpha_reply.find("alpha"),
            alpha_reply.find("bravo"),
            bravo_reply.find("bravo"),
            bravo_reply.find("alpha"),
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
        &format!(
            "{} turn {} -> {:?} {:?}; {} turn {} -> {:?} {:?}",
            b.id.0,
            tb.0,
            on_b.outcome,
            on_b.text,
            c.id.0,
            tc.0,
            on_c.outcome,
            on_c.text
        ),
    )?;

    let status = both.live.server.shutdown().await?;
    Ok((status.describe(), two_threads))
}

/// Steps 5 and 6, on a second process: the thread resumed, then an
/// unknown approval id and step 1's answered ones.
async fn resumed_in_a_new_process(
    codex: &Path,
    fixtures: Option<&Path>,
    original: &CodexThread,
    thread_opts: ThreadOptions,
    first_exit: &str,
    answered_in_1: Vec<ApprovalId>,
) -> Result<(Verdict, Verdict)> {
    let live = spawn(codex, log_path(fixtures, "server2-resume")).await?;
    let resumed = live
        .server
        .resume_thread(original.id.clone(), thread_opts)
        .await?;
    let asked = single(
        live,
        &resumed,
        "Which file did you successfully create earlier in this conversation? Answer with just the filename and use no tools.",
        Plan::Answer(Policy::Decline),
    )
    .await?;
    let resume = report(
        "5 resume in a new process",
        match (
            &resumed.id,
            asked.turn.text.find("a.txt"),
            &asked.turn.outcome,
        ) {
            (id, Some(_), Some(TurnOutcome::Completed))
                if id == &original.id =>
            {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        &format!(
            "first exit={first_exit} resumed {} -> {} reply={:?}",
            original.id.0, resumed.id.0, asked.turn.text
        ),
    )?;

    let server = &asked.live.server;
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
        &format!(
            "respond(never-sent)={bogus:?}; re-answering step 1's ids={again:?}"
        ),
    )?;

    let status = server.shutdown().await?;
    writeln!(std::io::stdout(), "exit status {}", status.describe())?;
    Ok((resume, ids))
}
