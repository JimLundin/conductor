//! S1 live checklist: drives the real `claude` CLI through approve, deny,
//! interrupt, resume, set_permission_mode, questions, approval for the
//! session, a chosen model and effort, reasoning, and a killed CLI resumed,
//! printing PASS/FAIL per step.
//!
//! Usage: s1 [FIXTURE_DIR]
//! Writes raw CLI stdout per step to FIXTURE_DIR/<step>.jsonl when given.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agents::Home;
use agents::claude::{
    AfterDeny, Block, ClaudeEvent, ClaudeOptions, ClaudeSession, Error, Outcome,
    PermissionDecision, PermissionMode, PermissionRequestId, Role, SessionId, SettingSource,
    VersionPolicy,
};
use agents::install::{Cli, install};
use agents::live::{Cue, Presence, Verdict, fresh_scratch, log_path, presence, report};
use anyhow::{Context, Result};
use proto::{Answer, Effort, Question};
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
    ApproveForSession,
    Deny(String),
}

impl Policy {
    fn decision(&self) -> PermissionDecision {
        match self {
            Policy::Approve => PermissionDecision::Allow,
            Policy::ApproveForSession => PermissionDecision::AllowForSession,
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
    /// How the CLI exited, under `Plan::Kill`.
    exited: Option<String>,
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
        let mut turn = Turn {
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
            exited: None,
            finished: None,
            session_id: None,
            interrupted: None,
            open: None,
        };
        turn.cue(Cue::Send).await;
        turn.run().await
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
                self.cue(Cue::Start).await;
                Ok(Progress::Playing)
            }
            ClaudeEvent::Message {
                role: Role::Assistant,
                content,
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
            } => {
                self.tool_errors
                    .extend(content.into_iter().filter_map(|block| match block {
                        Block::ToolResult {
                            content,
                            outcome: Outcome::Failed,
                            ..
                        } => Some(content.to_string()),
                        Block::ToolResult { .. }
                        | Block::Text(_)
                        | Block::Thinking(_)
                        | Block::ToolUse { .. }
                        | Block::Other(_) => None,
                    }));
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
            // A question is answered with each one's last option.
            ClaudeEvent::QuestionAsked { id, questions, .. } => {
                let answers = questions
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
                    .collect::<Vec<Answer>>();
                self.questions.push(questions);
                self.on_permission(id, Some(answers)).await?;
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
            ClaudeEvent::Exited { status } => match self.plan {
                Plan::Kill(_) => {
                    self.exited = Some(status.describe());
                    Ok(Progress::Done)
                }
                Plan::Answer(_) | Plan::Interrupt(_) | Plan::InterruptOnPermission => Err(
                    anyhow::Error::msg(format!("claude exited mid-turn: {}", status.describe())),
                ),
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
                | Block::Other(_) => None,
            }));
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
            (Plan::InterruptOnPermission, Some(_)) | (Plan::Interrupt(_) | Plan::Kill(_), _) => {
                let decision = PermissionDecision::Deny {
                    message: String::from("not expected after an interrupt"),
                    then: AfterDeny::Continue,
                };
                self.answer(id, decision).await
            }
            (Plan::Answer(policy), _) => {
                let decision = match answers {
                    Some(answers) => PermissionDecision::Answer(answers),
                    None => policy.decision(),
                };
                self.answer(id, decision).await
            }
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

fn options(claude: &Path, resume: Option<SessionId>, stdout_log: Option<PathBuf>) -> ClaudeOptions {
    ClaudeOptions {
        claude_path: PathBuf::from(claude),
        home: Home::User,
        credential: None,
        cwd: PathBuf::from(SCRATCH),
        model: Some(String::from("haiku")),
        effort: None,
        permission_mode: PermissionMode::Default,
        resume,
        // Keep the user's own hooks and plugins out of the spike.
        setting_sources: Some(Vec::from([SettingSource::Project, SettingSource::Local])),
        stdout_log,
        versions: VersionPolicy::SupportedOnly,
    }
}

async fn spawn(
    claude: &Path,
    resume: Option<SessionId>,
    stdout_log: Option<PathBuf>,
) -> Result<Live> {
    let (session, events) = ClaudeSession::spawn(options(claude, resume, stdout_log)).await?;
    Ok(Live { session, events })
}

/// Kills the process as a crash would, without closing its stdin.
fn kill(pid: Option<u32>) {
    let killed = pid.map(|pid| {
        std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
    });
    tracing::info!(?pid, ?killed, "killed claude");
}

/// The model and effort steps 11 to 13 choose, other than s1's default.
const CHOSEN_MODEL: &str = "sonnet";
const CHOSEN_EFFORT: Effort = Effort::High;
/// The model `get_settings` reports for [`CHOSEN_MODEL`].
const CHOSEN_RESOLVED: &str = "claude-sonnet-5-5";

fn chosen(claude: &Path, resume: Option<SessionId>, stdout_log: Option<PathBuf>) -> ClaudeOptions {
    ClaudeOptions {
        model: Some(String::from(CHOSEN_MODEL)),
        effort: Some(CHOSEN_EFFORT),
        ..options(claude, resume, stdout_log)
    }
}

/// Steps 11 to 13, in a fourth session started with a chosen model and
/// effort: the CLI reports them, a turn reasons, and the CLI killed in the
/// middle of a turn is resumed in a new process that remembers the first
/// turn.
async fn chosen_and_killed(
    claude: &Path,
    fixtures: Option<&Path>,
) -> Result<(Verdict, Verdict, Verdict)> {
    let log = log_path(fixtures, "session4-chosen-killed");
    let (session, events) = ClaudeSession::spawn(chosen(claude, None, log)).await?;
    let applied = session.applied().await;
    let model = report(
        "11 chosen model and effort",
        match applied
            .as_ref()
            .map(|applied| (applied.model.as_str(), applied.effort.as_deref()))
        {
            Ok((CHOSEN_RESOLVED, Some("high"))) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!("applied={applied:?}"),
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
            (joined, Some(_), Some(("success", Outcome::Succeeded))) if joined == t.thought => {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        format!(
            "thinking blocks={} text={:?} streamed={:?} reply={:?}",
            t.thinking.len(),
            t.thinking,
            t.thought,
            t.text
        ),
    )?;
    let session_id = t.session_id.clone().context("no session id in step 12")?;
    let killed = Turn::start(
        t.live,
        "Count from 1 to 200, one number per line.",
        Plan::Kill(Cue::Delta),
    )
    .await?;
    let (session, events) = ClaudeSession::spawn(chosen(
        claude,
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
            (Some(_), Some(_), Some(("success", Outcome::Succeeded)), Some(id))
                if id == &session_id =>
            {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        format!(
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

/// What an interrupt at a cue does to the turn.
enum Expect {
    Interrupted,
    /// The CLI acknowledges an interrupt sent before it reads the message,
    /// and then either aborts the turn or runs it to the end.
    Either,
}

/// Interrupts a counting turn at `cue`, then checks what the interrupt did
/// and that the next turn works.
async fn interrupted(live: Live, step: &str, cue: Cue, expect: Expect) -> Result<(Live, Verdict)> {
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
            t.finished
                .as_ref()
                .map(|finished| (finished.outcome, finished.terminal_reason())),
            after.result(),
            after.text.to_lowercase().find("pong"),
        ) {
            (
                Expect::Interrupted | Expect::Either,
                Ok(()),
                Some((Outcome::Failed, Some("aborted_streaming"))),
                Some(("success", Outcome::Succeeded)),
                Some(_),
            )
            | (
                Expect::Either,
                Ok(()),
                Some((Outcome::Succeeded, Some("completed"))),
                Some(("success", Outcome::Succeeded)),
                Some(_),
            ) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "interrupt()={interrupted:?} streamed={:?} text={:?} permissions={:?} finished={:?}; next turn finished={:?} reply={:?}",
            t.streamed, t.text, t.permissions, t.finished, after.finished, after.text
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
    let live = spawn(claude, None, log_path(fixtures, "session3-questions")).await?;
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
            (asked, Some(("success", Outcome::Succeeded)), Some(_))
                if asked == &Vec::from([Vec::from([2])]) =>
            {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        format!(
            "questions={:?} permissions={:?} finished={:?} reply={:?}",
            t.questions, t.permissions, t.finished, t.text
        ),
    )?;

    let t = Turn::start(
        t.live,
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
            (first, Presence::Present, None, Some(("success", Outcome::Succeeded)))
                if first
                    == &(
                        Vec::from([String::from("Bash")]),
                        Vec::from([Some(String::from("Bash(touch t1.txt)"))]),
                    ) =>
            {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        format!(
            "first permissions={first:?} t1.txt {:?}; again permissions={:?} finished={:?}",
            presence(&scratch.join("t1.txt")),
            again.permissions,
            again.finished
        ),
    )?;

    let t = Turn::start(
        again.live,
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
                    PermissionDecision::Answer(Vec::from([Answer(Vec::from([String::from(
                        "Pear",
                    )]))])),
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
            t.finished
                .as_ref()
                .map(|finished| (finished.outcome, finished.terminal_reason())),
        ) {
            (
                1,
                Some(Ok(())),
                1,
                Some(Err(_)),
                Some((Outcome::Failed, Some("aborted_tools" | "aborted_streaming"))),
            ) => Verdict::Pass,
            _ => Verdict::Fail,
        },
        format!(
            "questions={:?} interrupt()={:?} cancelled={} finished={:?} respond-after={late:?}",
            t.questions, t.interrupted, t.cancelled_permissions, t.finished
        ),
    )?;
    let t = Turn::start(
        t.live,
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
        format!(
            "questions={:?} tool errors={:?} finished={:?}",
            t.questions, t.tool_errors, t.finished
        ),
    )?;
    let status = t.live.session.shutdown().await?;
    writeln!(std::io::stdout(), "exit status {}", status.describe())?;
    Ok((question, for_session, withdrawn, duplicate))
}

async fn run(fixtures: Option<PathBuf>) -> Result<()> {
    fresh_scratch(Path::new(SCRATCH))?;
    let scratch = Path::new(SCRATCH);
    fixtures.as_ref().map(std::fs::create_dir_all).transpose()?;
    let fixtures = fixtures.as_deref();
    let claude = install(
        Cli::Claude,
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(LIVE_DATA),
    )
    .await?;

    let live = spawn(&claude, None, log_path(fixtures, "session1")).await?;
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
            (Presence::Present, Some(("success", Outcome::Succeeded)), seen)
                if seen == (&Vec::from([String::from("Write")]), &t.text) =>
            {
                Verdict::Pass
            }
            _ => Verdict::Fail,
        },
        format!(
            "permissions={:?} a.txt {:?} contents={:?} finished={:?} streamed={:?}",
            t.permissions,
            presence(&a),
            std::fs::read_to_string(&a).ok(),
            t.finished,
            t.streamed
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

    let (live, interrupt) = interrupted(
        t.live,
        "3 interrupt at start",
        Cue::Start,
        Expect::Interrupted,
    )
    .await?;
    let (live, mid_stream) = interrupted(
        live,
        "3c interrupt mid-stream",
        Cue::Delta,
        Expect::Interrupted,
    )
    .await?;
    let (live, at_send) =
        interrupted(live, "3d interrupt at send", Cue::Send, Expect::Either).await?;

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
        &claude,
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
    let (question, for_session, withdrawn, duplicate) =
        questions(&claude, scratch, fixtures).await?;
    let (chosen, reasoning, resumed) = chosen_and_killed(&claude, fixtures).await?;
    let failed = Vec::from([
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
    ])
    .into_iter()
    .filter(|verdict| matches!(verdict, Verdict::Fail))
    .count();
    match failed {
        0 => Ok(()),
        failed => Err(anyhow::Error::msg(format!("{failed} step(s) failed"))),
    }
}
