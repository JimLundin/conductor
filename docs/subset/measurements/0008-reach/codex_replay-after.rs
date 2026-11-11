//! Replays recorded `codex app-server` stdout (fixtures/codex) through a real
//! `CodexServer`. A stand-in process prints the recording, holding each
//! response until our request with that id arrives and each
//! `serverRequest/resolved` until we answer that request, as Codex does. The
//! actor, the boundary mapping and the exactly-once approval bookkeeping all
//! run against Codex's real bytes, driven by the same calls `s2` made.
#![cfg(unix)]

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use agents::codex::{
    ApprovalId, ApprovalKind, ApprovalPolicy, CodexEvent, CodexServer, CodexThread, Decision, Item,
    Sandbox, ServerOptions, ThreadEvent, ThreadId, ThreadOptions, TurnId, TurnOutcome,
};
use n0_future::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;

const FAKE_CODEX: &str = r#"#!/usr/bin/python3 -I
import json, sys, threading
requests, answers = set(), set()
seen = threading.Condition()
def read_stdin():
    for line in sys.stdin:
        msg = json.loads(line)
        with seen:
            if "id" in msg:
                (requests if "method" in msg else answers).add(msg["id"])
            seen.notify_all()
    with seen:
        requests.add("eof")
        seen.notify_all()
reader = threading.Thread(target=read_stdin)
reader.start()
for line in open(sys.argv[-1]):
    msg = json.loads(line)
    with seen:
        if "id" in msg and "method" not in msg:
            seen.wait_for(lambda: msg["id"] in requests or "eof" in requests)
        elif msg.get("method") == "serverRequest/resolved":
            # Codex also resolves requests it withdrew, which nobody answers.
            seen.wait_for(lambda: msg["params"]["requestId"] in answers, timeout=5)
    sys.stdout.write(line)
    sys.stdout.flush()
reader.join()
"#;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/codex")
        .join(name)
}

/// A working directory for `tests/fake-codex.sh`, which runs `FAKE_CODEX`
/// on the recording found there. The script is committed, not written here:
/// executing a file another test thread may have just written fails with
/// "Text file busy" when a concurrent fork still holds it open.
fn fake_codex_dir(fixture: &Path) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agents-codex-replay-{}-{}",
        std::process::id(),
        fixture.file_stem().unwrap().to_str().unwrap()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("fake-codex.py"), FAKE_CODEX).unwrap();
    std::fs::copy(fixture, dir.join("stdout.jsonl")).unwrap();
    dir
}

fn opts() -> ThreadOptions {
    ThreadOptions {
        cwd: "/tmp/s2-scratch".into(),
        model: Some(String::from("gpt-6-luna")),
        effort: Some(String::from("low")),
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
    unknown: BTreeMap<String, usize>,
    /// Every approval id answered so far.
    answered: Vec<ApprovalId>,
}

impl Recorder {
    async fn until(&mut self, goal: Goal, interrupt: Interrupt) {
        self.goal = goal;
        self.interrupt = interrupt;
        loop {
            let event = self.next().await;
            match self.on(event).await {
                Progress::Playing => {}
                Progress::Done => break,
            }
        }
    }

    /// Starts a turn on `thread` and records it until it completes.
    async fn turn(
        &mut self,
        thread: &CodexThread,
        text: &str,
        decision: Decision,
        interrupt: Interrupt,
    ) {
        let turn = thread.send_turn(String::from(text)).await.unwrap();
        let goal = Goal::Turn {
            thread: thread.clone(),
            turn,
            decision,
        };
        self.until(goal, interrupt).await;
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
                    (ThreadEvent::ApprovalRequested { id, .. }, _) => {
                        self.server
                            .respond(id.clone(), decision.clone())
                            .await
                            .unwrap();
                        self.answered.push(id);
                        Progress::Playing
                    }
                    (
                        ThreadEvent::AgentMessage { .. } | ThreadEvent::Item { .. },
                        Interrupt::AfterFirstOutput,
                    ) => {
                        expected.interrupt(turn.clone()).await.unwrap();
                        self.interrupt = Interrupt::Sent;
                        Progress::Playing
                    }
                    (ThreadEvent::TurnCompleted { turn: done, .. }, _) => {
                        match done.0.cmp(&turn.0) {
                            Ordering::Equal => Progress::Done,
                            Ordering::Less | Ordering::Greater => Progress::Playing,
                        }
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
            CodexEvent::Thread { thread, event } => {
                let line = format!("{} {}", self.name(thread), summary(event));
                self.seen.push(line);
            }
            CodexEvent::Exited { status } => self.seen.push(format!("exited {status}")),
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
        ThreadEvent::TurnStarted { .. } => String::from("turn started"),
        ThreadEvent::TurnCompleted { outcome, .. } => format!("turn {}", outcome_name(outcome)),
        ThreadEvent::AgentMessage { text, .. } => format!("says {text:?}"),
        ThreadEvent::Item { phase, item, .. } => match item {
            Item::Command { status, .. } => format!("{phase:?} command {status}"),
            Item::FileChange {
                changes, status, ..
            } => {
                format!("{phase:?} fileChange {} {status}", changes.len())
            }
        },
        ThreadEvent::ApprovalRequested {
            id, kind, options, ..
        } => {
            let kind = match kind {
                ApprovalKind::Command { .. } => "command",
                ApprovalKind::FileChange { .. } => "fileChange",
                ApprovalKind::Permissions { .. } => "permissions",
                ApprovalKind::UserInput { .. } => "userInput",
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
        unknown: BTreeMap::new(),
        answered: Vec::new(),
    }
}

async fn answer_again(server: &CodexServer, id: ApprovalId) -> String {
    match server.respond(id, Decision::Accept).await {
        Ok(()) => String::from("ok"),
        Err(e) => e.to_string(),
    }
}

#[tokio::test]
async fn approve_decline_interrupt_and_two_threads() {
    let mut rec = spawn("server1.jsonl").await;
    let server = rec.server.clone();
    assert_eq!(server.codex_version(), Some("0.166.0"));
    let models = server.list_models().await.unwrap();
    assert_eq!(
        models.iter().find_map(|m| match m.id.as_str() {
            "gpt-6-luna" => Some(m.id.as_str()),
            _ => None,
        }),
        Some("gpt-6-luna")
    );

    let a = server.start_thread(opts()).await.unwrap();
    rec.turn(&a, "a.txt", Decision::Accept, Interrupt::Never)
        .await;
    rec.turn(&a, "b.txt", Decision::Decline, Interrupt::Never)
        .await;
    rec.turn(&a, "long", Decision::Decline, Interrupt::AfterFirstOutput)
        .await;
    rec.turn(&a, "pong", Decision::Decline, Interrupt::Never)
        .await;

    let b = server.start_thread(opts()).await.unwrap();
    let c = server.start_thread(opts()).await.unwrap();
    let (tb, tc) = n0_future::future::try_zip(
        b.send_turn(String::from("alpha")),
        c.send_turn(String::from("bravo")),
    )
    .await
    .unwrap();
    rec.open = HashMap::from([(b.id.clone(), tb), (c.id.clone(), tc)]);
    rec.until(Goal::Turns, Interrupt::Never).await;

    let again = n0_future::stream::iter(rec.answered.clone())
        .then(|id| answer_again(&server, id))
        .collect::<Vec<String>>()
        .await;
    assert_eq!(server.shutdown().await.unwrap().code(), Some(0));
    rec.until(Goal::Exit, Interrupt::Never).await;

    assert_eq!(
        rec.seen,
        [
            "A started",
            // 1: approve. The first write fails in the read-only sandbox
            // without asking; the retry asks.
            "A turn started",
            "A says \"I’ll create the requested file with exactly that content.\"",
            "A Started command inProgress",
            "A Completed command failed",
            "A Started command inProgress",
            "A approval 0 command (3 options)",
            "A Completed command completed",
            "A says \"Created `a.txt` containing exactly `hello from s2`.\"",
            "A turn completed",
            // 2: decline.
            "A turn started",
            "A says \"I’ll request permission to write `b.txt`; if it’s declined, I’ll stop.\"",
            "A Started command inProgress",
            "A approval 1 command (3 options)",
            "A Completed command declined",
            "A says \"The request to create `b.txt` was declined.\"",
            "A turn completed",
            // 3: interrupt after the first output, then a normal turn.
            "A turn started",
            "A says \"starting\"",
            "A turn interrupted",
            "A turn started",
            "A says \"pong\"",
            "A turn completed",
            // 4: two threads with turns in flight on one process.
            "B started",
            "C started",
            "C turn started",
            "B turn started",
            "C says \"bravo\"",
            "C turn completed",
            "B says \"alpha\"",
            "B turn completed",
            "exited exit status: 0",
        ]
    );
    assert_eq!(
        again,
        ["no open approval with id 0", "no open approval with id 1",]
    );
    // Item kinds we do not map (userMessage, reasoning) and agentMessage
    // starts stay Unknown, alongside the frames §7.2 does not ask for.
    assert_eq!(
        rec.unknown,
        BTreeMap::from([
            ("account/rateLimits/updated".to_owned(), 9),
            ("hook/completed".to_owned(), 3),
            ("hook/started".to_owned(), 3),
            ("item/agentMessage/delta".to_owned(), 58),
            ("item/commandExecution/outputDelta".to_owned(), 1),
            ("item/completed".to_owned(), 8),
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
    let mut rec = spawn("server2-resume.jsonl").await;
    let server = rec.server.clone();
    let a = ThreadId(first_thread_id(&fixture("server1.jsonl")).expect("server1 started a thread"));
    let resumed = server.resume_thread(a.clone(), opts()).await.unwrap();
    assert_eq!(resumed.id, a);
    rec.turn(&resumed, "which file?", Decision::Decline, Interrupt::Never)
        .await;
    let bogus = server
        .respond(ApprovalId(String::from("never-sent")), Decision::Accept)
        .await
        .map_err(|e| e.to_string());
    assert_eq!(server.shutdown().await.unwrap().code(), Some(0));
    rec.until(Goal::Exit, Interrupt::Never).await;

    assert_eq!(
        rec.seen,
        [
            // `thread/resume` sends no `thread/started`.
            "A turn started",
            "A says \"a.txt\"",
            "A turn completed",
            "exited exit status: 0",
        ]
    );
    assert_eq!(bogus, Err("no open approval with id never-sent".to_owned()));
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
