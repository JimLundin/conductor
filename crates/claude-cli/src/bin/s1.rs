//! S1 live checklist: drives the real `claude` CLI through approve, deny,
//! interrupt, resume, `set_permission_mode`, questions, approval for the
//! session, a chosen model and effort, reasoning, a killed CLI resumed, a
//! checklist and the effort changed between turns, printing PASS/FAIL per
//! step.
//!
//! Usage: s1 [`FIXTURE_DIR`]
//! Writes raw CLI stdout per step to `FIXTURE_DIR`/<step>.jsonl when given.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agents::PlanStep;
use anyhow::{Context, Result};
use claude::{
    AfterDeny, Applied, Block, ClaudeEvent, MessageId, PermissionDecision,
    PermissionMode, PermissionRequestId, Role, SessionId,
};
use claude_cli::{
    ClaudeOptions, ClaudeSession, Error, Resume, SettingSource, VersionPolicy,
};
use install::live::{
    Cue, Presence, Verdict, fresh_scratch, log_path, presence, report,
};
use install::{Cli, Home, install};
use proto::{Answer, Effort, Question, TaskStatus, ToolOutcome, TurnOutcome};
use tokio::sync::mpsc;

/// Where the pinned CLIs are installed for the live checks, under this
/// checkout's `target/` rather than the `/tmp` tmpfs. They run with the
/// user's own homes, so the user's existing logins.
const LIVE_DATA: &str = "target/conductor-live";
const SCRATCH: &str = "/tmp/s1-scratch";
const TURN_TIMEOUT: Duration = Duration::from_secs(180);

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
    session: ClaudeSession,
    events: mpsc::Receiver<ClaudeEvent>,
}

/// How a turn answers permission requests.
enum Policy {
    Approve,
    ApproveForSession,
    Deny(String),
}

impl Policy {
    fn decision(&self) -> PermissionDecision {
        match self {
            Self::Approve => PermissionDecision::Allow,
            Self::ApproveForSession => PermissionDecision::AllowForSession,
            Self::Deny(message) => PermissionDecision::Deny {
                message: message.clone(),
                then: AfterDeny::Continue,
            },
        }
    }
}

/// How a turn is driven to its `result`.
enum Plan {
    Answer(Policy),
    /// Interrupt at the cue; deny any permission request.
    Interrupt(Cue),
    /// Leave the first permission request or question open and interrupt;
    /// deny any later one.
    InterruptOnPermission,
    /// Kill the CLI at the cue; the turn ends when it exits.
    Kill(Cue),
}

/// Whether a turn has reached its `result`.
enum Progress {
    Playing,
    Done,
}

#[derive(Debug)]
struct Finished {
    subtype: String,
    outcome: TurnOutcome,
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
    /// What each permission request offered to allow for the session.
    for_session: Vec<Option<String>>,
    questions: Vec<Vec<Question>>,
    /// The content of each failed tool result.
    tool_errors: Vec<String>,
    cancelled_permissions: usize,
    text: String,
    /// The text of every `StreamDelta`, joined.
    streamed: String,
    /// The text of each thinking block.
    thinking: Vec<String>,
    /// The text of every `ThinkingDelta`, joined.
    thought: String,
    /// Each `TodoWrite` checklist, in order.
    checklists: Vec<Vec<PlanStep>>,
    /// How the CLI exited, under `Plan::Kill`.
    exited: Option<String>,
    finished: Option<Finished>,
    session_id: Option<SessionId>,
    last_message: Option<MessageId>,
    /// What `interrupt()` returned, once the plan called it.
    interrupted: Option<Result<(), Error>>,
    /// The permission request `InterruptOnPermission` left open.
    open: Option<PermissionRequestId>,
}

impl Turn {
    async fn start(live: Live, prompt: &str, plan: Plan) -> Result<Self> {
        live.session.send_user(String::from(prompt)).await?;
        let mut turn = Self {
            live,
            plan,
            permissions: Vec::new(),
            for_session: Vec::new(),
            questions: Vec::new(),
            tool_errors: Vec::new(),
            cancelled_permissions: 0,
            text: String::new(),
            streamed: String::new(),
            thinking: Vec::new(),
            thought: String::new(),
            checklists: Vec::new(),
            exited: None,
            finished: None,
            session_id: None,
            last_message: None,
            interrupted: None,
            open: None,
        };
        turn.cue(Cue::Send).await;
        turn.run().await
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
                self.cue(Cue::Start).await;
                Ok(Progress::Playing)
            }
            ClaudeEvent::Message {
                role: Role::Assistant,
                content,
                ..
            } => {
                self.on_reply(content);
                Ok(Progress::Playing)
            }
            ClaudeEvent::StreamDelta { text } => {
                self.streamed.push_str(&text);
                self.cue(Cue::Delta).await;
                Ok(Progress::Playing)
            }
            ClaudeEvent::ThinkingDelta { text } => {
                self.thought.push_str(&text);
                Ok(Progress::Playing)
            }
            ClaudeEvent::Message {
                role: Role::User,
                content,
                ..
            } => {
                self.on_user(content);
                Ok(Progress::Playing)
            }
            ClaudeEvent::Unknown(_) => Ok(Progress::Playing),
            ClaudeEvent::PermissionRequested {
                id,
                tool_name,
                for_session,
                ..
            } => {
                self.permissions.push(tool_name);
                self.for_session.push(for_session);
                self.on_permission(id, None).await?;
                Ok(Progress::Playing)
            }
            ClaudeEvent::QuestionAsked { id, questions, .. } => {
                let answers = last_options(&questions);
                self.questions.push(questions);
                self.on_permission(id, Some(answers)).await?;
                Ok(Progress::Playing)
            }
            ClaudeEvent::PermissionCancelled { .. } => {
                self.cancelled_permissions =
                    self.cancelled_permissions.saturating_add(1);
                Ok(Progress::Playing)
            }
            ClaudeEvent::TurnFinished {
                subtype,
                outcome,
                terminal_reason,
                session_id,
                last_message,
                ..
            } => {
                self.last_message = last_message;
                self.finished = Some(Finished {
                    subtype,
                    outcome,
                    terminal_reason,
                });
                self.session_id.get_or_insert(session_id);
                Ok(Progress::Done)
            }
            ClaudeEvent::Exited { status } => match self.plan {
                Plan::Kill(_) => {
                    self.exited = Some(status.describe());
                    Ok(Progress::Done)
                }
                Plan::Answer(_)
                | Plan::Interrupt(_)
                | Plan::InterruptOnPermission => Err(anyhow::Error::msg(
                    format!("claude exited mid-turn: {}", status.describe()),
                )),
            },
        }
    }

    /// Interrupts once, if the plan waits for this cue.
    async fn cue(&mut self, cue: Cue) {
        match (&self.plan, cue, &self.interrupted) {
            (Plan::Interrupt(Cue::Send), Cue::Send, None)
            | (Plan::Interrupt(Cue::Start), Cue::Start, None)
            | (Plan::Interrupt(Cue::Delta), Cue::Delta, None) => {
                self.interrupted = Some(self.live.session.interrupt().await);
            }
            (Plan::Kill(Cue::Delta), Cue::Delta, None) => {
                self.interrupted = Some(Ok(()));
                kill(self.live.session.pid());
            }
            _ => {}
        }
    }

    fn on_reply(&mut self, content: Vec<Block>) {
        self.thinking
            .extend(content.iter().filter_map(|block| match block {
                Block::Thinking(text) => Some(text.clone()),
                Block::Text(_)
                | Block::ToolUse { .. }
                | Block::ToolResult { .. }
                | Block::Checklist(_)
                | Block::Other(_) => None,
            }));
        self.checklists.extend(content.iter().filter_map(
            |block| match block {
                Block::Checklist(steps) => Some(steps.clone()),
                Block::Text(_)
                | Block::Thinking(_)
                | Block::ToolUse { .. }
                | Block::ToolResult { .. }
                | Block::Other(_) => None,
            },
        ));
        let text = content
            .into_iter()
            .filter_map(|block| match block {
                Block::Text(text) => Some(text),
                Block::Thinking(_)
                | Block::ToolUse { .. }
                | Block::ToolResult { .. }
                | Block::Checklist(_)
                | Block::Other(_) => None,
            })
            .collect::<String>();
        self.text.push_str(&text);
    }

    fn on_user(&mut self, content: Vec<Block>) {
        self.tool_errors
            .extend(content.into_iter().filter_map(|block| match block {
                Block::ToolResult {
                    content,
                    outcome: ToolOutcome::Failed,
                    ..
                } => Some(content.to_string()),
                Block::ToolResult { .. }
                | Block::Text(_)
                | Block::Thinking(_)
                | Block::ToolUse { .. }
                | Block::Checklist(_)
                | Block::Other(_) => None,
            }));
    }

    /// Answers a permission request by the plan, or a question with
    /// `answers`.
    async fn on_permission(
        &mut self,
        id: PermissionRequestId,
        answers: Option<Vec<Answer>>,
    ) -> Result<()> {
        match (&self.plan, &self.open) {
            (Plan::InterruptOnPermission, None) => {
                self.open = Some(id);
                self.interrupted = Some(self.live.session.interrupt().await);
                Ok(())
            }
            (Plan::InterruptOnPermission, Some(_))
            | (Plan::Interrupt(_) | Plan::Kill(_), _) => {
                let decision = PermissionDecision::Deny {
                    message: String::from("not expected after an interrupt"),
                    then: AfterDeny::Continue,
                };
                self.answer(id, decision).await
            }
            (Plan::Answer(policy), _) => {
                let decision = answers.map_or_else(
                    || policy.decision(),
                    PermissionDecision::Answer,
                );
                self.answer(id, decision).await
            }
        }
    }

    async fn answer(
        &self,
        id: PermissionRequestId,
        decision: PermissionDecision,
    ) -> Result<()> {
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
    fn result(&self) -> Option<(&str, &TurnOutcome)> {
        self.finished
            .as_ref()
            .map(|finished| (finished.subtype.as_str(), &finished.outcome))
    }
}

/// A question is answered with each one's last option.
fn last_options(questions: &[Question]) -> Vec<Answer> {
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
        .collect()
}

fn options(
    claude: &Path,
    resume: Option<SessionId>,
    stdout_log: Option<PathBuf>,
) -> ClaudeOptions {
    ClaudeOptions {
        claude_path: PathBuf::from(claude),
        home: Home::User,
        credential: None,
        cwd: PathBuf::from(SCRATCH),
        model: Some(String::from("haiku")),
        effort: None,
        permission_mode: PermissionMode::Default,
        resume: resume.map_or(Resume::Fresh, Resume::Session),
        project_name: None,
        // Keep the user's own hooks and plugins out of the spike.
        setting_sources: Some(Vec::from([
            SettingSource::Project,
            SettingSource::Local,
        ])),
        stdout_log,
        versions: VersionPolicy::SupportedOnly,
    }
}

async fn spawn(
    claude: &Path,
    resume: Option<SessionId>,
    stdout_log: Option<PathBuf>,
) -> Result<Live> {
    let (session, events) =
        ClaudeSession::spawn(options(claude, resume, stdout_log)).await?;
    Ok(Live { session, events })
}

/// Kills the process as a crash would, without closing its stdin.
fn kill(pid: Option<u32>) {
    let killed = pid.map(|id| {
        std::process::Command::new("kill")
            .args(["-9", &id.to_string()])
            .status()
    });
    tracing::info!(?pid, ?killed, "killed claude");
}

/// The model and effort steps 11 to 13 choose, other than s1's default.
const CHOSEN_MODEL: &str = "sonnet";
const CHOSEN_EFFORT: Effort = Effort::High;
/// The model `get_settings` reports for [`CHOSEN_MODEL`].
const CHOSEN_RESOLVED: &str = "claude-sonnet-5-5";

/// Steps 11 to 13, in a fourth session started with a chosen model and
/// effort: the CLI reports them, a turn reasons, and the CLI killed in the
/// middle of a turn is resumed in a new process that remembers the first
/// turn.
async fn chosen_and_killed(
    claude: &Path,
    fixtures: Option<&Path>,
) -> Result<(Verdict, Verdict, Verdict)> {
    let chosen = |resume, stdout_log| ClaudeOptions {
        model: Some(String::from(CHOSEN_MODEL)),
        effort: Some(CHOSEN_EFFORT),
        ..options(claude, resume, stdout_log)
    };
    let log = log_path(fixtures, "session4-chosen-killed");
    let (session, events) = ClaudeSession::spawn(chosen(None, log)).await?;
    let applied = session.applied().await;
    let model = report(
        "11 chosen model and effort",
        match applied.as_ref().map(|settings| {
            (settings.model.as_str(), settings.effort.as_deref())
        }) {
            Ok((CHOSEN_RESOLVED, Some("high"))) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        &format!("applied={applied:?}"),
    )?;
    let t = Turn::start(
        Live { session, events },
        "A farmer has 17 sheep. All but 9 run away. He then buys twice as many as remain, and sells a third of the total. How many sheep does he have? Answer with just the number.",
        Plan::Answer(Policy::Deny(String::from("no tools"))),
    )
    .await?;
    let reasoning = report(
        "12 reasoning",
        match (t.thinking.concat(), t.thinking.first(), t.result()) {
            (joined, Some(_), Some(("success", TurnOutcome::Completed)))
                if joined == t.thought =>
            {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        &format!(
            "thinking blocks={} text={:?} streamed={:?} reply={:?}",
            t.thinking.len(),
            t.thinking,
            t.thought,
            t.text
        ),
    )?;
    let session_id =
        t.session_id.clone().context("no session id in step 12")?;
    let killed = Turn::start(
        t.live,
        "Count from 1 to 200, one number per line.",
        Plan::Kill(Cue::Delta),
    )
    .await?;
    let (session, events) = ClaudeSession::spawn(chosen(
        Some(session_id.clone()),
        log_path(fixtures, "session5-resumed"),
    ))
    .await?;
    let t = Turn::start(
        Live { session, events },
        "How many sheep did the farmer end up with in my puzzle earlier in this conversation? Answer with just the number.",
        Plan::Answer(Policy::Deny(String::from("no tools"))),
    )
    .await?;
    let resumed = report(
        "13 killed and resumed",
        match (&killed.exited, t.text.find("18"), t.result(), &t.session_id) {
            (
                Some(_),
                Some(_),
                Some(("success", TurnOutcome::Completed)),
                Some(id),
            ) if id == &session_id => Verdict::Pass,
            _ => Verdict::Fail,
        },
        &format!(
            "killed after {:?} exit={:?}; resumed {} -> session_id={:?} reply={:?}",
            killed.streamed.lines().last(),
            killed.exited,
            session_id.0,
            t.session_id.as_ref().map(|id| &id.0),
            t.text
        ),
    )?;
    let status = t.live.session.shutdown().await?;
    writeln!(std::io::stdout(), "exit status {}", status.describe())?;
    Ok((model, reasoning, resumed))
}

/// The effort `get_settings` reported, if it answered.
fn applied_effort(applied: &Result<Applied, Error>) -> Option<String> {
    applied
        .as_ref()
        .ok()
        .and_then(|settings| settings.effort.clone())
}

/// The statuses of a checklist's steps, in order.
fn statuses(steps: &[PlanStep]) -> Vec<TaskStatus> {
    steps.iter().map(|step| step.status).collect()
}

/// Steps 15 and 16, in a fifth session started at `high` effort: a turn
/// that keeps a checklist with `TodoWrite`, which asks no permission, and
/// the effort set to `low` in the running process before the next turn.
async fn checklist_and_effort(
    claude: &Path,
    fixtures: Option<&Path>,
) -> Result<(Verdict, Verdict)> {
    let (session, events) = ClaudeSession::spawn(ClaudeOptions {
        effort: Some(Effort::High),
        ..options(
            claude,
            None,
            log_path(fixtures, "session9-checklist-effort"),
        )
    })
    .await?;
    let t = Turn::start(
        Live { session, events },
        "Use the TodoWrite tool to make a checklist of exactly three steps: write a.txt, write b.txt, write c.txt. Then mark the first step in progress, then completed, with TodoWrite each time. Do not create any files and use no other tool besides finding TodoWrite. Then reply done.",
        Plan::Answer(Policy::Deny(String::from("only the checklist"))),
    )
    .await?;
    let checklist = report(
        "15 checklist",
        match (
            t.checklists.get(1),
            t.checklists.last().map(|steps| statuses(steps)),
            t.permissions.first(),
            t.result(),
        ) {
            (
                Some(_),
                Some(last),
                None,
                Some(("success", TurnOutcome::Completed)),
            ) if last
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
            "checklists={:?} permissions={:?} finished={:?} reply={:?}",
            t.checklists, t.permissions, t.finished, t.text
        ),
    )?;
    let before = t.live.session.applied().await;
    let set = t.live.session.set_effort(Effort::Low).await;
    let applied = t.live.session.applied().await;
    let after = Turn::start(
        t.live,
        "Reply with just the word pong.",
        Plan::Answer(Policy::Deny(String::from("no tools"))),
    )
    .await?;
    let kept = after.live.session.applied().await;
    let changed = report(
        "16 effort changed",
        match (
            applied_effort(&before).as_deref(),
            &set,
            applied_effort(&applied).as_deref(),
            applied_effort(&kept).as_deref(),
            after.result(),
            after.text.to_lowercase().find("pong"),
        ) {
            (
                Some("high"),
                Ok(()),
                Some("low"),
                Some("low"),
                Some(("success", TurnOutcome::Completed)),
                Some(_),
            ) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        &format!(
            "before={before:?} set_effort()={set:?} applied={applied:?} after the turn={kept:?} finished={:?} reply={:?}",
            after.finished, after.text
        ),
    )?;
    let status = after.live.session.shutdown().await?;
    writeln!(std::io::stdout(), "exit status {}", status.describe())?;
    Ok((checklist, changed))
}

/// What an interrupt at a cue does to the turn.
enum Expect {
    Interrupted,
    /// The CLI acknowledges an interrupt sent before it reads the message,
    /// and then either aborts the turn or runs it to the end.
    Either,
}

/// Interrupts a counting turn at `cue`, then checks what the interrupt did
/// and that the next turn works.
async fn interrupted(
    live: Live,
    step: &str,
    cue: Cue,
    expect: Expect,
) -> Result<(Live, Verdict)> {
    let t = Turn::start(
        live,
        "Count from 1 to 200, one number per line.",
        Plan::Interrupt(cue),
    )
    .await?;
    let interrupted = t
        .interrupted
        .as_ref()
        .context("turn ended before the interrupt cue")?;
    let after = Turn::start(
        t.live,
        "Reply with just the word pong.",
        Plan::Answer(Policy::Deny(String::from("no tools"))),
    )
    .await?;
    let verdict = report(
        step,
        match (
            expect,
            interrupted,
            t.finished.as_ref().map(|finished| {
                (&finished.outcome, finished.terminal_reason())
            }),
            after.result(),
            after.text.to_lowercase().find("pong"),
        ) {
            (
                Expect::Interrupted | Expect::Either,
                Ok(()),
                Some((TurnOutcome::Interrupted, Some("aborted_streaming"))),
                Some(("success", TurnOutcome::Completed)),
                Some(_),
            )
            | (
                Expect::Either,
                Ok(()),
                Some((TurnOutcome::Completed, Some("completed"))),
                Some(("success", TurnOutcome::Completed)),
                Some(_),
            ) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        &format!(
            "interrupt()={interrupted:?} streamed={:?} text={:?} permissions={:?} finished={:?}; next turn finished={:?} reply={:?}",
            t.streamed,
            t.text,
            t.permissions,
            t.finished,
            after.finished,
            after.text
        ),
    )?;
    Ok((after.live, verdict))
}

/// Steps 7 to 10, in a third session: a question answered, a command
/// approved for the session and then run again without asking, a question
/// withdrawn by an interrupt, and two questions with the same text, which
/// the CLI refuses before asking us.
async fn questions(
    claude: &Path,
    scratch: &Path,
    fixtures: Option<&Path>,
) -> Result<(Verdict, Verdict, Verdict, Verdict)> {
    let live =
        spawn(claude, None, log_path(fixtures, "session3-questions")).await?;
    let (answered, question) = question_answered(live).await?;
    let (approved, for_session) =
        approved_for_session(answered, scratch).await?;
    let (after_withdrawal, withdrawn) = question_withdrawn(approved).await?;
    let (refused, duplicate) = duplicate_questions(after_withdrawal).await?;
    let status = refused.session.shutdown().await?;
    writeln!(std::io::stdout(), "exit status {}", status.describe())?;
    Ok((question, for_session, withdrawn, duplicate))
}

/// Step 7: a question answered.
async fn question_answered(live: Live) -> Result<(Live, Verdict)> {
    let t = Turn::start(
        live,
        "Use the AskUserQuestion tool to ask me one question: which color do I prefer, with the options Red and Blue. Then reply with only the color I chose.",
        Plan::Answer(Policy::Deny(String::from("no tools besides the question"))),
    )
    .await?;
    let asked = t
        .questions
        .iter()
        .map(|questions| {
            questions
                .iter()
                .map(|question| question.options.len())
                .collect::<Vec<usize>>()
        })
        .collect::<Vec<Vec<usize>>>();
    let question = report(
        "7 question",
        match (&asked, t.result(), t.text.find("Blue")) {
            (shapes, Some(("success", TurnOutcome::Completed)), Some(_))
                if shapes == &Vec::from([Vec::from([2])]) =>
            {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        &format!(
            "questions={:?} permissions={:?} finished={:?} reply={:?}",
            t.questions, t.permissions, t.finished, t.text
        ),
    )?;
    Ok((t.live, question))
}

/// Step 8: a command approved for the session, then run again without
/// asking.
async fn approved_for_session(
    live: Live,
    scratch: &Path,
) -> Result<(Live, Verdict)> {
    let t = Turn::start(
        live,
        "Run exactly this bash command and nothing else: touch t1.txt",
        Plan::Answer(Policy::ApproveForSession),
    )
    .await?;
    let first = (t.permissions.clone(), t.for_session.clone());
    let again = Turn::start(
        t.live,
        "Run exactly this bash command again, and nothing else: touch t1.txt",
        Plan::Answer(Policy::Deny(String::from(
            "the session's rule should have allowed it",
        ))),
    )
    .await?;
    let for_session = report(
        "8 approve for session",
        match (
            &first,
            presence(&scratch.join("t1.txt")),
            again.permissions.first(),
            again.result(),
        ) {
            (
                requested,
                Presence::Present,
                None,
                Some(("success", TurnOutcome::Completed)),
            ) if requested
                == &(
                    Vec::from([String::from("Bash")]),
                    Vec::from([Some(String::from("Bash(touch t1.txt)"))]),
                ) =>
            {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        &format!(
            "first permissions={first:?} t1.txt {:?}; again permissions={:?} finished={:?}",
            presence(&scratch.join("t1.txt")),
            again.permissions,
            again.finished
        ),
    )?;
    Ok((again.live, for_session))
}

/// Step 9: a question withdrawn by an interrupt.
async fn question_withdrawn(live: Live) -> Result<(Live, Verdict)> {
    let t = Turn::start(
        live,
        "Use the AskUserQuestion tool to ask me one question: which fruit do I prefer, with the options Apple and Pear. Then reply with only the fruit I chose.",
        Plan::InterruptOnPermission,
    )
    .await?;
    let late = match &t.open {
        Some(id) => Some(
            t.live
                .session
                .respond(
                    id.clone(),
                    PermissionDecision::Answer(Vec::from([Answer(Vec::from(
                        [String::from("Pear")],
                    ))])),
                )
                .await,
        ),
        None => None,
    };
    let withdrawn = report(
        "9 question withdrawn by interrupt",
        match (
            t.questions.len(),
            &t.interrupted,
            t.cancelled_permissions,
            &late,
            t.finished.as_ref().map(|finished| {
                (&finished.outcome, finished.terminal_reason())
            }),
        ) {
            (
                1,
                Some(Ok(())),
                1,
                Some(Err(_)),
                Some((
                    TurnOutcome::Interrupted,
                    Some("aborted_tools" | "aborted_streaming"),
                )),
            ) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        &format!(
            "questions={:?} interrupt()={:?} cancelled={} finished={:?} respond-after={late:?}",
            t.questions, t.interrupted, t.cancelled_permissions, t.finished
        ),
    )?;
    Ok((t.live, withdrawn))
}

/// Step 10: two questions with the same text, which the CLI refuses before
/// asking us.
async fn duplicate_questions(live: Live) -> Result<(Live, Verdict)> {
    let t = Turn::start(
        live,
        "Use the AskUserQuestion tool once, with exactly two questions whose text is identical: both must be exactly 'Which color?'. The first has the options Red and Blue, the second Green and Yellow. If the tool refuses, say so and do not retry.",
        Plan::Answer(Policy::Deny(String::from("no tools besides the question"))),
    )
    .await?;
    let duplicate = report(
        "10 duplicate question texts",
        match (
            t.questions.len(),
            t.tool_errors
                .iter()
                .filter(|error| error.contains("Question texts must be unique"))
                .count(),
        ) {
            (0, 1) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        &format!(
            "questions={:?} tool errors={:?} finished={:?}",
            t.questions, t.tool_errors, t.finished
        ),
    )?;
    Ok((t.live, duplicate))
}

/// The prefix recipe survives a stop before the fork's first actual turn;
/// after that turn, Continue uses its confirmed new native id.
async fn undo_history(
    claude: &Path,
    fixtures: Option<&Path>,
) -> Result<Verdict> {
    let first = Turn::start(
        spawn(claude, None, log_path(fixtures, "session6-undo-original")).await?,
        "Remember my retained codeword is amber. Reply only amber. Do not use tools.",
        Plan::Answer(Policy::Approve),
    )
    .await?;
    let through = first
        .last_message
        .context("completed first turn had no transcript UUID")?;
    let source = first.live.session.id().clone();
    let second = Turn::start(
        first.live,
        "Remember my discarded codeword is cobalt. Reply only cobalt. Do not use tools.",
        Plan::Answer(Policy::Approve),
    )
    .await?;
    let _ = second.live.session.shutdown().await?;
    let target = SessionId(uuid::Uuid::new_v4().to_string());
    let fork = ClaudeOptions {
        resume: Resume::Fork {
            session: source,
            through,
            target: target.clone(),
        },
        ..options(claude, None, log_path(fixtures, "session7-undo-fork"))
    };
    let (session, _events) = ClaudeSession::spawn(fork.clone()).await?;
    let _ = session.shutdown().await?;
    let (session, events) = ClaudeSession::spawn(fork).await?;
    let prompt = "What are my retained and discarded codewords? Reply with only the two codewords joined by a slash, retained first, as in red/blue, writing unknown for a codeword not supplied in this conversation. Do not use tools.";
    let after = Turn::start(
        Live { session, events },
        prompt,
        Plan::Answer(Policy::Approve),
    )
    .await?;
    let actual = after.session_id.clone();
    let text = after.text.clone();
    let _ = after.live.session.shutdown().await?;
    let continued = Turn::start(
        spawn(
            claude,
            Some(target.clone()),
            log_path(fixtures, "session8-undo-continued"),
        )
        .await?,
        prompt,
        Plan::Answer(Policy::Approve),
    )
    .await?;
    let verdict = report(
        "14 undo native history and Continue",
        match (
            text.trim(),
            continued.text.trim(),
            actual.as_ref(),
            continued.result(),
        ) {
            (
                "amber/unknown",
                "amber/unknown",
                Some(got),
                Some(("success", TurnOutcome::Completed)),
            ) if got == &target => Verdict::Pass,
            _ => Verdict::Fail,
        },
        &format!(
            "fork target={target:?} actual={actual:?} after={text:?} continued={:?}",
            continued.text
        ),
    )?;
    let _ = continued.live.session.shutdown().await?;
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
    let claude = install(
        Cli::Claude,
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(LIVE_DATA),
    )
    .await?;

    let live = spawn(&claude, None, log_path(fixtures, "session1")).await?;
    let (approved, session_id, approve, deny) =
        approve_and_deny(live, scratch).await?;
    let (after_start, interrupt) = interrupted(
        approved,
        "3 interrupt at start",
        Cue::Start,
        Expect::Interrupted,
    )
    .await?;
    let (after_delta, mid_stream) = interrupted(
        after_start,
        "3c interrupt mid-stream",
        Cue::Delta,
        Expect::Interrupted,
    )
    .await?;
    let (after_send, at_send) = interrupted(
        after_delta,
        "3d interrupt at send",
        Cue::Send,
        Expect::Either,
    )
    .await?;
    let (first_exit, pending) = pending_permission(after_send, scratch).await?;
    let live = spawn(
        &claude,
        Some(session_id.clone()),
        log_path(fixtures, "session2-resume"),
    )
    .await?;
    let (resume, accept_edits, unknown_id) =
        resumed(live, scratch, &session_id, &first_exit).await?;
    let (question, for_session, withdrawn, duplicate) =
        questions(&claude, scratch, fixtures).await?;
    let (chosen, reasoning, resumed) =
        chosen_and_killed(&claude, fixtures).await?;
    let undo = undo_history(&claude, fixtures).await?;
    let (checklist, effort) = checklist_and_effort(&claude, fixtures).await?;
    let failed = [
        approve,
        deny,
        interrupt,
        mid_stream,
        at_send,
        pending,
        resume,
        accept_edits,
        unknown_id,
        question,
        for_session,
        withdrawn,
        duplicate,
        chosen,
        reasoning,
        resumed,
        undo,
        checklist,
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

/// Steps 1 and 2: a write approved, then one denied.
async fn approve_and_deny(
    live: Live,
    scratch: &Path,
) -> Result<(Live, SessionId, Verdict, Verdict)> {
    let t = Turn::start(
        live,
        "Use the Write tool to create the file a.txt in the current directory with the content `hello from s1`. Do nothing else.",
        Plan::Answer(Policy::Approve),
    )
    .await?;
    let a = scratch.join("a.txt");
    let approve = report(
        "1 approve",
        match (presence(&a), t.result(), (&t.permissions, &t.streamed)) {
            (
                Presence::Present,
                Some(("success", TurnOutcome::Completed)),
                seen,
            ) if seen == (&Vec::from([String::from("Write")]), &t.text) => {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        &format!(
            "permissions={:?} a.txt {:?} contents={:?} finished={:?} streamed={:?}",
            t.permissions,
            presence(&a),
            std::fs::read_to_string(&a).ok(),
            t.finished,
            t.streamed
        ),
    )?;
    let session_id = t.session_id.clone().context("no session id in turn 1")?;

    let denied = Turn::start(
        t.live,
        "Use the Write tool to create the file b.txt in the current directory with the content `nope`. If it is denied, do not retry; just say so.",
        Plan::Answer(Policy::Deny(String::from("The user does not want b.txt."))),
    )
    .await?;
    let b = scratch.join("b.txt");
    let deny = report(
        "2 deny",
        match (
            denied.permissions.first().map(String::as_str),
            presence(&b),
            &denied.finished,
        ) {
            (Some("Write"), Presence::Missing, Some(_)) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        &format!(
            "permissions={:?} b.txt {:?} finished={:?} reply={:?}",
            denied.permissions,
            presence(&b),
            denied.finished,
            denied.text
        ),
    )?;

    Ok((denied.live, session_id, approve, deny))
}

/// Step 3b: an interrupt with a permission request open. Returns how the
/// CLI exited after it.
async fn pending_permission(
    live: Live,
    scratch: &Path,
) -> Result<(String, Verdict)> {
    let t = Turn::start(
        live,
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
        &format!(
            "permissions={:?} interrupt()={:?} cancelled={} finished={:?} terminal_reason={:?} respond-after={late:?}",
            t.permissions,
            t.interrupted,
            t.cancelled_permissions,
            t.finished,
            t.finished.as_ref().and_then(Finished::terminal_reason)
        ),
    )?;

    let status = t.live.session.shutdown().await?;
    Ok((status.describe(), pending))
}

/// Steps 4 to 6, in the resumed session: it remembers the first, then
/// `acceptEdits` and a request id that was never sent.
async fn resumed(
    live: Live,
    scratch: &Path,
    session_id: &SessionId,
    first_exit: &str,
) -> Result<(Verdict, Verdict, Verdict)> {
    let t = Turn::start(
        live,
        "What file did you successfully create earlier in this conversation? Answer with just the filename.",
        Plan::Answer(Policy::Deny(String::from("no tools"))),
    )
    .await?;
    let resume = report(
        "4 resume",
        match (t.text.find("a.txt"), t.result()) {
            (Some(_), Some(("success", TurnOutcome::Completed))) => {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        &format!(
            "first exit={first_exit} resumed {} -> session_id={:?} reply={:?}",
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
    let accepted = Turn::start(
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
        match (&mode, accepted.permissions.first(), presence(&c)) {
            (Ok(()), None, Presence::Present) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        &format!(
            "set_permission_mode()={mode:?} permissions={:?} c.txt {:?} finished={:?}",
            accepted.permissions,
            presence(&c),
            accepted.finished
        ),
    )?;

    let bogus = accepted
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
        &format!("respond(never-sent)={bogus:?}"),
    )?;

    let status = accepted.live.session.shutdown().await?;
    writeln!(std::io::stdout(), "exit status {}", status.describe())?;
    Ok((resume, accept_edits, unknown_id))
}
