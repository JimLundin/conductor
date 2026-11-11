//! Replays recorded `codex app-server` stdout (fixtures/codex) through a real
//! `CodexServer`. A stand-in process prints the recording, holding each
//! response until our request with that id arrives and each
//! `serverRequest/resolved` until we answer that request, as Codex does. The
//! actor, the boundary mapping and the exactly-once approval bookkeeping all
//! run against Codex's real bytes, driven by the same calls `s2` made.
#![cfg(unix)]

use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agents::codex::{
    ApprovalId, ApprovalKind, ApprovalPolicy, CodexEvent, CodexServer, CodexThread, Decision, Item,
    Sandbox, ServerOptions, ThreadEvent, ThreadId, ThreadOptions, TurnId, TurnOutcome,
};
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

/// A `codex` stand-in that ignores its arguments except the fixture path,
/// which `command()` cannot pass, so it is baked into a wrapper script.
fn fake_codex(fixture: &Path) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agents-codex-replay-{}-{}",
        std::process::id(),
        fixture.file_stem().unwrap().to_str().unwrap()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let fake = dir.join("fake-codex.py");
    std::fs::write(&fake, FAKE_CODEX).unwrap();
    let script = dir.join("codex");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nexec /usr/bin/python3 -I '{}' '{}'\n",
            fake.display(),
            fixture.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

fn opts() -> ThreadOptions {
    ThreadOptions {
        cwd: "/tmp/s2-scratch".into(),
        model: Some("gpt-6-luna".into()),
        effort: Some("low".into()),
        approval_policy: ApprovalPolicy::OnRequest,
        sandbox: Sandbox::ReadOnly,
    }
}

/// Collects the event stream as readable lines. Thread ids become `A`, `B`,
/// ... in order of first appearance; unmapped frames are tallied by method
/// instead of listed, since deltas and status frames dominate the stream.
struct Recorder {
    events: mpsc::Receiver<CodexEvent>,
    names: HashMap<ThreadId, char>,
    seen: Vec<String>,
    unknown: BTreeMap<String, usize>,
}

impl Recorder {
    fn name(&mut self, thread: &ThreadId) -> char {
        let next = (b'A' + self.names.len() as u8) as char;
        *self.names.entry(thread.clone()).or_insert(next)
    }

    async fn next(&mut self) -> CodexEvent {
        let event = tokio::time::timeout(Duration::from_secs(20), self.events.recv())
            .await
            .expect("no event within 20s; the replay is stuck")
            .expect("event stream ended early");
        let line = match &event {
            CodexEvent::Thread { thread, event } => {
                summary(event).map(|s| format!("{} {s}", self.name(thread)))
            }
            CodexEvent::Exited { status } => Some(format!("exited {status}")),
            CodexEvent::Unknown(_) => None,
        };
        match (line, &event) {
            (Some(line), _) => self.seen.push(line),
            (
                None,
                CodexEvent::Unknown(raw)
                | CodexEvent::Thread {
                    event: ThreadEvent::Unknown(raw),
                    ..
                },
            ) => *self.unknown.entry(method(raw)).or_default() += 1,
            (None, _) => unreachable!("summary covers every other event"),
        }
        event
    }

    /// Reads events for `thread` until its turn completes, answering every
    /// approval with `decision` and interrupting after the first output when
    /// `interrupt` is set. Returns the ids it answered.
    async fn turn(
        &mut self,
        server: &CodexServer,
        thread: &CodexThread,
        turn: &TurnId,
        decision: Decision,
        interrupt: bool,
    ) -> Vec<ApprovalId> {
        let mut answered = Vec::new();
        let mut interrupted = !interrupt;
        loop {
            let CodexEvent::Thread { thread: t, event } = self.next().await else {
                continue;
            };
            assert_eq!(t, thread.id);
            match event {
                ThreadEvent::ApprovalRequested { id, .. } => {
                    server.respond(id.clone(), decision.clone()).await.unwrap();
                    answered.push(id);
                }
                ThreadEvent::AgentMessage { .. } | ThreadEvent::Item { .. } if !interrupted => {
                    thread.interrupt(turn.clone()).await.unwrap();
                    interrupted = true;
                }
                ThreadEvent::TurnCompleted { turn: done, .. } if &done == turn => {
                    return answered;
                }
                _ => {}
            }
        }
    }
}

fn method(raw: &serde_json::Value) -> String {
    raw["method"].as_str().unwrap_or("<no method>").to_owned()
}

fn summary(event: &ThreadEvent) -> Option<String> {
    Some(match event {
        ThreadEvent::Started => "started".into(),
        ThreadEvent::TurnStarted { .. } => "turn started".into(),
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
        ThreadEvent::Unknown(_) => return None,
    })
}

fn outcome_name(outcome: &TurnOutcome) -> String {
    match outcome {
        TurnOutcome::Completed => "completed".into(),
        TurnOutcome::Interrupted => "interrupted".into(),
        TurnOutcome::Failed { message } => format!("failed {message:?}"),
    }
}

async fn spawn(name: &str) -> (CodexServer, Recorder) {
    let (server, events) = CodexServer::spawn(ServerOptions {
        codex_path: fake_codex(&fixture(name)),
        cwd: std::env::temp_dir(),
        stdout_log: None,
    })
    .await
    .expect("initialize is answered by the recording");
    let recorder = Recorder {
        events,
        names: HashMap::new(),
        seen: Vec::new(),
        unknown: BTreeMap::new(),
    };
    (server, recorder)
}

#[tokio::test]
async fn approve_decline_interrupt_and_two_threads() {
    let (server, mut rec) = spawn("server1.jsonl").await;
    assert_eq!(server.codex_version(), Some("0.166.0"));
    let models = server.list_models().await.unwrap();
    assert!(models.iter().any(|m| m.id == "gpt-6-luna"));

    let a = server.start_thread(opts()).await.unwrap();
    let turn = a.send_turn("a.txt".into()).await.unwrap();
    let approved = rec.turn(&server, &a, &turn, Decision::Accept, false).await;
    let turn = a.send_turn("b.txt".into()).await.unwrap();
    let declined = rec.turn(&server, &a, &turn, Decision::Decline, false).await;
    let turn = a.send_turn("long".into()).await.unwrap();
    rec.turn(&server, &a, &turn, Decision::Decline, true).await;
    let turn = a.send_turn("pong".into()).await.unwrap();
    rec.turn(&server, &a, &turn, Decision::Decline, false).await;

    let b = server.start_thread(opts()).await.unwrap();
    let c = server.start_thread(opts()).await.unwrap();
    let (tb, tc) =
        tokio::try_join!(b.send_turn("alpha".into()), c.send_turn("bravo".into())).unwrap();
    let mut open = vec![(b.id.clone(), tb), (c.id.clone(), tc)];
    while !open.is_empty() {
        if let CodexEvent::Thread {
            thread,
            event: ThreadEvent::TurnCompleted { turn, .. },
        } = rec.next().await
        {
            let before = open.len();
            open.retain(|(t, tu)| !(t == &thread && tu == &turn));
            assert_eq!(
                open.len(),
                before - 1,
                "turn {turn:?} belongs to {thread:?}"
            );
        }
    }

    let mut again = Vec::new();
    for id in approved.into_iter().chain(declined) {
        again.push(match server.respond(id, Decision::Accept).await {
            Ok(()) => "ok".to_owned(),
            Err(e) => e.to_string(),
        });
    }
    let status = server.shutdown().await.unwrap();
    assert!(status.success());
    while !matches!(rec.next().await, CodexEvent::Exited { .. }) {}

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
    let (server, mut rec) = spawn("server2-resume.jsonl").await;
    let a = ThreadId(first_thread_id(&fixture("server1.jsonl")).expect("server1 started a thread"));
    let resumed = server.resume_thread(a.clone(), opts()).await.unwrap();
    assert_eq!(resumed.id, a);
    let turn = resumed.send_turn("which file?".into()).await.unwrap();
    rec.turn(&server, &resumed, &turn, Decision::Decline, false)
        .await;
    let bogus = server
        .respond(ApprovalId("never-sent".into()), Decision::Accept)
        .await
        .map_err(|e| e.to_string());
    let status = server.shutdown().await.unwrap();
    assert!(status.success());
    while !matches!(rec.next().await, CodexEvent::Exited { .. }) {}

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
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .find(|m| m["method"] == "thread/started")
        .and_then(|m| m.pointer("/params/thread/id")?.as_str().map(str::to_owned))
}
