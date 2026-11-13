//! Replays recorded `codex app-server` stdout (fixtures/codex) through a real
//! `CodexServer`. A stand-in process prints the recording, holding each
//! response until our request with that id arrives and each
//! `serverRequest/resolved` until we answer that request, as Codex does. The
//! actor, the boundary mapping and the exactly-once approval bookkeeping all
//! run against Codex's real bytes, driven by the same calls `s2` made.
#![cfg(unix)]

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use agents::Home;
use agents::codex::{
    ApprovalId, ApprovalKind, ApprovalPolicy, CodexEvent, CodexServer, CodexThread, Decision,
    Sandbox, ServerOptions, ThreadEvent, ThreadId, ThreadOptions, ThreadSettings, TurnId,
};
use n0_future::StreamExt;
use proto::{Answer, Effort, ToolKind, ToolPhase, TurnOutcome};
use serde_json::Value;
use tokio::sync::mpsc;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/codex")
        .join(name)
}

/// A working directory for `tests/fake-codex.sh`, which plays the recording
/// found there. The script is committed, not written here: executing a file
/// another test thread may have just written fails with "Text file busy" when
/// a concurrent fork still holds it open.
fn fake_codex_dir(fixture: &Path) -> PathBuf {
    let dir = std::env::current_exe()
        .unwrap()
        .ancestors()
        .nth(3)
        .unwrap()
        .join("tmp")
        .join(format!(
            "agents-codex-replay-{}-{}",
            std::process::id(),
            fixture.file_stem().unwrap().to_str().unwrap()
        ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(fixture, dir.join("stdout.jsonl")).unwrap();
    dir
}

fn opts() -> ThreadOptions {
    ThreadOptions {
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
    answered: Vec<ApprovalId>,
}

impl Recorder {
    async fn until(self, goal: Goal, interrupt: Interrupt) -> Recorder {
        Recorder {
            goal,
            interrupt,
            ..self
        }
        .run()
        .await
    }

    async fn run(mut self) -> Recorder {
        loop {
            let event = self.next().await;
            match self.on(event).await {
                Progress::Playing => {}
                Progress::Done => break self,
            }
        }
    }

    async fn next(&mut self) -> CodexEvent {
        let event = tokio::time::timeout(Duration::from_secs(20), self.events.recv())
            .await
            .expect("no event within 20s; the replay is stuck")
            .expect("event stream ended early");
        self.record(&event);
        event
    }

    async fn on(&mut self, event: CodexEvent) -> Progress {
        match (&self.goal, event) {
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
                    (ThreadEvent::ApprovalRequested { .. }, Interrupt::OnRequest) => {
                        expected.interrupt(turn.clone()).await.unwrap();
                        self.interrupt = Interrupt::Sent;
                        Progress::Playing
                    }
                    (ThreadEvent::ApprovalRequested { id, .. }, _) => {
                        self.server
                            .respond(id.clone(), decision.clone())
                            .await
                            .unwrap();
                        self.answered.push(id);
                        Progress::Playing
                    }
                    (
                        ThreadEvent::AgentMessage { .. } | ThreadEvent::ToolCall { .. },
                        Interrupt::AfterFirstOutput,
                    ) => {
                        expected.interrupt(turn.clone()).await.unwrap();
                        self.interrupt = Interrupt::Sent;
                        Progress::Playing
                    }
                    (ThreadEvent::TurnCompleted { turn: done, .. }, _) if &done == turn => {
                        Progress::Done
                    }
                    _ => Progress::Playing,
                }
            }
            (Goal::Exit | Goal::Turns | Goal::Turn { .. }, _) => Progress::Playing,
        }
    }

    fn record(&mut self, event: &CodexEvent) {
        match event {
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
                    let line = format!("{} streams {deltas:?}", self.name(thread));
                    self.seen.push(line);
                }
                let line = format!("{} {}", self.name(thread), summary(event));
                self.seen.push(line);
            }
            CodexEvent::Exited { status } => {
                self.seen.push(format!("exited {}", status.describe()))
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
        ThreadEvent::SecretQuestionDeclined { .. } => String::from("secret question declined"),
        ThreadEvent::TurnStarted { .. } => String::from("turn started"),
        ThreadEvent::TurnCompleted { outcome, .. } => format!("turn {}", outcome_name(outcome)),
        ThreadEvent::AgentDelta { text, .. } => format!("delta {text:?}"),
        ThreadEvent::AgentMessage { text, .. } => format!("says {text:?}"),
        ThreadEvent::ReasoningDelta { text, .. } => format!("reasoning delta {text:?}"),
        ThreadEvent::Reasoning { text, .. } => format!("reasons {text:?}"),
        ThreadEvent::ToolCall { phase, .. } => match phase {
            ToolPhase::Started { tool } => match tool {
                ToolKind::Command { .. } => String::from("started command"),
                ToolKind::FileChange { files } => format!("started fileChange {}", files.len()),
                ToolKind::Other { .. } => String::from("started other"),
            },
            ToolPhase::Finished { outcome } => format!("finished tool {outcome:?}"),
        },
        ThreadEvent::ApprovalRequested {
            id, kind, options, ..
        } => {
            let kind = match kind {
                ApprovalKind::Command { .. } => "command",
                ApprovalKind::FileChange { .. } => "fileChange",
                ApprovalKind::Permissions { .. } => "permissions",
                ApprovalKind::UserInput { questions } => &format!("userInput {questions:?}"),
            };
            format!("approval {} {kind} ({} options)", id.0, options.len())
        }
        ThreadEvent::ApprovalCancelled { id } => format!("approval {} cancelled", id.0),
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

async fn spawn(name: &str) -> Recorder {
    let (server, events) = CodexServer::spawn(ServerOptions {
        codex_path: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-codex.sh"),
        home: Home::User,
        credential: None,
        cwd: fake_codex_dir(&fixture(name)),
        stdout_log: None,
    })
    .await
    .expect("initialize is answered by the recording");
    Recorder {
        server,
        events,
        goal: Goal::Exit,
        interrupt: Interrupt::Never,
        open: HashMap::new(),
        names: HashMap::new(),
        seen: Vec::new(),
        deltas: HashMap::new(),
        unknown: BTreeMap::new(),
        answered: Vec::new(),
    }
}

/// Starts a turn on `thread` and records it until it completes.
async fn turn(
    rec: Recorder,
    thread: &CodexThread,
    text: &str,
    decision: Decision,
    interrupt: Interrupt,
) -> Recorder {
    let turn = thread.send_turn(String::from(text)).await.unwrap();
    let goal = Goal::Turn {
        thread: thread.clone(),
        turn,
        decision,
    };
    rec.until(goal, interrupt).await
}

async fn answer_again(server: &CodexServer, id: ApprovalId) -> String {
    match server.respond(id, Decision::Accept).await {
        Ok(()) => String::from("ok"),
        Err(e) => e.to_string(),
    }
}

#[tokio::test]
async fn approve_decline_interrupt_and_two_threads() {
    let rec = spawn("server1.jsonl").await;
    let server = rec.server.clone();
    assert_eq!(server.codex_version(), Some("0.166.0"));
    let models = server.list_models().await.unwrap();
    assert_eq!(
        models.choices.iter().find_map(|m| match m.id.0.as_str() {
            "gpt-6-luna" => Some(m.id.0.as_str()),
            _ => None,
        }),
        Some("gpt-6-luna")
    );

    let a = server.start_thread(opts()).await.unwrap();
    let rec = turn(rec, &a, "a.txt", Decision::Accept, Interrupt::Never).await;
    let rec = turn(rec, &a, "b.txt", Decision::Decline, Interrupt::Never).await;
    let rec = turn(
        rec,
        &a,
        "long",
        Decision::Decline,
        Interrupt::AfterFirstOutput,
    )
    .await;
    let rec = turn(rec, &a, "pong", Decision::Decline, Interrupt::Never).await;

    let b = server.start_thread(opts()).await.unwrap();
    let c = server.start_thread(opts()).await.unwrap();
    let (tb, tc) = n0_future::future::try_zip(
        b.send_turn(String::from("alpha")),
        c.send_turn(String::from("bravo")),
    )
    .await
    .unwrap();
    let rec = Recorder {
        open: HashMap::from([(b.id.clone(), tb), (c.id.clone(), tc)]),
        ..rec
    }
    .until(Goal::Turns, Interrupt::Never)
    .await;

    let again = n0_future::stream::iter(rec.answered.clone())
        .then(|id| answer_again(&server, id))
        .collect::<Vec<String>>()
        .await;
    assert_eq!(
        server.shutdown().await.unwrap().describe(),
        "exit status: 0"
    );
    let rec = rec.until(Goal::Exit, Interrupt::Never).await;

    assert_eq!(
        rec.seen,
        [
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
            "A approval 0 command (3 options)",
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
            "A approval 1 command (3 options)",
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
        ]
    );
    assert_eq!(
        again,
        ["no open approval with id 0", "no open approval with id 1",]
    );
    // Item kinds we do not map (userMessage), reasoning and agentMessage
    // starts stay Unknown, alongside the frames §7.2 does not ask for.
    assert_eq!(
        rec.unknown,
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
    );
}

#[tokio::test]
async fn resume_in_a_new_process() {
    let rec = spawn("server2-resume.jsonl").await;
    let server = rec.server.clone();
    let a = ThreadId(first_thread_id(&fixture("server1.jsonl")).expect("server1 started a thread"));
    let resumed = server.resume_thread(a.clone(), opts()).await.unwrap();
    assert_eq!(resumed.id, a);
    let rec = turn(
        rec,
        &resumed,
        "which file?",
        Decision::Decline,
        Interrupt::Never,
    )
    .await;
    let bogus = server
        .respond(ApprovalId(String::from("never-sent")), Decision::Accept)
        .await
        .map_err(|e| e.to_string());
    assert_eq!(
        server.shutdown().await.unwrap().describe(),
        "exit status: 0"
    );
    let rec = rec.until(Goal::Exit, Interrupt::Never).await;

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
}

/// `s2` steps 10 and 11: a thread started with a chosen effort, which Codex
/// reports back, and a turn that reasons. The recording goes on to the turn
/// `s2` killed, which waits here for a `turn/start` nobody sends.
#[tokio::test]
async fn chosen_effort_and_reasoning() {
    let rec = spawn("server4-chosen-killed.jsonl").await;
    let server = rec.server.clone();
    let a = server.start_thread(opts()).await.unwrap();
    let rec = turn(
        rec,
        &a,
        "is 1000003 prime?",
        Decision::Decline,
        Interrupt::Never,
    )
    .await;
    assert_eq!(
        server.shutdown().await.unwrap().describe(),
        "exit status: 0"
    );
    let rec = rec.until(Goal::Exit, Interrupt::Never).await;
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
}

/// `s2` steps 7 to 9. The recording holds the question's
/// `serverRequest/resolved` until our answer, so a change to the question's
/// shape in a later pin fails here, as does a lost feature `warning`.
#[tokio::test]
async fn questions_and_accept_for_session() {
    let rec = spawn("server3-questions.jsonl").await;
    let server = rec.server.clone();
    let a = server.start_thread(opts()).await.unwrap();
    let blue = Decision::Answer(Vec::from([Answer(Vec::from([String::from("Blue")]))]));
    let rec = turn(rec, &a, "ask", blue, Interrupt::Never).await;
    let rec = turn(
        rec,
        &a,
        "touch",
        Decision::AcceptForSession,
        Interrupt::Never,
    )
    .await;
    let rec = turn(rec, &a, "again", Decision::Decline, Interrupt::Never).await;
    let rec = turn(
        rec,
        &a,
        "ask again",
        Decision::Decline,
        Interrupt::OnRequest,
    )
    .await;
    assert_eq!(
        server.shutdown().await.unwrap().describe(),
        "exit status: 0"
    );
    let rec = rec.until(Goal::Exit, Interrupt::Never).await;
    assert_eq!(
        rec.seen,
        [
            "A started",
            "A turn started",
            "A approval 0 userInput [Question { header: \"Color\", text: \"Which color do you prefer?\", options: [QuestionOption { label: \"Red\", description: \"Choose Red.\" }, QuestionOption { label: \"Blue\", description: \"Choose Blue.\" }], selection: One }] (0 options)",
            "A streams [\"Blue\"]",
            "A says \"Blue\"",
            "A turn completed",
            "A turn started",
            "A streams [\"I\", \"’ll\", \" run\", \" the\", \" command\", \" exactly\", \" as\", \" given\", \".\"]",
            "A says \"I’ll run the command exactly as given.\"",
            "A started command",
            "A approval 1 command (3 options)",
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
            "A approval 2 userInput [Question { header: \"Fruit\", text: \"Which fruit do you prefer?\", options: [QuestionOption { label: \"Apple\", description: \"Choose Apple.\" }, QuestionOption { label: \"Pear\", description: \"Choose Pear.\" }], selection: One }] (0 options)",
            "A turn interrupted",
            "exited exit status: 0",
        ]
    );
    assert_eq!(rec.unknown.get("warning"), Some(&1));
}

fn first_thread_id(fixture: &Path) -> Option<String> {
    std::fs::read_to_string(fixture)
        .ok()?
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find_map(|m| match m.get("method").and_then(Value::as_str) {
            Some("thread/started") => Some(m),
            _ => None,
        })
        .and_then(|m| m.pointer("/params/thread/id")?.as_str().map(str::to_owned))
}

/// Each refusal recording has its own directory, even when tests run together.
fn refusal_options(name: &str, recording: &str) -> ServerOptions {
    let cwd = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/tmp")
        .join(format!("codex-refusal-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cwd);
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(cwd.join("stdout.jsonl"), recording).unwrap();
    ServerOptions {
        codex_path: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-codex.sh"),
        home: Home::User,
        credential: None,
        cwd,
        stdout_log: None,
    }
}

#[tokio::test]
async fn initialize_exit_reports_last_nonempty_stderr_line() {
    let options = refusal_options(
        "initialize-stderr",
        r#"{"fake":"fail","stderr":"loading config\ninvalid provider configuration\n\n","status":1}"#,
    );
    let cwd = options.cwd.clone();
    let error = CodexServer::spawn(options).await.err().unwrap();
    assert_eq!(
        error.to_string(),
        "the codex app-server has exited: invalid provider configuration"
    );
    std::fs::remove_dir_all(cwd).unwrap();
}

#[tokio::test]
async fn initialize_exit_without_stderr_keeps_exit_error() {
    let options = refusal_options(
        "initialize-empty",
        r#"{"fake":"fail","stderr":"","status":1}"#,
    );
    let cwd = options.cwd.clone();
    let error = CodexServer::spawn(options).await.err().unwrap();
    assert_eq!(error.to_string(), "the codex app-server has exited");
    std::fs::remove_dir_all(cwd).unwrap();
}

#[tokio::test]
async fn initialize_rejection_reports_json_rpc_message() {
    let options = refusal_options(
        "initialize-rpc",
        r#"{"id":1,"error":{"code":-32600,"message":"unsupported initialize request"}}"#,
    );
    let cwd = options.cwd.clone();
    let error = CodexServer::spawn(options).await.err().unwrap();
    assert_eq!(
        error.to_string(),
        "codex rejected initialize: unsupported initialize request"
    );
    std::fs::remove_dir_all(cwd).unwrap();
}

async fn thread_refusal(method: &str, failure: &str, expected: &str) {
    let recording = format!(
        "{{\"id\":1,\"result\":{{\"userAgent\":\"conductor/0.166.0\"}}}}\n{{\"fake\":\"await\",\"match\":{{\"method\":\"{method}\"}}}}\n{failure}\n"
    );
    let options = refusal_options(&format!("{method}-{expected}"), &recording);
    let cwd = options.cwd.clone();
    let (server, _events) = CodexServer::spawn(options).await.unwrap();
    let error = match method {
        "thread/start" => server.start_thread(opts()).await.err().unwrap(),
        "thread/resume" => server
            .resume_thread(ThreadId(String::from("missing")), opts())
            .await
            .err()
            .unwrap(),
        _ => agents::codex::Error::Command(format!("unexpected test method {method}")),
    };
    assert_eq!(error.to_string(), expected);
    // A later request must retain the same stderr reason after the actor exits.
    if let agents::codex::Error::Refused(_) = error {
        assert_eq!(
            server.start_thread(opts()).await.err().unwrap().to_string(),
            expected
        );
    }
    let _ = server.shutdown().await.unwrap();
    std::fs::remove_dir_all(cwd).unwrap();
}

#[tokio::test]
async fn start_and_resume_exit_report_stderr() {
    thread_refusal(
        "thread/start",
        r#"{"fake":"fail","stderr":"could not create thread storage","status":1}"#,
        "the codex app-server has exited: could not create thread storage",
    )
    .await;
    thread_refusal(
        "thread/resume",
        r#"{"fake":"fail","stderr":"could not load thread storage","status":1}"#,
        "the codex app-server has exited: could not load thread storage",
    )
    .await;
}

#[tokio::test]
async fn start_and_resume_rejection_report_json_rpc_messages() {
    thread_refusal(
        "thread/start",
        r#"{"id":2,"error":{"code":-32600,"message":"invalid working directory"}}"#,
        "codex rejected thread/start: invalid working directory",
    )
    .await;
    thread_refusal(
        "thread/resume",
        r#"{"id":2,"error":{"code":-32600,"message":"thread not found"}}"#,
        "codex rejected thread/resume: thread not found",
    )
    .await;
}
