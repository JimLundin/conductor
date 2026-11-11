//! S1 live checklist: drives the real `claude` CLI through approve, deny,
//! interrupt, resume and set_permission_mode, printing PASS/FAIL per step.
//!
//! Usage: s1 [FIXTURE_DIR]
//! Writes raw CLI stdout per step to FIXTURE_DIR/<step>.jsonl when given.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agents::claude::{
    AfterDeny, Block, ClaudeEvent, ClaudeOptions, ClaudeSession, Error, Outcome,
    PermissionDecision, PermissionMode, PermissionRequestId, Role, SessionId, SettingSource,
    VersionPolicy,
};
use anyhow::{Context, Result};
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

/// How a turn answers permission requests.
enum Policy {
    Approve,
    Deny(String),
}

impl Policy {
    fn decision(&self) -> PermissionDecision {
        match self {
            Policy::Approve => PermissionDecision::Allow,
            Policy::Deny(message) => PermissionDecision::Deny {
                message: message.clone(),
                then: AfterDeny::Continue,
            },
        }
    }
}

/// How a turn is driven to its `result`.
enum Plan {
    Answer(Policy),
    /// Approve permissions and interrupt after the first assistant message.
    InterruptOnFirstReply,
    /// Leave the first permission request open and interrupt; deny any later one.
    InterruptOnPermission,
}

/// Whether a turn has reached its `result`.
enum Progress {
    Playing,
    Done,
}

#[derive(Debug)]
struct Finished {
    subtype: String,
    outcome: Outcome,
    terminal_reason: Option<String>,
}

impl Finished {
    fn terminal_reason(&self) -> Option<&str> {
        self.terminal_reason.as_deref()
    }
}

/// One turn being driven to its `result`, and what it saw on the way.
struct Turn {
    plan: Plan,
    permissions: Vec<String>,
    cancelled_permissions: usize,
    text: String,
    finished: Option<Finished>,
    session_id: Option<SessionId>,
    /// What `interrupt()` returned, once the plan called it.
    interrupted: Option<Result<(), Error>>,
    /// The permission request `InterruptOnPermission` left open.
    open: Option<PermissionRequestId>,
}

impl Turn {
    async fn start(
        session: &ClaudeSession,
        events: &mut mpsc::Receiver<ClaudeEvent>,
        prompt: &str,
        plan: Plan,
    ) -> Result<Turn> {
        session.send_user(String::from(prompt)).await?;
        Turn {
            plan,
            permissions: Vec::new(),
            cancelled_permissions: 0,
            text: String::new(),
            finished: None,
            session_id: None,
            interrupted: None,
            open: None,
        }
        .run(session, events)
        .await
    }

    async fn run(
        mut self,
        session: &ClaudeSession,
        events: &mut mpsc::Receiver<ClaudeEvent>,
    ) -> Result<Turn> {
        loop {
            let event = next(events).await?;
            match self.on(session, event).await? {
                Progress::Playing => {}
                Progress::Done => break Ok(self),
            }
        }
    }

    async fn on(&mut self, session: &ClaudeSession, event: ClaudeEvent) -> Result<Progress> {
        match event {
            ClaudeEvent::SessionStarted { session_id } => {
                self.session_id = Some(session_id);
                Ok(Progress::Playing)
            }
            ClaudeEvent::Message {
                role: Role::Assistant,
                content,
            } => {
                self.on_reply(session, content).await;
                Ok(Progress::Playing)
            }
            ClaudeEvent::Message { .. } | ClaudeEvent::Unknown(_) => Ok(Progress::Playing),
            ClaudeEvent::PermissionRequested { id, tool_name, .. } => {
                self.permissions.push(tool_name);
                self.on_permission(session, id).await?;
                Ok(Progress::Playing)
            }
            ClaudeEvent::PermissionCancelled { .. } => {
                self.cancelled_permissions = self.cancelled_permissions.saturating_add(1);
                Ok(Progress::Playing)
            }
            ClaudeEvent::TurnFinished {
                subtype,
                outcome,
                terminal_reason,
                session_id,
            } => {
                self.finished = Some(Finished {
                    subtype,
                    outcome,
                    terminal_reason,
                });
                self.session_id.get_or_insert(session_id);
                Ok(Progress::Done)
            }
            ClaudeEvent::Exited { status } => Err(anyhow::Error::msg(format!(
                "claude exited mid-turn: {status}"
            ))),
        }
    }

    async fn on_reply(&mut self, session: &ClaudeSession, content: Vec<Block>) {
        let text = content
            .into_iter()
            .filter_map(|block| match block {
                Block::Text(text) => Some(text),
                Block::Thinking(_)
                | Block::ToolUse { .. }
                | Block::ToolResult { .. }
                | Block::Other(_) => None,
            })
            .collect::<String>();
        self.text.push_str(&text);
        match (&self.plan, &self.interrupted) {
            (Plan::InterruptOnFirstReply, None) => {
                self.interrupted = Some(session.interrupt().await);
            }
            (Plan::InterruptOnFirstReply, Some(_))
            | (Plan::Answer(_) | Plan::InterruptOnPermission, _) => {}
        }
    }

    async fn on_permission(
        &mut self,
        session: &ClaudeSession,
        id: PermissionRequestId,
    ) -> Result<()> {
        match (&self.plan, &self.open) {
            (Plan::InterruptOnPermission, None) => {
                self.open = Some(id);
                self.interrupted = Some(session.interrupt().await);
                Ok(())
            }
            (Plan::InterruptOnPermission, Some(_)) => {
                let decision = PermissionDecision::Deny {
                    message: String::from("late request"),
                    then: AfterDeny::Continue,
                };
                answer(session, id, decision).await
            }
            (Plan::InterruptOnFirstReply, _) => {
                answer(session, id, PermissionDecision::Allow).await
            }
            (Plan::Answer(policy), _) => answer(session, id, policy.decision()).await,
        }
    }

    /// The turn's `result` subtype and outcome, for matching.
    fn result(&self) -> Option<(&str, Outcome)> {
        self.finished
            .as_ref()
            .map(|finished| (finished.subtype.as_str(), finished.outcome))
    }
}

async fn next(events: &mut mpsc::Receiver<ClaudeEvent>) -> Result<ClaudeEvent> {
    tokio::time::timeout(TURN_TIMEOUT, events.recv())
        .await
        .context("turn timed out")?
        .context("event stream closed")
}

async fn answer(
    session: &ClaudeSession,
    id: PermissionRequestId,
    decision: PermissionDecision,
) -> Result<()> {
    match session.respond(id.clone(), decision).await {
        Ok(()) => Ok(()),
        Err(e) => Ok(writeln!(
            std::io::stdout(),
            "note: respond({}) failed: {e}",
            id.0
        )?),
    }
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

fn options(resume: Option<SessionId>, stdout_log: Option<PathBuf>) -> ClaudeOptions {
    ClaudeOptions {
        claude_path: PathBuf::from(CLAUDE),
        cwd: PathBuf::from(SCRATCH),
        model: Some(String::from("haiku")),
        permission_mode: PermissionMode::Default,
        resume,
        // Keep the user's own hooks and plugins out of the spike.
        setting_sources: Some(Vec::from([SettingSource::Project, SettingSource::Local])),
        stdout_log,
        versions: VersionPolicy::SupportedOnly,
    }
}

fn fresh_scratch() -> Result<()> {
    let scratch = Path::new(SCRATCH);
    match presence(scratch) {
        Presence::Present => std::fs::remove_dir_all(scratch)?,
        Presence::Missing => {}
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

    let (session, mut events) =
        ClaudeSession::spawn(options(None, log_path(fixtures, "session1"))).await?;
    let t = Turn::start(
        &session,
        &mut events,
        "Use the Write tool to create the file a.txt in the current directory with the content `hello from s1`. Do nothing else.",
        Plan::Answer(Policy::Approve),
    )
    .await?;
    let a = scratch.join("a.txt");
    let approve = report(
        "1 approve",
        match (
            t.permissions.cmp(&Vec::from([String::from("Write")])),
            presence(&a),
            t.result(),
        ) {
            (
                std::cmp::Ordering::Equal,
                Presence::Present,
                Some(("success", Outcome::Succeeded)),
            ) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "permissions={:?} a.txt {:?} contents={:?} finished={:?}",
            t.permissions,
            presence(&a),
            std::fs::read_to_string(&a).ok(),
            t.finished
        ),
    )?;
    let session_id = t.session_id.clone().context("no session id in turn 1")?;

    let t = Turn::start(
        &session,
        &mut events,
        "Use the Write tool to create the file b.txt in the current directory with the content `nope`. If it is denied, do not retry; just say so.",
        Plan::Answer(Policy::Deny(String::from("The user does not want b.txt."))),
    )
    .await?;
    let b = scratch.join("b.txt");
    let deny = report(
        "2 deny",
        match (
            t.permissions.first().map(String::as_str),
            presence(&b),
            &t.finished,
        ) {
            (Some("Write"), Presence::Missing, Some(_)) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "permissions={:?} b.txt {:?} finished={:?} reply={:?}",
            t.permissions,
            presence(&b),
            t.finished,
            t.text
        ),
    )?;

    let t = Turn::start(
        &session,
        &mut events,
        "Say the word `starting` first. Then use the Bash tool to run `sleep 30 && echo slept`. Then count from 1 to 200, one number per line.",
        Plan::InterruptOnFirstReply,
    )
    .await?;
    let interrupted = t
        .interrupted
        .as_ref()
        .context("turn ended before any assistant output")?;
    let after = Turn::start(
        &session,
        &mut events,
        "Reply with just the word pong.",
        Plan::Answer(Policy::Deny(String::from("no tools"))),
    )
    .await?;
    let interrupt = report(
        "3 interrupt",
        match (
            interrupted,
            t.finished.as_ref().map(|finished| finished.outcome),
            t.finished
                .as_ref()
                .and_then(Finished::terminal_reason)
                .and_then(|reason| reason.strip_prefix("aborted")),
            after.result(),
            after.text.to_lowercase().find("pong"),
        ) {
            (
                Ok(()),
                Some(Outcome::Failed),
                Some(_),
                Some(("success", Outcome::Succeeded)),
                Some(_),
            ) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "interrupt()={interrupted:?} permissions={:?} cancelled={} finished={:?} terminal_reason={:?}; next turn finished={:?} reply={:?}",
            t.permissions,
            t.cancelled_permissions,
            t.finished,
            t.finished.as_ref().and_then(Finished::terminal_reason),
            after.finished,
            after.text
        ),
    )?;

    let t = Turn::start(
        &session,
        &mut events,
        "Use the Write tool to create the file d.txt in the current directory with the content `late`. Do nothing else.",
        Plan::InterruptOnPermission,
    )
    .await?;
    let late = match &t.open {
        Some(id) => Some(session.respond(id.clone(), PermissionDecision::Allow).await),
        None => None,
    };
    let pending = report(
        "3b interrupt pending permission",
        match (
            &t.interrupted,
            t.cancelled_permissions,
            &late,
            presence(&scratch.join("d.txt")),
        ) {
            (Some(Ok(())), 1, Some(Err(_)), Presence::Missing) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "permissions={:?} interrupt()={:?} cancelled={} finished={:?} terminal_reason={:?} respond-after={late:?}",
            t.permissions,
            t.interrupted,
            t.cancelled_permissions,
            t.finished,
            t.finished.as_ref().and_then(Finished::terminal_reason)
        ),
    )?;

    let status = session.shutdown().await?;
    let (session, mut events) = ClaudeSession::spawn(options(
        Some(session_id.clone()),
        log_path(fixtures, "session2-resume"),
    ))
    .await?;
    let t = Turn::start(
        &session,
        &mut events,
        "What file did you successfully create earlier in this conversation? Answer with just the filename.",
        Plan::Answer(Policy::Deny(String::from("no tools"))),
    )
    .await?;
    let resume = report(
        "4 resume",
        match (t.text.find("a.txt"), t.result()) {
            (Some(_), Some(("success", Outcome::Succeeded))) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "first exit={status} resumed {} -> session_id={:?} reply={:?}",
            session_id.0,
            t.session_id.as_ref().map(|id| &id.0),
            t.text
        ),
    )?;

    let mode = session
        .set_permission_mode(PermissionMode::AcceptEdits)
        .await;
    let t = Turn::start(
        &session,
        &mut events,
        "Use the Write tool to create the file c.txt in the current directory with the content `accepted`. Do nothing else.",
        Plan::Answer(Policy::Deny(String::from(
            "acceptEdits should not have asked",
        ))),
    )
    .await?;
    let c = scratch.join("c.txt");
    let accept_edits = report(
        "5 acceptEdits",
        match (&mode, t.permissions.first(), presence(&c)) {
            (Ok(()), None, Presence::Present) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "set_permission_mode()={mode:?} permissions={:?} c.txt {:?} finished={:?}",
            t.permissions,
            presence(&c),
            t.finished
        ),
    )?;

    let bogus = session
        .respond(
            PermissionRequestId(String::from("never-sent")),
            PermissionDecision::Allow,
        )
        .await;
    let unknown_id = report(
        "6 unknown id",
        match &bogus {
            Ok(()) => Verdict::Fail,
            Err(_) => Verdict::Pass,
        },
        format!("respond(never-sent)={bogus:?}"),
    )?;

    let status = session.shutdown().await?;
    writeln!(std::io::stdout(), "exit status {status}")?;
    let failed = Vec::from([
        approve,
        deny,
        interrupt,
        pending,
        resume,
        accept_edits,
        unknown_id,
    ])
    .into_iter()
    .fold(0_usize, |failed, verdict| match verdict {
        Verdict::Pass => failed,
        Verdict::Fail => failed.saturating_add(1),
    });
    match failed {
        0 => Ok(()),
        failed => Err(anyhow::Error::msg(format!("{failed} step(s) failed"))),
    }
}
