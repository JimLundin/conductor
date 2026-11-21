//! Replays recorded `codex app-server` stdout (fixtures/codex) through a real
//! `CodexServer`. A stand-in process prints the recording, holding each
//! response until our request with that id arrives and each
//! `serverRequest/resolved` until we answer that request, as Codex does. The
//! actor, the boundary mapping and the exactly-once approval bookkeeping all
//! run against Codex's real bytes, driven by the calls the recorded run made.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use codex::{
    ApprovalId, ApprovalKind, ApprovalPolicy, CodexEvent, Decision, Sandbox,
    ThreadEvent, ThreadId, ThreadOptions, ThreadSettings, TurnId,
};
use install::Home;
use n0_future::StreamExt;
use proto::{Answer, Effort, TaskStatus, ToolKind, ToolPhase, TurnOutcome};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::replay_support::{
    TestResult, fake_cli_path, fake_codex_dir, fixture,
};

use super::{CodexServer, CodexThread, Error, ServerOptions};

fn opts() -> ThreadOptions {
    ThreadOptions {
        instructions: None,
        cwd: "/tmp/s2-scratch".into(),
        model: Some(String::from("gpt-6-luna")),
        effort: Some(Effort::Low),
        approval_policy: ApprovalPolicy::OnRequest,
        sandbox: Sandbox::ReadOnly,
    }
}

/// What the recorder reads events until.
enum Goal {
    /// The turn completes. Every event must belong to its thread, and every
    /// approval is answered with `decision`.
    Turn {
        thread: CodexThread,
        turn: TurnId,
        decision: Decision,
    },
    /// Every turn in `Recorder::open` completes.
    Turns,
    /// The app-server exits.
    Exit,
}

/// Whether a turn interrupts itself after its first output.
enum Interrupt {
    Never,
    AfterFirstOutput,
    /// Leaves the first request open and interrupts the turn instead.
    OnRequest,
    Sent,
}

/// Whether the recorder has reached its goal.
enum Progress {
    Playing,
    Done,
}

/// Collects the event stream as readable lines. Thread ids become `A`, `B`,
/// ... in order of first appearance; unmapped frames are tallied by method
/// instead of listed, since deltas and status frames dominate the stream.
struct Recorder {
    server: CodexServer,
    cwd: PathBuf,
    events: mpsc::Receiver<CodexEvent>,
    goal: Goal,
    interrupt: Interrupt,
    /// The turns `Goal::Turns` waits on, by thread.
    open: HashMap<ThreadId, TurnId>,
    names: HashMap<ThreadId, char>,
    seen: Vec<String>,
    /// Each thread's deltas since its last other event, recorded as one line.
    deltas: HashMap<ThreadId, Vec<String>>,
    unknown: BTreeMap<String, usize>,
    /// Every approval id answered so far.
    answered: Vec<(ThreadId, ApprovalId)>,
}

impl Recorder {
    async fn until(
        &mut self,
        goal: Goal,
        interrupt: Interrupt,
    ) -> TestResult<()> {
        self.goal = goal;
        self.interrupt = interrupt;
        loop {
            let event = self.next().await?;
            match self.on(event).await? {
                Progress::Playing => {}
                Progress::Done => break Ok(()),
            }
        }
    }

    async fn next(&mut self) -> TestResult<CodexEvent> {
        let event =
            tokio::time::timeout(Duration::from_secs(20), self.events.recv())
                .await
                .map_err(|_| "no event within 20s; the replay is stuck")?
                .ok_or("event stream ended early")?;
        self.record(&event);
        Ok(event)
    }

    async fn on(&mut self, received: CodexEvent) -> TestResult<Progress> {
        Ok(match (&self.goal, received) {
            (Goal::Exit, CodexEvent::Exited { .. }) => Progress::Done,
            (
                Goal::Turns,
                CodexEvent::Thread {
                    thread,
                    event: ThreadEvent::TurnCompleted { turn, .. },
                },
            ) => {
                assert_eq!(
                    self.open.remove(&thread),
                    Some(turn),
                    "a completed turn belongs to {thread:?}"
                );
                match self.open.keys().next() {
                    Some(_) => Progress::Playing,
                    None => Progress::Done,
                }
            }
            (
                Goal::Turn {
                    thread: expected,
                    turn,
                    decision,
                },
                CodexEvent::Thread { thread, event },
            ) => {
                assert_eq!(thread, expected.id);
                match (event, &self.interrupt) {
                    (
                        ThreadEvent::ApprovalRequested { .. },
                        Interrupt::OnRequest,
                    )
                    | (
                        ThreadEvent::AgentMessage { .. }
                        | ThreadEvent::ToolCall { .. },
                        Interrupt::AfterFirstOutput,
                    ) => {
                        expected.interrupt(turn.clone()).await?;
                        self.interrupt = Interrupt::Sent;
                        Progress::Playing
                    }
                    (ThreadEvent::ApprovalRequested { id, .. }, _) => {
                        self.server
                            .respond(
                                thread.clone(),
                                id.clone(),
                                decision.clone(),
                            )
                            .await?;
                        self.answered.push((thread, id));
                        Progress::Playing
                    }
                    (ThreadEvent::TurnCompleted { turn: done, .. }, _)
                        if &done == turn =>
                    {
                        Progress::Done
                    }
                    _ => Progress::Playing,
                }
            }
            (Goal::Exit | Goal::Turns | Goal::Turn { .. }, _) => {
                Progress::Playing
            }
        })
    }

    fn record(&mut self, received: &CodexEvent) {
        match received {
            CodexEvent::Thread {
                event: ThreadEvent::Unknown(raw),
                ..
            }
            | CodexEvent::Unknown(raw) => self.tally(method(raw)),
            CodexEvent::Thread {
                thread,
                event: ThreadEvent::AgentDelta { text, .. },
            } => self
                .deltas
                .entry(thread.clone())
                .or_default()
                .push(text.clone()),
            CodexEvent::Thread { thread, event } => {
                if let Some(deltas) = self.deltas.remove(thread) {
                    let line =
                        format!("{} streams {deltas:?}", self.name(thread));
                    self.seen.push(line);
                }
                let line = format!("{} {}", self.name(thread), summary(event));
                self.seen.push(line);
            }
            CodexEvent::Exited { status } => {
                self.seen.push(format!("exited {}", status.describe()));
            }
        }
    }

    fn tally(&mut self, method: String) {
        let count = self
            .unknown
            .get(&method)
            .copied()
            .unwrap_or(0)
            .saturating_add(1);
        self.unknown.insert(method, count);
    }

    fn name(&mut self, thread: &ThreadId) -> char {
        let index = u8::try_from(self.names.len()).unwrap_or(u8::MAX);
        let next = char::from(b'A'.saturating_add(index));
        self.names.entry(thread.clone()).or_insert(next).to_owned()
    }
}

fn method(raw: &Value) -> String {
    raw.get("method")
        .and_then(Value::as_str)
        .unwrap_or("<no method>")
        .to_owned()
}

fn summary(event: &ThreadEvent) -> String {
    match event {
        ThreadEvent::Started => String::from("started"),
        ThreadEvent::SecretQuestionDeclined { .. } => {
            String::from("secret question declined")
        }
        ThreadEvent::TurnStarted { .. } => String::from("turn started"),
        ThreadEvent::TurnCompleted { outcome, .. } => {
            format!("turn {}", outcome_name(outcome))
        }
        ThreadEvent::AgentDelta { text, .. } => format!("delta {text:?}"),
        ThreadEvent::AgentMessage { text, .. } => format!("says {text:?}"),
        ThreadEvent::ReasoningDelta { text, .. } => {
            format!("reasoning delta {text:?}")
        }
        ThreadEvent::Reasoning { text, .. } => format!("reasons {text:?}"),
        ThreadEvent::ToolCall { phase, .. } => match phase {
            ToolPhase::Started { tool } => match tool {
                ToolKind::Command { .. } => String::from("started command"),
                ToolKind::FileChange { files } => {
                    format!("started fileChange {}", files.len())
                }
                ToolKind::Other { .. } => String::from("started other"),
            },
            ToolPhase::Finished { outcome } => {
                format!("finished tool {outcome:?}")
            }
        },
        ThreadEvent::ApprovalRequested { id, kind, .. } => {
            let label = match kind {
                ApprovalKind::Command { .. } => "command",
                ApprovalKind::FileChange { .. } => "fileChange",
                ApprovalKind::Permissions { .. } => "permissions",
                ApprovalKind::UserInput { questions } => {
                    &format!("userInput {questions:?}")
                }
            };
            format!("approval {} {label}", id.0)
        }
        ThreadEvent::ApprovalCancelled { id } => {
            format!("approval {} cancelled", id.0)
        }
        ThreadEvent::PlanUpdated { steps, .. } => format!(
            "plan {:?}",
            steps
                .iter()
                .map(|step| (step.text.as_str(), step.status))
                .collect::<Vec<(&str, TaskStatus)>>()
        ),
        ThreadEvent::SettingsChanged(settings) => {
            format!("settings {settings:?}")
        }
        ThreadEvent::Unknown(raw) => format!("unknown {}", method(raw)),
    }
}

fn outcome_name(outcome: &TurnOutcome) -> String {
    match outcome {
        TurnOutcome::Completed => String::from("completed"),
        TurnOutcome::Interrupted => String::from("interrupted"),
        TurnOutcome::Failed { message } => format!("failed {message:?}"),
    }
}

async fn spawn(name: &str) -> TestResult<Recorder> {
    let cwd = fake_codex_dir(&fixture(name), "driver")?;
    let (server, events) = CodexServer::spawn(ServerOptions {
        codex_path: fake_cli_path(),
        home: Home::User,
        credential: None,
        cwd: cwd.clone(),
    })
    .await?;
    Ok(Recorder {
        server,
        cwd,
        events,
        goal: Goal::Exit,
        interrupt: Interrupt::Never,
        open: HashMap::new(),
        names: HashMap::new(),
        seen: Vec::new(),
        deltas: HashMap::new(),
        unknown: BTreeMap::new(),
        answered: Vec::new(),
    })
}

fn captured(cwd: &Path) -> TestResult<Vec<Value>> {
    Ok(std::fs::read_to_string(cwd.join("stdin.jsonl"))?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<_>, _>>()?)
}

/// Starts a turn on `thread` and records it until it completes.
async fn turn(
    rec: &mut Recorder,
    thread: &CodexThread,
    text: &str,
    decision: Decision,
    interrupt: Interrupt,
) -> TestResult<()> {
    let turn = thread.send_turn(String::from(text), None).await?;
    let goal = Goal::Turn {
        thread: thread.clone(),
        turn,
        decision,
    };
    rec.until(goal, interrupt).await
}

async fn answer_again(
    server: &CodexServer,
    thread: ThreadId,
    id: ApprovalId,
) -> String {
    match server.respond(thread, id, Decision::Accept).await {
        Ok(()) => String::from("ok"),
        Err(e) => e.to_string(),
    }
}

#[tokio::test]
async fn approve_decline_interrupt_and_two_threads() -> TestResult<()> {
    let mut rec = spawn("server1.jsonl").await?;
    let server = rec.server.clone();
    assert_eq!(server.codex_version(), Some("0.166.0"));
    let models = server.list_models().await?;
    assert_eq!(
        models.choices.iter().find_map(|m| match m.id.0.as_str() {
            "gpt-6-luna" => Some(m.id.0.as_str()),
            _ => None,
        }),
        Some("gpt-6-luna")
    );

    let a = server.open_thread(opts(), None).await?;
    turn(&mut rec, &a, "a.txt", Decision::Accept, Interrupt::Never).await?;
    turn(&mut rec, &a, "b.txt", Decision::Decline, Interrupt::Never).await?;
    turn(
        &mut rec,
        &a,
        "long",
        Decision::Decline,
        Interrupt::AfterFirstOutput,
    )
    .await?;
    turn(&mut rec, &a, "pong", Decision::Decline, Interrupt::Never).await?;

    let b = server.open_thread(opts(), None).await?;
    let c = server.open_thread(opts(), None).await?;
    let (tb, tc) = n0_future::future::try_zip(
        b.send_turn(String::from("alpha"), None),
        c.send_turn(String::from("bravo"), None),
    )
    .await?;
    rec.open = HashMap::from([(b.id.clone(), tb), (c.id.clone(), tc)]);
    rec.until(Goal::Turns, Interrupt::Never).await?;

    let again = n0_future::stream::iter(rec.answered.clone())
        .then(|(thread, id)| answer_again(&server, thread, id))
        .collect::<Vec<String>>()
        .await;
    assert_eq!(server.shutdown().await?.describe(), "exit status: 0");
    rec.until(Goal::Exit, Interrupt::Never).await?;

    assert_eq!(rec.seen, SERVER1_SEEN);
    let replies = captured(&rec.cwd)?
        .into_iter()
        .filter(|line| line.get("id").is_some() && line.get("method").is_none())
        .collect::<Vec<_>>();
    assert_eq!(
        replies,
        [
            serde_json::json!({"id": 0, "result": {"decision": "accept"}}),
            serde_json::json!({"id": 1, "result": {"decision": "decline"}}),
        ],
    );
    assert_eq!(
        again,
        [
            "no open approval with id integer:0",
            "no open approval with id integer:1",
        ]
    );
    // Item kinds we do not map (userMessage), reasoning and agentMessage
    // starts stay Unknown, alongside the frames §7.2 does not ask for.
    assert_eq!(rec.unknown, server1_unknown());
    Ok(())
}

/// What `approve_decline_interrupt_and_two_threads` records.
const SERVER1_SEEN: &[&str] = &[
    "A started",
    // 1: approve. The first write fails in the read-only sandbox
    // without asking; the retry asks.
    "A turn started",
    "A streams [\"I\", \"’ll\", \" create\", \" the\", \" requested\", \" file\", \" with\", \" exactly\", \" that\", \" content\", \".\"]",
    "A says \"I’ll create the requested file with exactly that content.\"",
    "A started command",
    "A finished tool Failed",
    "A reasons \"\"",
    "A started command",
    "A approval integer:0 command",
    "A finished tool Succeeded",
    "A streams [\"Created\", \" `\", \"a\", \".txt\", \"`\", \" containing\", \" exactly\", \" `\", \"hello\", \" from\", \" s\", \"2\", \"`.\"]",
    "A says \"Created `a.txt` containing exactly `hello from s2`.\"",
    "A turn completed",
    // 2: decline.
    "A turn started",
    "A reasons \"\"",
    "A streams [\"I\", \"’ll\", \" request\", \" permission\", \" to\", \" write\", \" `\", \"b\", \".txt\", \"`;\", \" if\", \" it\", \"’s\", \" declined\", \",\", \" I\", \"’ll stop\", \".\"]",
    "A says \"I’ll request permission to write `b.txt`; if it’s declined, I’ll stop.\"",
    "A started command",
    "A approval integer:1 command",
    "A finished tool Declined",
    "A streams [\"The\", \" request\", \" to\", \" create\", \" `\", \"b\", \".txt\", \"`\", \" was\", \" declined\", \".\"]",
    "A says \"The request to create `b.txt` was declined.\"",
    "A turn completed",
    // 3: interrupt after the first output, then a normal turn.
    "A turn started",
    "A streams [\"starting\"]",
    "A says \"starting\"",
    "A turn interrupted",
    "A turn started",
    "A streams [\"pong\"]",
    "A says \"pong\"",
    "A turn completed",
    // 4: two threads with turns in flight on one process.
    "B started",
    "C started",
    "C turn started",
    "B turn started",
    "C streams [\"br\", \"avo\"]",
    "C says \"bravo\"",
    "C turn completed",
    "B streams [\"alpha\"]",
    "B says \"alpha\"",
    "B turn completed",
    "exited exit status: 0",
];

fn server1_unknown() -> BTreeMap<String, usize> {
    BTreeMap::from([
        ("account/rateLimits/updated".to_owned(), 9),
        ("hook/completed".to_owned(), 3),
        ("hook/started".to_owned(), 3),
        ("item/commandExecution/outputDelta".to_owned(), 1),
        ("item/completed".to_owned(), 6),
        ("item/started".to_owned(), 16),
        ("mcpServer/startupStatus/updated".to_owned(), 6),
        ("remoteControl/status/changed".to_owned(), 1),
        ("thread/readState/changed".to_owned(), 5),
        ("thread/status/changed".to_owned(), 16),
        ("thread/tokenUsage/updated".to_owned(), 9),
    ])
}

#[tokio::test]
async fn resume_in_a_new_process() -> TestResult<()> {
    let mut rec = spawn("server2-resume.jsonl").await?;
    let server = rec.server.clone();
    let a = ThreadId(first_thread_id(&fixture("server1.jsonl"))?);
    let resumed = server.open_thread(opts(), Some(a.clone())).await?;
    assert_eq!(resumed.id, a);
    turn(
        &mut rec,
        &resumed,
        "which file?",
        Decision::Decline,
        Interrupt::Never,
    )
    .await?;
    let bogus = server
        .respond(a, ApprovalId(String::from("never-sent")), Decision::Accept)
        .await
        .map_err(|e| e.to_string());
    assert_eq!(server.shutdown().await?.describe(), "exit status: 0");
    rec.until(Goal::Exit, Interrupt::Never).await?;

    assert_eq!(
        rec.seen,
        [
            // `thread/resume` sends no `thread/started`.
            "A turn started",
            "A streams [\"a\", \".txt\"]",
            "A says \"a.txt\"",
            "A turn completed",
            "exited exit status: 0",
        ]
    );
    assert_eq!(bogus, Err("no open approval with id never-sent".to_owned()));
    Ok(())
}

/// A thread started with a chosen effort, which Codex reports back, and a
/// turn that reasons. The recording goes on to a turn killed mid-run, which
/// waits here for a `turn/start` nobody sends.
#[tokio::test]
async fn chosen_effort_and_reasoning() -> TestResult<()> {
    let mut rec = spawn("server4-chosen-killed.jsonl").await?;
    let server = rec.server.clone();
    let a = server.open_thread(opts(), None).await?;
    turn(
        &mut rec,
        &a,
        "is 1000003 prime?",
        Decision::Decline,
        Interrupt::Never,
    )
    .await?;
    assert_eq!(server.shutdown().await?.describe(), "exit status: 0");
    rec.until(Goal::Exit, Interrupt::Never).await?;
    let deltas = rec
        .seen
        .iter()
        .filter(|line| line.starts_with("A reasoning delta"))
        .count();
    let reasoning = "**Assessing primality**\n\nI need to determine if 1,000,003 is prime. To do this, I should check its divisors up to the square root of about 1,000. It seems I recall that 1,000,003 is a known prime number, often recognized as the first prime above one million. I think I can confidently answer \"yes\" to the user's question without needing any tools. It’s important to verify my information carefully, though!";
    assert_eq!(
        (
            a.settings,
            deltas,
            rec.seen
                .into_iter()
                .filter(|line| !line.starts_with("A reasoning delta"))
                .collect::<Vec<String>>()
        ),
        (
            ThreadSettings {
                model: String::from("gpt-6-luna"),
                effort: Some(Effort::High)
            },
            89,
            Vec::from([
                String::from("A started"),
                String::from("A turn started"),
                format!("A reasons {reasoning:?}"),
                String::from("A streams [\"Yes\", \".\"]"),
                String::from("A says \"Yes.\""),
                String::from("A turn completed"),
                String::from("exited exit status: 0"),
            ])
        )
    );
    Ok(())
}

/// Three plan updates, each the whole plan, and a turn started at `high`,
/// which Codex reports as the thread's effort.
#[tokio::test]
async fn plan_and_effort_changed_with_the_turn() -> TestResult<()> {
    let mut rec = spawn("server8-plan-effort.jsonl").await?;
    let server = rec.server.clone();
    let a = server.open_thread(opts(), None).await?;
    turn(&mut rec, &a, "plan", Decision::Decline, Interrupt::Never).await?;
    let high = a
        .send_turn(String::from("pong"), Some(Effort::High))
        .await?;
    rec.until(
        Goal::Turn {
            thread: a.clone(),
            turn: high,
            decision: Decision::Decline,
        },
        Interrupt::Never,
    )
    .await?;
    assert_eq!(server.shutdown().await?.describe(), "exit status: 0");
    rec.until(Goal::Exit, Interrupt::Never).await?;
    let plan = |first| {
        format!(
            "A plan [(\"write a.txt\", {first}), (\"write b.txt\", Pending), (\"write c.txt\", Pending)]"
        )
    };
    let started = captured(&rec.cwd)?
        .into_iter()
        .filter(|request| {
            request.get("method") == Some(&Value::from("turn/start"))
        })
        .map(|request| request.pointer("/params/effort").cloned())
        .collect::<Vec<Option<Value>>>();
    assert_eq!(
        (started, rec.seen.into_iter().collect::<Vec<String>>()),
        (
            Vec::from([None, Some(Value::from("high"))]),
            Vec::from([
                String::from("A started"),
                String::from("A turn started"),
                String::from("A reasons \"\""),
                plan("Pending"),
                plan("InProgress"),
                plan("Completed"),
                String::from("A streams [\"done\"]"),
                String::from("A says \"done\""),
                String::from("A turn completed"),
                String::from(
                    "A settings ThreadSettings { model: \"gpt-6-luna\", effort: Some(High) }"
                ),
                String::from("A turn started"),
                String::from("A streams [\"pong\"]"),
                String::from("A says \"pong\""),
                String::from("A turn completed"),
                String::from("exited exit status: 0"),
            ])
        )
    );
    Ok(())
}

/// The recording holds the question's `serverRequest/resolved` until our
/// answer, so a change to the question's shape in a later pin fails here, as
/// does a lost feature `warning`.
#[tokio::test]
async fn questions_and_accept_for_session() -> TestResult<()> {
    let mut rec = spawn("server3-questions.jsonl").await?;
    let server = rec.server.clone();
    let a = server.open_thread(opts(), None).await?;
    let blue =
        Decision::Answer(Vec::from([Answer(Vec::from([String::from(
            "Blue",
        )]))]));
    turn(&mut rec, &a, "ask", blue, Interrupt::Never).await?;
    turn(
        &mut rec,
        &a,
        "touch",
        Decision::AcceptForSession,
        Interrupt::Never,
    )
    .await?;
    turn(&mut rec, &a, "again", Decision::Decline, Interrupt::Never).await?;
    turn(
        &mut rec,
        &a,
        "ask again",
        Decision::Decline,
        Interrupt::OnRequest,
    )
    .await?;
    assert_eq!(server.shutdown().await?.describe(), "exit status: 0");
    rec.until(Goal::Exit, Interrupt::Never).await?;
    assert_eq!(
        rec.seen,
        [
            "A started",
            "A turn started",
            "A approval integer:0 userInput [Question { header: \"Color\", text: \"Which color do you prefer?\", options: [QuestionOption { label: \"Red\", description: \"Choose Red.\" }, QuestionOption { label: \"Blue\", description: \"Choose Blue.\" }], selection: One }]",
            "A streams [\"Blue\"]",
            "A says \"Blue\"",
            "A turn completed",
            "A turn started",
            "A streams [\"I\", \"’ll\", \" run\", \" the\", \" command\", \" exactly\", \" as\", \" given\", \".\"]",
            "A says \"I’ll run the command exactly as given.\"",
            "A started command",
            "A approval integer:1 command",
            "A finished tool Succeeded",
            "A reasons \"\"",
            "A says \"\"",
            "A turn completed",
            "A turn started",
            "A started command",
            "A finished tool Succeeded",
            "A says \"\"",
            "A turn completed",
            "A turn started",
            "A approval integer:2 userInput [Question { header: \"Fruit\", text: \"Which fruit do you prefer?\", options: [QuestionOption { label: \"Apple\", description: \"Choose Apple.\" }, QuestionOption { label: \"Pear\", description: \"Choose Pear.\" }], selection: One }]",
            "A turn interrupted",
            "exited exit status: 0",
        ]
    );
    assert_eq!(rec.unknown.get("warning"), Some(&1));
    Ok(())
}

fn first_thread_id(fixture: &Path) -> TestResult<String> {
    let messages = std::fs::read_to_string(fixture)?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<Value>, _>>()?;
    let started = messages
        .iter()
        .find(|m| {
            m.get("method").and_then(Value::as_str) == Some("thread/started")
        })
        .ok_or("the fixture starts no thread")?;
    Ok(started
        .pointer("/params/thread/id")
        .and_then(Value::as_str)
        .ok_or("thread/started names no thread id")?
        .to_owned())
}

/// Each refusal recording has its own directory, even when tests run together.
fn refusal_options(name: &str, recording: &str) -> TestResult<ServerOptions> {
    let cwd = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/tmp")
        .join(format!("codex-refusal-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(&cwd)?;
    std::fs::write(cwd.join("stdout.jsonl"), recording)?;
    Ok(ServerOptions {
        codex_path: fake_cli_path(),
        home: Home::User,
        credential: None,
        cwd,
    })
}

#[tokio::test]
async fn initialize_exit_reports_last_nonempty_stderr_line() -> TestResult<()> {
    let options = refusal_options(
        "initialize-stderr",
        r#"{"fake":"fail","stderr":"loading config\ninvalid provider configuration\n\n","status":1}"#,
    )?;
    let cwd = options.cwd.clone();
    let error = CodexServer::spawn(options)
        .await
        .err()
        .ok_or("expected a value")?;
    assert_eq!(
        error.to_string(),
        "the codex app-server has exited: invalid provider configuration"
    );
    std::fs::remove_dir_all(cwd)?;
    Ok(())
}

#[tokio::test]
async fn initialize_exit_without_stderr_keeps_exit_error() -> TestResult<()> {
    let options = refusal_options(
        "initialize-empty",
        r#"{"fake":"fail","stderr":"","status":1}"#,
    )?;
    let cwd = options.cwd.clone();
    let error = CodexServer::spawn(options)
        .await
        .err()
        .ok_or("expected a value")?;
    assert_eq!(error.to_string(), "the codex app-server has exited");
    std::fs::remove_dir_all(cwd)?;
    Ok(())
}

#[tokio::test]
async fn initialize_rejection_reports_json_rpc_message() -> TestResult<()> {
    let options = refusal_options(
        "initialize-rpc",
        r#"{"id":1,"error":{"code":-32600,"message":"unsupported initialize request"}}"#,
    )?;
    let cwd = options.cwd.clone();
    let error = CodexServer::spawn(options)
        .await
        .err()
        .ok_or("expected a value")?;
    assert_eq!(
        error.to_string(),
        "codex rejected initialize: unsupported initialize request"
    );
    std::fs::remove_dir_all(cwd)?;
    Ok(())
}

async fn thread_refusal(
    method: &str,
    failure: &str,
    expected: &str,
) -> TestResult<()> {
    let recording = format!(
        "{{\"id\":1,\"result\":{{\"userAgent\":\"conductor/0.166.0\"}}}}\n{{\"fake\":\"await\",\"match\":{{\"method\":\"{method}\"}}}}\n{failure}\n"
    );
    let options = refusal_options(&format!("{method}-{expected}"), &recording)?;
    let cwd = options.cwd.clone();
    let (server, _events) = CodexServer::spawn(options).await?;
    let error = match method {
        "thread/start" => server
            .open_thread(opts(), None)
            .await
            .err()
            .ok_or("thread/start succeeded")?,
        "thread/resume" => server
            .open_thread(opts(), Some(ThreadId(String::from("missing"))))
            .await
            .err()
            .ok_or("thread/resume succeeded")?,
        _ => Error::Command(format!("unexpected test method {method}")),
    };
    assert_eq!(error.to_string(), expected);
    // A later request must retain the same stderr reason after the actor exits.
    if let Error::Refused(_) = error {
        assert_eq!(
            server
                .open_thread(opts(), None)
                .await
                .err()
                .ok_or("a later thread/start succeeded")?
                .to_string(),
            expected
        );
    }
    let _ = server.shutdown().await?;
    std::fs::remove_dir_all(cwd)?;
    Ok(())
}

#[tokio::test]
async fn start_and_resume_exit_report_stderr() -> TestResult<()> {
    thread_refusal(
        "thread/start",
        r#"{"fake":"fail","stderr":"could not create thread storage","status":1}"#,
        "the codex app-server has exited: could not create thread storage",
    )
    .await?;
    thread_refusal(
        "thread/resume",
        r#"{"fake":"fail","stderr":"could not load thread storage","status":1}"#,
        "the codex app-server has exited: could not load thread storage",
    )
    .await
}

#[tokio::test]
async fn start_and_resume_rejection_report_json_rpc_messages() -> TestResult<()>
{
    thread_refusal(
        "thread/start",
        r#"{"id":2,"error":{"code":-32600,"message":"invalid working directory"}}"#,
        "codex rejected thread/start: invalid working directory",
    )
    .await?;
    thread_refusal(
        "thread/resume",
        r#"{"id":2,"error":{"code":-32600,"message":"thread not found"}}"#,
        "codex rejected thread/resume: thread not found",
    )
    .await
}

struct RevertFixture {
    server: CodexServer,
    thread: CodexThread,
    cwd: PathBuf,
}

async fn revert_fixture(
    name: &str,
    outcome: &str,
) -> TestResult<RevertFixture> {
    let recording = format!(
        "{{\"id\":1,\"result\":{{\"userAgent\":\"conductor/0.166.0\"}}}}\n\
         {{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"th_undo\"}},\"model\":\"gpt-6-luna\",\"reasoningEffort\":\"low\"}}}}\n\
         {{\"id\":3,\"result\":{{\"turn\":{{\"id\":\"tu_first\"}}}}}}\n\
         {{\"id\":4,\"result\":{{\"turn\":{{\"id\":\"tu_discard\"}}}}}}\n\
         {outcome}\n\
         {{\"id\":6,\"result\":{{\"turn\":{{\"id\":\"tu_after\"}}}}}}\n"
    );
    let options = refusal_options(name, &recording)?;
    let cwd = options.cwd.clone();
    let (server, _events) = CodexServer::spawn(options).await?;
    let thread = server.open_thread(opts(), None).await?;
    let _ = thread.send_turn(String::from("retain"), None).await?;
    let _ = thread.send_turn(String::from("discard"), None).await?;
    Ok(RevertFixture {
        server,
        thread,
        cwd,
    })
}

#[tokio::test]
async fn revert_targets_native_turn_and_keeps_thread_usable() -> TestResult<()>
{
    let live = revert_fixture(
        "revert-ok",
        r#"{"id":5,"result":{"thread":{"id":"th_undo","turns":[]}}}"#,
    )
    .await?;
    live.thread
        .revert(TurnId(String::from("tu_discard")))
        .await?;
    assert_eq!(
        live.thread
            .send_turn(String::from("after undo"), None)
            .await?,
        TurnId(String::from("tu_after"))
    );
    let _ = live.server.shutdown().await?;
    let sent = std::fs::read_to_string(live.cwd.join("stdin.jsonl"))?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<Value>, _>>()?
        .into_iter()
        .find(|line| {
            line.get("method").and_then(Value::as_str) == Some("thread/revert")
        })
        .ok_or("expected a value")?;
    assert_eq!(
        sent.get("params"),
        Some(&serde_json::from_str::<Value>(
            r#"{"threadId":"th_undo","beforeTurnId":"tu_discard"}"#
        )?)
    );
    std::fs::remove_dir_all(live.cwd)?;
    Ok(())
}

#[tokio::test]
async fn revert_refusal_preserves_reason_and_does_not_close_thread()
-> TestResult<()> {
    let live = revert_fixture(
        "revert-refused",
        r#"{"id":5,"error":{"code":-32600,"message":"cannot revert an active turn"}}"#,
    )
    .await?;
    let error = live
        .thread
        .revert(TurnId(String::from("tu_discard")))
        .await
        .err()
        .ok_or("the revert succeeded")?;
    assert_eq!(
        error.to_string(),
        "codex rejected thread/revert: cannot revert an active turn"
    );
    assert_eq!(
        live.thread
            .send_turn(String::from("after refusal"), None)
            .await?,
        TurnId(String::from("tu_after"))
    );
    let _ = live.server.shutdown().await?;
    std::fs::remove_dir_all(live.cwd)?;
    Ok(())
}

async fn history_fixture(name: &str, pages: &str) -> TestResult<RevertFixture> {
    let recording = format!(
        "{{\"id\":1,\"result\":{{\"userAgent\":\"conductor/0.166.0\"}}}}\n\
         {{\"id\":2,\"result\":{{\"thread\":{{\"id\":\"th_history\"}},\"model\":\"gpt-6-luna\",\"reasoningEffort\":\"low\"}}}}\n{pages}\n"
    );
    let options = refusal_options(name, &recording)?;
    let cwd = options.cwd.clone();
    let (server, _events) = CodexServer::spawn(options).await?;
    let thread = server.open_thread(opts(), None).await?;
    Ok(RevertFixture {
        server,
        thread,
        cwd,
    })
}

#[tokio::test]
async fn history_reads_every_page_in_chronological_order() -> TestResult<()> {
    let live = history_fixture(
        "history-pages",
        r#"{"id":3,"result":{"data":[{"id":"first"},{"id":"second"}],"nextCursor":"after-second"}}
{"id":4,"result":{"data":[{"id":"third"}],"nextCursor":null}}"#,
    )
    .await?;
    assert_eq!(
        live.thread.history_turns().await?,
        Vec::from([
            TurnId(String::from("first")),
            TurnId(String::from("second")),
            TurnId(String::from("third"))
        ])
    );
    let _ = live.server.shutdown().await?;
    let requests = std::fs::read_to_string(live.cwd.join("stdin.jsonl"))?
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<Result<Vec<Value>, _>>()?
        .into_iter()
        .filter(|line| {
            line.get("method").and_then(Value::as_str)
                == Some("thread/turns/list")
        })
        .filter_map(|line| line.get("params").cloned())
        .collect::<Vec<Value>>();
    assert_eq!(
        requests,
        Vec::from([
            serde_json::from_str::<Value>(
                r#"{"threadId":"th_history","limit":100,"sortDirection":"asc"}"#
            )?,
            serde_json::from_str::<Value>(
                r#"{"threadId":"th_history","cursor":"after-second","limit":100,"sortDirection":"asc"}"#
            )?,
        ])
    );
    std::fs::remove_dir_all(live.cwd)?;
    Ok(())
}

#[tokio::test]
async fn endless_history_cursor_fails_instead_of_returning_partial_prefix()
-> TestResult<()> {
    let pages = std::ops::RangeInclusive::new(3,102).map(|id|format!("{{\"id\":{id},\"result\":{{\"data\":[{{\"id\":\"partial\"}}],\"nextCursor\":\"endless\"}}}}"))
        .collect::<Vec<String>>().join("\n");
    let live = history_fixture("history-bound", &pages).await?;
    let error = tokio::time::timeout(
        Duration::from_secs(20),
        live.thread.history_turns(),
    )
    .await?
    .err()
    .ok_or("the endless history was read")?;
    assert_eq!(
        error.to_string(),
        "codex history exceeded 100 pages; refusing an incomplete undo check"
    );
    let _ = live.server.shutdown().await?;
    std::fs::remove_dir_all(live.cwd)?;
    Ok(())
}
