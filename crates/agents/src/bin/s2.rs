//! S2 live checklist: drives the real `codex app-server` through approve,
//! decline, interrupt, two concurrent threads on one process, resume in a new
//! process and an unknown approval id, printing PASS/FAIL per step.
//!
//! Usage: s2 [FIXTURE_DIR]
//! Writes raw app-server stdout per process to FIXTURE_DIR/<name>.jsonl when given.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agents::codex::{
    ApprovalId, ApprovalKind, ApprovalPolicy, CodexEvent, CodexServer, CodexThread, Decision, Item,
    Sandbox, ServerOptions, ThreadEvent, ThreadId, ThreadOptions, TurnId, TurnOutcome,
};
use anyhow::{Context, Result, bail};
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
#[derive(Clone, Copy, Debug)]
enum Policy {
    Approve,
    Decline,
}

#[derive(Default, Debug)]
struct Turn {
    approvals: Vec<String>,
    answered: Vec<ApprovalId>,
    cancelled: usize,
    items: Vec<String>,
    text: String,
    outcome: Option<TurnOutcome>,
}

async fn run(fixtures: Option<PathBuf>) -> Result<()> {
    let scratch = Path::new(SCRATCH);
    if scratch.exists() {
        std::fs::remove_dir_all(scratch)?;
    }
    std::fs::create_dir_all(scratch)?;
    let git = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(scratch)
        .status()?;
    if !git.success() {
        bail!("git init failed in {SCRATCH}");
    }
    if let Some(dir) = &fixtures {
        std::fs::create_dir_all(dir)?;
    }
    let log = |name: &str| {
        fixtures.as_ref().map(|dir| {
            let path = dir.join(format!("{name}.jsonl"));
            let _ = std::fs::remove_file(&path);
            path
        })
    };
    let server_opts = |stdout_log| ServerOptions {
        codex_path: CODEX.into(),
        cwd: scratch.into(),
        stdout_log,
    };
    let mut failures = 0;
    let mut report = |step: &str, ok: bool, detail: String| {
        println!("{} {step}: {detail}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            failures += 1;
        }
    };

    let (server, events) = CodexServer::spawn(server_opts(log("server1"))).await?;
    let mut live = Live { server, events };
    let models = live.server.list_models().await?;
    let model = models
        .iter()
        .find(|m| m.description.starts_with("Fast"))
        .context("model/list offers no model described as fast")?
        .id
        .clone();
    println!(
        "codex {} (userAgent {:?}); model {model} from {:?}",
        live.server.codex_version().unwrap_or("?"),
        live.server.user_agent(),
        models.iter().map(|m| &m.id).collect::<Vec<_>>()
    );
    // on-request + workspace-write ran writes inside the workspace without
    // asking; read-only makes every write an approval request.
    let thread_opts = ThreadOptions {
        cwd: scratch.into(),
        model: Some(model),
        effort: Some("low".into()),
        approval_policy: ApprovalPolicy::OnRequest,
        sandbox: Sandbox::ReadOnly,
    };

    let a = live.server.start_thread(thread_opts.clone()).await?;
    let t = turn(
        &mut live,
        &a,
        "Create the file a.txt in the current directory containing exactly `hello from s2`. Do nothing else.",
        Policy::Approve,
    )
    .await?;
    let a_txt = scratch.join("a.txt");
    report(
        "1 approve",
        !t.approvals.is_empty() && a_txt.exists() && t.outcome == Some(TurnOutcome::Completed),
        format!(
            "approvals={:?} items={:?} a.txt={:?} outcome={:?}",
            t.approvals,
            t.items,
            std::fs::read_to_string(&a_txt).ok(),
            t.outcome
        ),
    );
    let answered_in_1 = t.answered.clone();

    let t = turn(
        &mut live,
        &a,
        "Create the file b.txt in the current directory containing exactly `nope`. If the request is declined, do not retry; just say so.",
        Policy::Decline,
    )
    .await?;
    let b_txt = scratch.join("b.txt");
    report(
        "2 decline",
        !t.approvals.is_empty() && !b_txt.exists() && t.outcome == Some(TurnOutcome::Completed),
        format!(
            "approvals={:?} items={:?} b.txt exists={} outcome={:?} reply={:?}",
            t.approvals,
            t.items,
            b_txt.exists(),
            t.outcome,
            t.text
        ),
    );

    let (t, interrupted) = interrupted_turn(
        &mut live,
        &a,
        "First say `starting`. Then run the shell command `sleep 60`. Then count from 1 to 300, one number per line.",
    )
    .await?;
    let after = turn(
        &mut live,
        &a,
        "Reply with just the word pong.",
        Policy::Decline,
    )
    .await?;
    report(
        "3 interrupt",
        interrupted.is_ok()
            && t.outcome == Some(TurnOutcome::Interrupted)
            && after.outcome == Some(TurnOutcome::Completed)
            && after.text.to_lowercase().contains("pong"),
        format!(
            "interrupt()={interrupted:?} after output {:?} outcome={:?}; next turn outcome={:?} reply={:?}",
            t.text, t.outcome, after.outcome, after.text
        ),
    );

    let b = live.server.start_thread(thread_opts.clone()).await?;
    let c = live.server.start_thread(thread_opts.clone()).await?;
    let (tb, tc) = tokio::try_join!(
        b.send_turn("Reply with just the word alpha. Use no tools.".into()),
        c.send_turn("Reply with just the word bravo. Use no tools.".into()),
    )?;
    let both = concurrent(&mut live, [(&b, &tb), (&c, &tc)]).await?;
    let (rb, rc) = (&both[&b.id], &both[&c.id]);
    report(
        "4 two threads, one process",
        rb.outcome == Some(TurnOutcome::Completed)
            && rc.outcome == Some(TurnOutcome::Completed)
            && rb.text.to_lowercase().contains("alpha")
            && !rb.text.to_lowercase().contains("bravo")
            && rc.text.to_lowercase().contains("bravo")
            && !rc.text.to_lowercase().contains("alpha"),
        format!(
            "{} turn {} -> {:?} {:?}; {} turn {} -> {:?} {:?}",
            b.id.0, tb.0, rb.outcome, rb.text, c.id.0, tc.0, rc.outcome, rc.text
        ),
    );

    let status = live.server.shutdown().await?;
    let (server, events) = CodexServer::spawn(server_opts(log("server2-resume"))).await?;
    let mut live = Live { server, events };
    let resumed = live
        .server
        .resume_thread(a.id.clone(), thread_opts.clone())
        .await?;
    let t = turn(
        &mut live,
        &resumed,
        "Which file did you successfully create earlier in this conversation? Answer with just the filename and use no tools.",
        Policy::Decline,
    )
    .await?;
    report(
        "5 resume in a new process",
        resumed.id == a.id && t.text.contains("a.txt") && t.outcome == Some(TurnOutcome::Completed),
        format!(
            "first exit={status} resumed {} -> {} reply={:?}",
            a.id.0, resumed.id.0, t.text
        ),
    );

    let bogus = live
        .server
        .respond(ApprovalId("never-sent".into()), Decision::Accept)
        .await;
    let mut again = Vec::new();
    for id in answered_in_1 {
        again.push(live.server.respond(id, Decision::Accept).await);
    }
    report(
        "6 unknown and answered ids",
        bogus.is_err() && !again.is_empty() && again.iter().all(Result::is_err),
        format!("respond(never-sent)={bogus:?}; re-answering step 1's ids={again:?}"),
    );

    let status = live.server.shutdown().await?;
    println!("exit status {status}");
    if failures > 0 {
        bail!("{failures} step(s) failed");
    }
    Ok(())
}

async fn turn(live: &mut Live, thread: &CodexThread, prompt: &str, policy: Policy) -> Result<Turn> {
    let turn_id = thread.send_turn(prompt.into()).await?;
    let mut t = Turn::default();
    while t.outcome.is_none() {
        let event = own(live, &thread.id).await?;
        observe(live, &mut t, &turn_id, event, policy).await?;
    }
    Ok(t)
}

async fn interrupted_turn(
    live: &mut Live,
    thread: &CodexThread,
    prompt: &str,
) -> Result<(Turn, agents::codex::Result<()>)> {
    let turn_id = thread.send_turn(prompt.into()).await?;
    let mut t = Turn::default();
    let mut interrupted = None;
    while t.outcome.is_none() {
        let event = own(live, &thread.id).await?;
        let output = matches!(
            event,
            ThreadEvent::AgentMessage { .. } | ThreadEvent::Item { .. }
        );
        observe(live, &mut t, &turn_id, event, Policy::Approve).await?;
        if output && interrupted.is_none() {
            interrupted = Some(thread.interrupt(turn_id.clone()).await);
        }
    }
    let interrupted = interrupted.context("turn ended before any output")?;
    Ok((t, interrupted))
}

/// Runs turns on several threads at once and checks every event names the
/// thread and turn it belongs to.
async fn concurrent(
    live: &mut Live,
    turns: [(&CodexThread, &TurnId); 2],
) -> Result<HashMap<ThreadId, Turn>> {
    let mut results: HashMap<ThreadId, Turn> = turns
        .iter()
        .map(|(thread, _)| (thread.id.clone(), Turn::default()))
        .collect();
    while results.values().any(|t| t.outcome.is_none()) {
        let (thread, event) = match next(live).await? {
            CodexEvent::Thread { thread, event } => (thread, event),
            CodexEvent::Exited { status } => bail!("codex exited mid-turn: {status}"),
            CodexEvent::Unknown(_) => continue,
        };
        let Some((_, turn_id)) = turns.iter().find(|(t, _)| t.id == thread) else {
            bail!("event for a thread with no turn running: {}", thread.0);
        };
        let t = results.get_mut(&thread).expect("built from turns");
        observe(live, t, turn_id, event, Policy::Decline).await?;
    }
    Ok(results)
}

/// The next event for `thread`; any other thread's event here is a routing bug.
async fn own(live: &mut Live, thread: &ThreadId) -> Result<ThreadEvent> {
    loop {
        match next(live).await? {
            CodexEvent::Thread { thread: t, event } if &t == thread => return Ok(event),
            CodexEvent::Thread { thread: t, event } => {
                bail!("event for {} while only {} runs: {event:?}", t.0, thread.0)
            }
            CodexEvent::Exited { status } => bail!("codex exited mid-turn: {status}"),
            CodexEvent::Unknown(_) => {}
        }
    }
}

async fn next(live: &mut Live) -> Result<CodexEvent> {
    tokio::time::timeout(TURN_TIMEOUT, live.events.recv())
        .await
        .context("turn timed out")?
        .context("event stream closed")
}

async fn observe(
    live: &mut Live,
    t: &mut Turn,
    turn_id: &TurnId,
    event: ThreadEvent,
    policy: Policy,
) -> Result<()> {
    let check = |turn: &TurnId| {
        if turn != turn_id {
            bail!("event for turn {} during turn {}", turn.0, turn_id.0);
        }
        Ok(())
    };
    match event {
        ThreadEvent::Started | ThreadEvent::Unknown(_) => {}
        ThreadEvent::TurnStarted { turn } => check(&turn)?,
        ThreadEvent::TurnCompleted { turn, outcome } => {
            check(&turn)?;
            t.outcome = Some(outcome);
        }
        ThreadEvent::AgentMessage { turn, text } => {
            check(&turn)?;
            if !t.text.is_empty() {
                t.text.push('\n');
            }
            t.text.push_str(&text);
        }
        ThreadEvent::Item { turn, phase, item } => {
            check(&turn)?;
            t.items.push(match item {
                Item::Command { status, .. } => format!("{phase:?} command {status}"),
                Item::FileChange {
                    changes, status, ..
                } => format!(
                    "{phase:?} fileChange {:?} {status}",
                    changes.iter().map(|c| &c.path).collect::<Vec<_>>()
                ),
            });
        }
        ThreadEvent::ApprovalRequested {
            id,
            turn,
            kind,
            options,
        } => {
            check(&turn)?;
            let what = match kind {
                ApprovalKind::Command { command, .. } => format!("command {command:?}"),
                ApprovalKind::FileChange { reason, .. } => format!("fileChange {reason:?}"),
                ApprovalKind::Permissions { requested, .. } => format!("permissions {requested}"),
                ApprovalKind::UserInput { questions } => format!("userInput {questions}"),
            };
            // An option is a bare string or a one-key object such as
            // `{"acceptWithExecpolicyAmendment": ...}`.
            let names: Vec<String> = options
                .iter()
                .map(|o| match o {
                    Value::String(s) => s.clone(),
                    Value::Object(m) => m.keys().cloned().collect::<Vec<_>>().join("+"),
                    other => other.to_string(),
                })
                .collect();
            t.approvals
                .push(format!("{id} {what} options=[{}]", names.join(", ")));
            let decision = match policy {
                Policy::Approve => Decision::Accept,
                Policy::Decline => Decision::Decline,
            };
            live.server.respond(id.clone(), decision).await?;
            t.answered.push(id);
        }
        ThreadEvent::ApprovalCancelled { .. } => t.cancelled += 1,
    }
    Ok(())
}
