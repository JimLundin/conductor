//! S1 live checklist: drives the real `claude` CLI through approve, deny,
//! interrupt, resume and set_permission_mode, printing PASS/FAIL per step.
//!
//! Usage: s1 [FIXTURE_DIR]
//! Writes raw CLI stdout per step to FIXTURE_DIR/<step>.jsonl when given.

use std::path::{Path, PathBuf};
use std::time::Duration;

use agents::claude::{
    Block, ClaudeEvent, ClaudeOptions, ClaudeSession, PermissionDecision, PermissionMode, Role,
    SessionId, SettingSource,
};
use anyhow::{Context, Result, bail};
use tokio::sync::mpsc;

const CLAUDE: &str = "/home/jim/.local/bin/claude";
const SCRATCH: &str = "/tmp/s1-scratch";
const TURN_TIMEOUT: Duration = Duration::from_secs(180);

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
    session: ClaudeSession,
    events: mpsc::Receiver<ClaudeEvent>,
}

/// How a turn answers permission requests.
#[derive(Clone, Copy)]
enum Policy<'a> {
    Approve,
    Deny(&'a str),
}

#[derive(Default, Debug)]
struct Turn {
    permissions: Vec<String>,
    cancelled_permissions: usize,
    text: String,
    finished: Option<(String, bool)>,
    terminal_reason: Option<String>,
    session_id: Option<SessionId>,
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
    let opts = |resume: Option<SessionId>, stdout_log| ClaudeOptions {
        claude_path: CLAUDE.into(),
        cwd: scratch.into(),
        model: Some("haiku".into()),
        permission_mode: PermissionMode::Default,
        resume,
        // Keep the user's own hooks and plugins out of the spike.
        setting_sources: Some(vec![SettingSource::Project, SettingSource::Local]),
        stdout_log,
    };
    let mut failures = 0;
    let mut report = |step: &str, ok: bool, detail: String| {
        println!("{} {step}: {detail}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            failures += 1;
        }
    };

    let (session, events) = ClaudeSession::spawn(opts(None, log("session1"))).await?;
    let mut live = Live { session, events };

    let t = turn(
        &mut live,
        "Use the Write tool to create the file a.txt in the current directory with the content `hello from s1`. Do nothing else.",
        Policy::Approve,
    )
    .await?;
    let a = scratch.join("a.txt");
    report(
        "1 approve",
        t.permissions == ["Write"] && a.exists() && t.finished == Some(("success".into(), false)),
        format!(
            "permissions={:?} a.txt exists={} contents={:?} finished={:?}",
            t.permissions,
            a.exists(),
            std::fs::read_to_string(&a).ok(),
            t.finished
        ),
    );
    let session_id = t.session_id.clone().context("no session id in turn 1")?;

    let t = turn(
        &mut live,
        "Use the Write tool to create the file b.txt in the current directory with the content `nope`. If it is denied, do not retry; just say so.",
        Policy::Deny("The user does not want b.txt."),
    )
    .await?;
    let b = scratch.join("b.txt");
    report(
        "2 deny",
        t.permissions.first().map(String::as_str) == Some("Write")
            && !b.exists()
            && t.finished.is_some(),
        format!(
            "permissions={:?} b.txt exists={} finished={:?} reply={:?}",
            t.permissions,
            b.exists(),
            t.finished,
            t.text
        ),
    );

    let (t, interrupted) = interrupted_turn(
        &mut live,
        "Say the word `starting` first. Then use the Bash tool to run `sleep 30 && echo slept`. Then count from 1 to 200, one number per line.",
    )
    .await?;
    let after = turn(
        &mut live,
        "Reply with just the word pong.",
        Policy::Deny("no tools"),
    )
    .await?;
    report(
        "3 interrupt",
        interrupted.is_ok()
            && t.finished.as_ref().is_some_and(|(_, is_error)| *is_error)
            && t.terminal_reason
                .as_deref()
                .is_some_and(|r| r.starts_with("aborted"))
            && after.finished == Some(("success".into(), false))
            && after.text.to_lowercase().contains("pong"),
        format!(
            "interrupt()={interrupted:?} permissions={:?} cancelled={} finished={:?} terminal_reason={:?}; next turn finished={:?} reply={:?}",
            t.permissions,
            t.cancelled_permissions,
            t.finished,
            t.terminal_reason,
            after.finished,
            after.text
        ),
    );

    live.session
        .send_user("Use the Write tool to create the file d.txt in the current directory with the content `late`. Do nothing else.".into())
        .await?;
    let mut t = Turn::default();
    let mut open = None;
    let mut interrupted = None;
    while t.finished.is_none() {
        match next(&mut live).await? {
            ClaudeEvent::PermissionRequested { id, tool_name, .. } if open.is_none() => {
                t.permissions.push(tool_name);
                open = Some(id);
                interrupted = Some(live.session.interrupt().await);
            }
            event => observe(&mut live, &mut t, event, Policy::Deny("late request")).await?,
        }
    }
    let late = match &open {
        Some(id) => Some(
            live.session
                .respond(id.clone(), PermissionDecision::Allow)
                .await,
        ),
        None => None,
    };
    report(
        "3b interrupt pending permission",
        matches!(interrupted, Some(Ok(())))
            && t.cancelled_permissions == 1
            && matches!(late, Some(Err(_)))
            && !scratch.join("d.txt").exists(),
        format!(
            "permissions={:?} interrupt()={interrupted:?} cancelled={} finished={:?} terminal_reason={:?} respond-after={late:?}",
            t.permissions, t.cancelled_permissions, t.finished, t.terminal_reason
        ),
    );

    let status = live.session.shutdown().await?;
    let (session, events) =
        ClaudeSession::spawn(opts(Some(session_id.clone()), log("session2-resume"))).await?;
    let mut live = Live { session, events };
    let t = turn(
        &mut live,
        "What file did you successfully create earlier in this conversation? Answer with just the filename.",
        Policy::Deny("no tools"),
    )
    .await?;
    report(
        "4 resume",
        t.text.contains("a.txt") && t.finished == Some(("success".into(), false)),
        format!(
            "first exit={status} resumed {} -> session_id={:?} reply={:?}",
            session_id.0,
            t.session_id.as_ref().map(|s| &s.0),
            t.text
        ),
    );

    let mode = live
        .session
        .set_permission_mode(PermissionMode::AcceptEdits)
        .await;
    let t = turn(
        &mut live,
        "Use the Write tool to create the file c.txt in the current directory with the content `accepted`. Do nothing else.",
        Policy::Deny("acceptEdits should not have asked"),
    )
    .await?;
    let c = scratch.join("c.txt");
    report(
        "5 acceptEdits",
        mode.is_ok() && t.permissions.is_empty() && c.exists(),
        format!(
            "set_permission_mode()={mode:?} permissions={:?} c.txt exists={} finished={:?}",
            t.permissions,
            c.exists(),
            t.finished
        ),
    );

    let bogus = live
        .session
        .respond(
            agents::claude::PermissionRequestId("never-sent".into()),
            PermissionDecision::Allow,
        )
        .await;
    report(
        "6 unknown id",
        bogus.is_err(),
        format!("respond(never-sent)={bogus:?}"),
    );

    let status = live.session.shutdown().await?;
    println!("exit status {status}");
    if failures > 0 {
        bail!("{failures} step(s) failed");
    }
    Ok(())
}

async fn turn(live: &mut Live, prompt: &str, policy: Policy<'_>) -> Result<Turn> {
    live.session.send_user(prompt.into()).await?;
    let mut t = Turn::default();
    while t.finished.is_none() {
        let event = next(live).await?;
        observe(live, &mut t, event, policy).await?;
    }
    Ok(t)
}

async fn interrupted_turn(
    live: &mut Live,
    prompt: &str,
) -> Result<(Turn, agents::claude::Result<()>)> {
    live.session.send_user(prompt.into()).await?;
    let mut t = Turn::default();
    let mut interrupted = None;
    while t.finished.is_none() {
        let event = next(live).await?;
        let is_assistant = matches!(
            event,
            ClaudeEvent::Message {
                role: Role::Assistant,
                ..
            }
        );
        observe(live, &mut t, event, Policy::Approve).await?;
        if is_assistant && interrupted.is_none() {
            interrupted = Some(live.session.interrupt().await);
        }
    }
    let interrupted = interrupted.context("turn ended before any assistant output")?;
    Ok((t, interrupted))
}

async fn next(live: &mut Live) -> Result<ClaudeEvent> {
    tokio::time::timeout(TURN_TIMEOUT, live.events.recv())
        .await
        .context("turn timed out")?
        .context("event stream closed")
}

async fn observe(
    live: &mut Live,
    t: &mut Turn,
    event: ClaudeEvent,
    policy: Policy<'_>,
) -> Result<()> {
    match event {
        ClaudeEvent::SessionStarted { session_id } => t.session_id = Some(session_id),
        ClaudeEvent::Message {
            role: Role::Assistant,
            content,
        } => {
            for block in content {
                if let Block::Text(text) = block {
                    t.text.push_str(&text);
                }
            }
        }
        ClaudeEvent::Message { .. } | ClaudeEvent::Unknown(_) => {}
        ClaudeEvent::PermissionRequested { id, tool_name, .. } => {
            t.permissions.push(tool_name);
            let decision = match policy {
                Policy::Approve => PermissionDecision::Allow,
                Policy::Deny(message) => PermissionDecision::Deny {
                    message: message.into(),
                    interrupt: false,
                },
            };
            if let Err(e) = live.session.respond(id.clone(), decision).await {
                println!("note: respond({id}) failed: {e}");
            }
        }
        ClaudeEvent::PermissionCancelled { .. } => t.cancelled_permissions += 1,
        ClaudeEvent::TurnFinished {
            subtype,
            is_error,
            terminal_reason,
            session_id,
        } => {
            t.finished = Some((subtype, is_error));
            t.terminal_reason = terminal_reason;
            t.session_id.get_or_insert(session_id);
        }
        ClaudeEvent::Exited { status } => bail!("claude exited mid-turn: {status}"),
    }
    Ok(())
}
