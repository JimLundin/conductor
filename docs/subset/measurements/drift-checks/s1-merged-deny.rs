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
use agents::live::{Presence, Verdict, fresh_scratch, log_path, presence, report};
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

struct Live {
    session: ClaudeSession,
    events: mpsc::Receiver<ClaudeEvent>,
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
    /// Interrupt as soon as the turn starts, before the model has said
    /// anything; deny any permission request.
    InterruptOnStart,
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
    live: Live,
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
    async fn start(live: Live, prompt: &str, plan: Plan) -> Result<Turn> {
        live.session.send_user(String::from(prompt)).await?;
        Turn {
            live,
            plan,
            permissions: Vec::new(),
            cancelled_permissions: 0,
            text: String::new(),
            finished: None,
            session_id: None,
            interrupted: None,
            open: None,
        }
        .run()
        .await
    }

    async fn run(mut self) -> Result<Turn> {
        loop {
            let event = self.next().await?;
            match self.on(event).await? {
                Progress::Playing => {}
                Progress::Done => break Ok(self),
            }
        }
    }

    async fn next(&mut self) -> Result<ClaudeEvent> {
        tokio::time::timeout(TURN_TIMEOUT, self.live.events.recv())
            .await
            .context("turn timed out")?
            .context("event stream closed")
    }

    async fn on(&mut self, event: ClaudeEvent) -> Result<Progress> {
        match event {
            ClaudeEvent::TurnStarted { session_id } => {
                self.session_id = Some(session_id);
                if let (Plan::InterruptOnStart, None) = (&self.plan, &self.interrupted) {
                    self.interrupted = Some(self.live.session.interrupt().await);
                }
                Ok(Progress::Playing)
            }
            ClaudeEvent::Message {
                role: Role::Assistant,
                content,
            } => {
                self.on_reply(content);
                Ok(Progress::Playing)
            }
            ClaudeEvent::Message { .. } | ClaudeEvent::Unknown(_) => Ok(Progress::Playing),
            ClaudeEvent::PermissionRequested { id, tool_name, .. } => {
                self.permissions.push(tool_name);
                self.on_permission(id).await?;
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
                "claude exited mid-turn: {}",
                status.describe()
            ))),
        }
    }

    fn on_reply(&mut self, content: Vec<Block>) {
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
    }

    async fn on_permission(&mut self, id: PermissionRequestId) -> Result<()> {
        match (&self.plan, &self.open) {
            (Plan::InterruptOnPermission, None) => {
                self.open = Some(id);
                self.interrupted = Some(self.live.session.interrupt().await);
                Ok(())
            }
            (Plan::InterruptOnPermission, Some(_)) | (Plan::InterruptOnStart, _) => {
                let decision = PermissionDecision::Deny {
                    message: String::from("not expected after an interrupt"),
                    then: AfterDeny::Continue,
                };
                self.answer(id, decision).await
            }
            (Plan::Answer(policy), _) => self.answer(id, policy.decision()).await,
        }
    }

    async fn answer(&self, id: PermissionRequestId, decision: PermissionDecision) -> Result<()> {
        match self.live.session.respond(id.clone(), decision).await {
            Ok(()) => Ok(()),
            Err(e) => Ok(writeln!(
                std::io::stdout(),
                "note: respond({}) failed: {e}",
                id.0
            )?),
        }
    }

    /// The turn's `result` subtype and outcome, for matching.
    fn result(&self) -> Option<(&str, Outcome)> {
        self.finished
            .as_ref()
            .map(|finished| (finished.subtype.as_str(), finished.outcome))
    }
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

async fn spawn(resume: Option<SessionId>, stdout_log: Option<PathBuf>) -> Result<Live> {
    let (session, events) = ClaudeSession::spawn(options(resume, stdout_log)).await?;
    Ok(Live { session, events })
}

async fn run(fixtures: Option<PathBuf>) -> Result<()> {
    fresh_scratch(Path::new(SCRATCH))?;
    let scratch = Path::new(SCRATCH);
    fixtures.as_ref().map(std::fs::create_dir_all).transpose()?;
    let fixtures = fixtures.as_deref();

    let live = spawn(None, log_path(fixtures, "session1")).await?;
    let t = Turn::start(
        live,
        "Use the Write tool to create the file a.txt in the current directory with the content `hello from s1`. Do nothing else.",
        Plan::Answer(Policy::Approve),
    )
    .await?;
    let a = scratch.join("a.txt");
    let approve = report(
        "1 approve",
        match (
            t.permissions
                .eq(&Vec::from([String::from("Write")]))
                .then_some(&t.permissions),
            presence(&a),
            t.result(),
        ) {
            (Some(_), Presence::Present, Some(("success", Outcome::Succeeded))) => Verdict::Pass,
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
        t.live,
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
        t.live,
        "Count from 1 to 200, one number per line.",
        Plan::InterruptOnStart,
    )
    .await?;
    let interrupted = t
        .interrupted
        .as_ref()
        .context("turn ended without starting")?;
    let after = Turn::start(
        t.live,
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
        after.live,
        "Use the Write tool to create the file d.txt in the current directory with the content `late`. Do nothing else.",
        Plan::InterruptOnPermission,
    )
    .await?;
    let late = match &t.open {
        Some(id) => Some(
            t.live
                .session
                .respond(id.clone(), PermissionDecision::Allow)
                .await,
        ),
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

    let status = t.live.session.shutdown().await?;
    let live = spawn(
        Some(session_id.clone()),
        log_path(fixtures, "session2-resume"),
    )
    .await?;
    let t = Turn::start(
        live,
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
            "first exit={} resumed {} -> session_id={:?} reply={:?}",
            status.describe(),
            session_id.0,
            t.session_id.as_ref().map(|id| &id.0),
            t.text
        ),
    )?;

    let mode = t
        .live
        .session
        .set_permission_mode(PermissionMode::AcceptEdits)
        .await;
    let t = Turn::start(
        t.live,
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

    let bogus = t
        .live
        .session
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

    let status = t.live.session.shutdown().await?;
    writeln!(std::io::stdout(), "exit status {}", status.describe())?;
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
    .filter(|verdict| matches!(verdict, Verdict::Fail))
    .count();
    match failed {
        0 => Ok(()),
        failed => Err(anyhow::Error::msg(format!("{failed} step(s) failed"))),
    }
}
