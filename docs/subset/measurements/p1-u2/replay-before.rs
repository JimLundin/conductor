//! Replays recorded `claude` stdout (fixtures/claude) through a real
//! `ClaudeSession`. A stand-in process prints the recording and then swallows
//! stdin, so the actor, the boundary mapping and the exactly-once permission
//! bookkeeping all run against the CLI's real bytes.
#![cfg(unix)]

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agents::claude::{
    AfterDeny, Block, ClaudeEvent, ClaudeOptions, ClaudeSession, Outcome, PermissionDecision,
    PermissionMode, PermissionRequestId, Role, SettingSource, VersionPolicy,
};
use n0_future::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/claude")
        .join(name)
}

/// A working directory for `tests/fake-claude.sh`, which prints the
/// recording found there and logs what it reads. The script is committed, not
/// written here: executing a file another test thread may have just written
/// fails with "Text file busy" when a concurrent fork still holds it open.
struct FakeClaude {
    dir: PathBuf,
    /// Every line the session wrote to the stand-in's stdin.
    stdin_log: PathBuf,
}

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("agents-replay-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fake_claude(name: &str, stdout: &Path) -> FakeClaude {
    let dir = test_dir(name);
    std::fs::copy(stdout, dir.join("stdout.jsonl")).unwrap();
    FakeClaude {
        stdin_log: dir.join("stdin.jsonl"),
        dir,
    }
}

fn options(claude: &FakeClaude) -> ClaudeOptions {
    ClaudeOptions {
        claude_path: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake-claude.sh"),
        cwd: claude.dir.clone(),
        model: None,
        permission_mode: PermissionMode::Default,
        resume: None,
        setting_sources: Some(Vec::from([SettingSource::Project])),
        stdout_log: None,
        versions: VersionPolicy::SupportedOnly,
    }
}

/// The CLI's own `is_error` spelling, so the summaries read like its JSON.
fn is_error(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Succeeded => "false",
        Outcome::Failed => "true",
    }
}

fn block_summary(block: &Block) -> String {
    match block {
        Block::Text(t) => format!("text {t:?}"),
        Block::Thinking(_) => String::from("thinking"),
        Block::ToolUse { name, detail, .. } => format!("tool_use {name} {detail:?}"),
        Block::ToolResult { outcome, .. } => format!("tool_result error={}", is_error(outcome)),
        Block::Other(_) => String::from("other"),
    }
}

fn summary(event: &ClaudeEvent) -> String {
    match event {
        ClaudeEvent::TurnStarted { session_id } => format!("turn_started {}", session_id.0),
        ClaudeEvent::Message { role, content } => {
            let blocks = content.iter().map(block_summary).collect::<Vec<String>>();
            let role = match role {
                Role::Assistant => "assistant",
                Role::User => "user",
            };
            format!("{role}: {}", blocks.join(", "))
        }
        ClaudeEvent::PermissionRequested {
            tool_name, detail, ..
        } => format!("permission {tool_name} {detail:?}"),
        ClaudeEvent::StreamDelta { text } => format!("delta {text:?}"),
        ClaudeEvent::PermissionCancelled { .. } => String::from("permission_cancelled"),
        ClaudeEvent::TurnFinished {
            subtype,
            outcome,
            terminal_reason,
            ..
        } => format!(
            "finished {subtype} is_error={} {terminal_reason:?}",
            is_error(outcome)
        ),
        ClaudeEvent::Exited { status } => format!("exited {}", status.describe()),
        ClaudeEvent::Unknown(v) => format!("unknown {}", frame(v)),
    }
}

/// An unmapped frame's `type/subtype`.
fn frame(v: &Value) -> String {
    format!(
        "{}/{}",
        v.get("type").and_then(Value::as_str).unwrap_or("?"),
        v.get("subtype").and_then(Value::as_str).unwrap_or("-")
    )
}

/// The `type` of a recorded stdout line.
fn frame_type(line: &str) -> Option<String> {
    serde_json::from_str::<Value>(line)
        .unwrap()
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Whether a replay has reached the event it reads until.
enum Progress {
    Playing,
    Done,
}

/// The event a replay reads until.
enum Until {
    /// The next permission request, left for the test to answer.
    Permission,
    /// The next `StreamDelta`.
    Delta,
    /// This many turns have finished since the session began.
    Turns(usize),
}

/// Reads a session's events, answering the n-th permission request with
/// `decisions[n]` and leaving the rest open, as the live run did before
/// interrupting. Unmapped frames are tallied by `type/subtype` instead of
/// listed, since stream events dominate the recording.
struct Replay {
    session: ClaudeSession,
    events: mpsc::Receiver<ClaudeEvent>,
    decisions: Vec<PermissionDecision>,
    until: Until,
    finished: usize,
    seen: Vec<String>,
    /// The deltas since the last other event, recorded as one line.
    deltas: Option<Vec<String>>,
    unknown: BTreeMap<String, usize>,
    ids: Vec<PermissionRequestId>,
}

impl Replay {
    fn new(
        session: ClaudeSession,
        events: mpsc::Receiver<ClaudeEvent>,
        decisions: Vec<PermissionDecision>,
    ) -> Replay {
        Replay {
            session,
            events,
            decisions,
            until: Until::Turns(0),
            finished: 0,
            seen: Vec::new(),
            deltas: None,
            unknown: BTreeMap::new(),
            ids: Vec::new(),
        }
    }

    async fn until(self, until: Until) -> Replay {
        Replay { until, ..self }.run().await
    }

    async fn run(mut self) -> Replay {
        loop {
            let event = tokio::time::timeout(Duration::from_secs(20), self.events.recv())
                .await
                .expect("no event within 20s; the replay is stuck")
                .expect("event stream ended early");
            match self.on(event).await {
                Progress::Playing => {}
                Progress::Done => break self,
            }
        }
    }

    async fn on(&mut self, event: ClaudeEvent) -> Progress {
        match &event {
            ClaudeEvent::PermissionRequested { id, .. } => self.answer(id).await,
            ClaudeEvent::TurnFinished { .. } => self.finished = self.finished.saturating_add(1),
            _ => {}
        }
        match &event {
            ClaudeEvent::Unknown(raw) => self.tally(frame(raw)),
            ClaudeEvent::StreamDelta { text } => {
                self.deltas.get_or_insert_with(Vec::new).push(text.clone())
            }
            _ => self.record(summary(&event)),
        }
        match (&self.until, &event) {
            (Until::Permission, ClaudeEvent::PermissionRequested { .. })
            | (Until::Delta, ClaudeEvent::StreamDelta { .. }) => Progress::Done,
            (Until::Turns(turns), _) => match self.finished.cmp(turns) {
                Ordering::Less => Progress::Playing,
                Ordering::Equal | Ordering::Greater => Progress::Done,
            },
            (Until::Permission | Until::Delta, _) => Progress::Playing,
        }
    }

    async fn answer(&mut self, id: &PermissionRequestId) {
        let answered = match self.decisions.get(self.ids.len()).cloned() {
            Some(decision) => self.session.respond(id.clone(), decision).await,
            None => Ok(()),
        };
        answered.unwrap();
        self.ids.push(id.clone());
    }

    /// Records a line, after one for the deltas that came before it.
    fn record(&mut self, line: String) {
        if let Some(deltas) = self.deltas.take() {
            self.seen.push(format!("streams {deltas:?}"));
        }
        self.seen.push(line);
    }

    fn tally(&mut self, frame: String) {
        let count = self
            .unknown
            .get(&frame)
            .copied()
            .unwrap_or(0)
            .saturating_add(1);
        self.unknown.insert(frame, count);
    }
}

/// What a replay saw, and what answering each permission id again returned
/// once the recording had played out.
struct Replayed {
    events: Vec<String>,
    unknown: BTreeMap<String, usize>,
    second: Vec<String>,
}

async fn answer_again(session: &ClaudeSession, id: PermissionRequestId) -> String {
    match session.respond(id, PermissionDecision::Allow).await {
        Ok(()) => String::from("ok"),
        Err(e) => e.to_string(),
    }
}

async fn replay(name: &str, decisions: Vec<PermissionDecision>) -> Replayed {
    let path = fixture(name);
    let (session, events) = ClaudeSession::spawn(options(&fake_claude(name, &path)))
        .await
        .expect("initialize is answered by the recording's first line");

    let recording = std::fs::read_to_string(&path).unwrap();
    let results = recording
        .lines()
        .filter(|line| matches!(frame_type(line).as_deref(), Some("result")))
        .count();
    let played = Replay::new(session, events, decisions)
        .until(Until::Turns(results))
        .await;
    assert_ne!(
        played.seen.len().cmp(&recording.lines().count()),
        Ordering::Greater
    );

    let session = played.session;
    let second = n0_future::stream::iter(played.ids)
        .then(|id| answer_again(&session, id))
        .collect::<Vec<String>>()
        .await;
    assert_eq!(
        session.shutdown().await.unwrap().describe(),
        "exit status: 0"
    );
    Replayed {
        events: played.seen,
        unknown: played.unknown,
        second,
    }
}

#[tokio::test]
async fn approve_deny_interrupt_recording() {
    let replayed = replay(
        "session1-approve-deny-interrupt.jsonl",
        Vec::from([
            PermissionDecision::Allow,
            PermissionDecision::Deny {
                message: String::from("no"),
                then: AfterDeny::Continue,
            },
        ]),
    )
    .await;
    assert_eq!(
        replayed.events,
        [
            // 1: approve a Write. Only text streams; tool input stays Unknown.
            "turn_started 03ef5463-f8da-4412-a8bd-0371a11cce56",
            "assistant: tool_use Write Path(\"/tmp/s1-scratch/a.txt\")",
            "permission Write Path(\"/tmp/s1-scratch/a.txt\")",
            "user: tool_result error=false",
            "streams [\"I\", \" created `a.txt` in\", \" `/tmp/s1\", \"-scratch` with the content\", \" `hello from s1`.\"]",
            "assistant: text \"I created `a.txt` in `/tmp/s1-scratch` with the content `hello from s1`.\"",
            "finished success is_error=false Some(\"completed\")",
            // 2: deny a Write.
            "turn_started 03ef5463-f8da-4412-a8bd-0371a11cce56",
            "assistant: tool_use Write Path(\"/tmp/s1-scratch/b.txt\")",
            "permission Write Path(\"/tmp/s1-scratch/b.txt\")",
            "user: tool_result error=true",
            "streams [\"The write to\", \" `\", \"b.txt` was denied\", \" \", \"(\\\"The user does not want b.\", \"txt.\\\"\", \"), so I did not retry and\", \" no file was created.\"]",
            "assistant: text \"The write to `b.txt` was denied (\\\"The user does not want b.txt.\\\"), so I did not retry and no file was created.\"",
            "finished success is_error=false Some(\"completed\")",
            // 3: interrupt as the turn starts, then a normal turn.
            "turn_started 03ef5463-f8da-4412-a8bd-0371a11cce56",
            "user: text \"[Request interrupted by user]\"",
            "finished error_during_execution is_error=true Some(\"aborted_streaming\")",
            "turn_started 03ef5463-f8da-4412-a8bd-0371a11cce56",
            "streams [\"pong\"]",
            "assistant: text \"pong\"",
            "finished success is_error=false Some(\"completed\")",
            // 3c: interrupt on the first delta; the cut-off message repeats it.
            "turn_started 03ef5463-f8da-4412-a8bd-0371a11cce56",
            "streams [\"1\"]",
            "assistant: text \"1\"",
            "user: text \"[Request interrupted by user]\"",
            "finished error_during_execution is_error=true Some(\"aborted_streaming\")",
            "turn_started 03ef5463-f8da-4412-a8bd-0371a11cce56",
            "streams [\"pong\"]",
            "assistant: text \"pong\"",
            "finished success is_error=false Some(\"completed\")",
            // 3d: interrupt right after sending; this time the CLI ran the turn.
            "turn_started 03ef5463-f8da-4412-a8bd-0371a11cce56",
            "streams [\"1\\n2\\n3\\n4\", \"\\n5\\n6\\n7\\n8\", \"\\n9\\n10\\n11\\n12\", \"\\n13\\n14\\n15\\n16\", \"\\n17\\n18\\n19\\n20\", \"\\n21\\n22\\n23\\n24\", \"\\n25\\n26\\n27\\n28\", \"\\n29\\n30\\n31\\n32\", \"\\n33\\n34\\n35\\n36\", \"\\n37\\n38\\n39\\n40\", \"\\n41\\n42\\n43\\n44\", \"\\n45\\n46\\n47\\n48\", \"\\n49\\n50\\n51\\n52\", \"\\n53\\n54\\n55\\n56\", \"\\n57\\n58\\n59\\n60\", \"\\n61\\n62\\n63\\n64\", \"\\n65\\n66\\n67\\n68\", \"\\n69\\n70\\n71\\n72\", \"\\n73\\n74\\n75\\n76\", \"\\n77\\n78\\n79\\n80\", \"\\n81\\n82\\n83\\n84\", \"\\n85\\n86\\n87\\n88\", \"\\n89\\n90\\n91\\n92\", \"\\n93\\n94\\n95\\n96\", \"\\n97\\n98\\n99\\n100\", \"\\n101\\n102\\n103\\n104\", \"\\n105\\n106\\n107\\n108\", \"\\n109\\n110\\n111\\n112\", \"\\n113\\n114\\n115\\n116\", \"\\n117\\n118\\n119\\n120\", \"\\n121\\n122\\n123\\n124\", \"\\n125\\n126\\n127\\n128\", \"\\n129\\n130\\n131\\n132\", \"\\n133\\n134\\n135\\n136\", \"\\n137\\n138\\n139\\n140\", \"\\n141\\n142\\n143\\n144\", \"\\n145\\n146\\n147\\n148\", \"\\n149\\n150\\n151\\n152\", \"\\n153\\n154\\n155\\n156\", \"\\n157\\n158\\n159\\n160\", \"\\n161\\n162\\n163\\n164\", \"\\n165\\n166\\n167\\n168\", \"\\n169\\n170\\n171\\n172\", \"\\n173\\n174\\n175\\n176\", \"\\n177\\n178\\n179\\n180\", \"\\n181\\n182\\n183\\n184\", \"\\n185\\n186\\n187\\n188\", \"\\n189\\n190\\n191\\n192\", \"\\n193\\n194\\n195\\n196\", \"\\n197\\n198\\n199\\n200\"]",
            "assistant: text \"1\\n2\\n3\\n4\\n5\\n6\\n7\\n8\\n9\\n10\\n11\\n12\\n13\\n14\\n15\\n16\\n17\\n18\\n19\\n20\\n21\\n22\\n23\\n24\\n25\\n26\\n27\\n28\\n29\\n30\\n31\\n32\\n33\\n34\\n35\\n36\\n37\\n38\\n39\\n40\\n41\\n42\\n43\\n44\\n45\\n46\\n47\\n48\\n49\\n50\\n51\\n52\\n53\\n54\\n55\\n56\\n57\\n58\\n59\\n60\\n61\\n62\\n63\\n64\\n65\\n66\\n67\\n68\\n69\\n70\\n71\\n72\\n73\\n74\\n75\\n76\\n77\\n78\\n79\\n80\\n81\\n82\\n83\\n84\\n85\\n86\\n87\\n88\\n89\\n90\\n91\\n92\\n93\\n94\\n95\\n96\\n97\\n98\\n99\\n100\\n101\\n102\\n103\\n104\\n105\\n106\\n107\\n108\\n109\\n110\\n111\\n112\\n113\\n114\\n115\\n116\\n117\\n118\\n119\\n120\\n121\\n122\\n123\\n124\\n125\\n126\\n127\\n128\\n129\\n130\\n131\\n132\\n133\\n134\\n135\\n136\\n137\\n138\\n139\\n140\\n141\\n142\\n143\\n144\\n145\\n146\\n147\\n148\\n149\\n150\\n151\\n152\\n153\\n154\\n155\\n156\\n157\\n158\\n159\\n160\\n161\\n162\\n163\\n164\\n165\\n166\\n167\\n168\\n169\\n170\\n171\\n172\\n173\\n174\\n175\\n176\\n177\\n178\\n179\\n180\\n181\\n182\\n183\\n184\\n185\\n186\\n187\\n188\\n189\\n190\\n191\\n192\\n193\\n194\\n195\\n196\\n197\\n198\\n199\\n200\"",
            "finished success is_error=false Some(\"completed\")",
            "turn_started 03ef5463-f8da-4412-a8bd-0371a11cce56",
            "streams [\"pong\"]",
            "assistant: text \"pong\"",
            "finished success is_error=false Some(\"completed\")",
            // 3b: interrupt with a permission request open; the CLI withdraws it.
            "turn_started 03ef5463-f8da-4412-a8bd-0371a11cce56",
            "assistant: tool_use Write Path(\"/tmp/s1-scratch/d.txt\")",
            "permission Write Path(\"/tmp/s1-scratch/d.txt\")",
            "permission_cancelled",
            "user: tool_result error=true",
            "user: text \"[Request interrupted by user for tool use]\"",
            "finished error_during_execution is_error=true Some(\"aborted_tools\")",
        ]
    );
    assert_eq!(
        replayed.unknown,
        BTreeMap::from([
            ("stream_event/-".to_owned(), 81),
            ("system/status".to_owned(), 11),
        ])
    );
    // a.txt and b.txt were answered above and d.txt was withdrawn by the CLI,
    // so none of them can be answered now.
    assert_eq!(
        replayed.second,
        [
            "no open permission request with id ef41e4d8-de15-458b-88d0-f46aa4286c9a",
            "no open permission request with id 0a710d2a-6061-4dba-96f1-3dc74607c276",
            "no open permission request with id 8e8d863f-1d3f-4910-8591-5d1453ca9b6a",
        ]
    );
}

#[tokio::test]
async fn resume_and_accept_edits_recording() {
    // acceptEdits must not ask, so no permission request may reach `second`.
    let replayed = replay("session2-resume-accept-edits.jsonl", Vec::new()).await;
    assert_eq!(
        replayed.events,
        [
            "turn_started 03ef5463-f8da-4412-a8bd-0371a11cce56",
            "streams [\"a.txt\"]",
            "assistant: text \"a.txt\"",
            "finished success is_error=false Some(\"completed\")",
            "turn_started 03ef5463-f8da-4412-a8bd-0371a11cce56",
            "assistant: tool_use Write Path(\"/tmp/s1-scratch/c.txt\")",
            "user: tool_result error=false",
            "streams [\"I created `c.txt`\", \" in `/tmp/s\", \"1-scratch` with the\", \" content `accepted`.\"]",
            "assistant: text \"I created `c.txt` in `/tmp/s1-scratch` with the content `accepted`.\"",
            "finished success is_error=false Some(\"completed\")",
        ]
    );
    assert_eq!(
        replayed.unknown,
        BTreeMap::from([
            ("stream_event/-".to_owned(), 25),
            ("system/status".to_owned(), 4),
        ])
    );
    assert_eq!(replayed.second, Vec::<String>::new());
}

/// Writes `lines` to a fresh recording and returns its path.
fn recording(name: &str, lines: Vec<&str>) -> PathBuf {
    let path = test_dir(name).join("recording.jsonl");
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
    path
}

/// A real `result` line, so the test knows when the actor has handled every
/// line before it.
fn recorded_result() -> String {
    std::fs::read_to_string(fixture("session2-resume-accept-edits.jsonl"))
        .unwrap()
        .lines()
        .find_map(|line| match frame_type(line).as_deref() {
            Some("result") => Some(line.to_owned()),
            _ => None,
        })
        .unwrap()
}

const INITIALIZED: &str = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_1","response":{"claude_code_version":"2.1.316"}}}"#;

#[tokio::test]
async fn unknown_control_request_gets_an_error_reply() {
    let stdout = recording(
        "unknown-control",
        Vec::from([
            INITIALIZED,
            r#"{"type":"control_request","request_id":"cli_3","request":{"subtype":"elicit_frobnicate","q":1}}"#,
            r#"{"type":"brand_new_frame","x":1}"#,
            &recorded_result(),
        ]),
    );
    let claude = fake_claude("unknown-control", &stdout);
    let (session, events) = ClaudeSession::spawn(options(&claude)).await.unwrap();
    let played = Replay::new(session, events, Vec::new())
        .until(Until::Turns(1))
        .await;
    played.session.shutdown().await.unwrap();
    assert_eq!(
        played.seen,
        ["finished success is_error=false Some(\"completed\")"]
    );
    assert_eq!(
        played.unknown,
        BTreeMap::from([("brand_new_frame/-".to_owned(), 1)])
    );
    let written = std::fs::read_to_string(claude.stdin_log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .collect::<Vec<Value>>();
    assert_eq!(
        written,
        [
            serde_json::from_str::<Value>(
                r#"{"type": "control_request", "request_id": "req_1", "request": {"subtype": "initialize"}}"#
            )
            .unwrap(),
            serde_json::from_str::<Value>(
                r#"{"type": "control_response", "response": {"subtype": "error", "request_id": "cli_3",
                "error": "unsupported control request subtype: elicit_frobnicate"}}"#
            )
            .unwrap(),
        ]
    );
}

const FUTURE_CLI: &str = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_1","response":{"claude_code_version":"2.2.0"}}}"#;

#[tokio::test]
async fn unsupported_cli_version_is_refused() {
    let stdout = recording("future-cli", Vec::from([FUTURE_CLI]));
    let claude = fake_claude("future-cli", &stdout);
    let refused = ClaudeSession::spawn(options(&claude))
        .await
        .err()
        .map(|e| e.to_string());
    assert_eq!(
        refused.as_deref(),
        Some(
            "claude 2.2.0 is not supported (supported: 2.1.316 to 2.1.322); set VersionPolicy::AllowUntested to run it anyway"
        )
    );
}

const VERSIONLESS_CLI: &str = r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_1","response":{}}}"#;

#[tokio::test]
async fn cli_that_reports_no_version_is_refused() {
    let stdout = recording("versionless-cli", Vec::from([VERSIONLESS_CLI]));
    let claude = fake_claude("versionless-cli", &stdout);
    let refused = ClaudeSession::spawn(options(&claude))
        .await
        .err()
        .map(|e| e.to_string());
    assert_eq!(
        refused.as_deref(),
        Some(
            "claude did not report its version, so it can't be checked; set VersionPolicy::AllowUntested to run it anyway"
        )
    );
}

#[tokio::test]
async fn allow_untested_version_runs_it_anyway() {
    let stdout = recording("future-cli-allowed", Vec::from([FUTURE_CLI]));
    let claude = fake_claude("future-cli-allowed", &stdout);
    let opts = ClaudeOptions {
        versions: VersionPolicy::AllowUntested,
        ..options(&claude)
    };
    let (session, _events) = ClaudeSession::spawn(opts).await.unwrap();
    assert_eq!(
        session.shutdown().await.unwrap().describe(),
        "exit status: 0"
    );
}

/// How long a gated stand-in must stay silent to count as waiting.
const QUIET: Duration = Duration::from_millis(300);

/// An event that arrives within `QUIET`, if any.
async fn early(events: &mut mpsc::Receiver<ClaudeEvent>) -> Option<String> {
    tokio::time::timeout(QUIET, events.recv())
        .await
        .ok()
        .flatten()
        .map(|event| summary(&event))
}

/// A session on an e2e recording, before its first user message.
async fn gated(name: &str) -> Replay {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/e2e")
        .join(name);
    let claude = fake_claude(name, &path);
    let (session, events) = ClaudeSession::spawn(options(&claude)).await.unwrap();
    Replay::new(session, events, Vec::new())
}

#[tokio::test]
async fn approve_recording_waits_for_the_message_and_the_answer() {
    let mut r = gated("claude-approve.jsonl").await;
    assert_eq!(early(&mut r.events).await, None);

    r.session.send_user(String::from("a.txt")).await.unwrap();
    let mut r = r.until(Until::Permission).await;
    assert_eq!(early(&mut r.events).await, None);

    r.session
        .respond(
            PermissionRequestId(String::from("ef41e4d8-de15-458b-88d0-f46aa4286c9a")),
            PermissionDecision::Allow,
        )
        .await
        .unwrap();
    let r = r.until(Until::Turns(1)).await;
    assert_eq!(
        r.seen,
        [
            "turn_started 03ef5463-f8da-4412-a8bd-0371a11cce56",
            "assistant: tool_use Write Path(\"/tmp/s1-scratch/a.txt\")",
            "permission Write Path(\"/tmp/s1-scratch/a.txt\")",
            "user: tool_result error=false",
            "streams [\"I\", \" created `a.txt` in\", \" `/tmp/s1\", \"-scratch` with the content\", \" `hello from s1`.\"]",
            "assistant: text \"I created `a.txt` in `/tmp/s1-scratch` with the content `hello from s1`.\"",
            "finished success is_error=false Some(\"completed\")",
        ]
    );
    assert_eq!(
        r.unknown,
        BTreeMap::from([
            ("stream_event/-".to_owned(), 20),
            ("system/status".to_owned(), 2),
        ])
    );
    assert_eq!(
        r.session.shutdown().await.unwrap().describe(),
        "exit status: 0"
    );
}

#[tokio::test]
async fn deny_interrupt_recording_waits_for_each_answer_and_the_interrupt() {
    let mut r = gated("claude-deny-interrupt.jsonl").await;
    assert_eq!(early(&mut r.events).await, None);

    r.session.send_user(String::from("b.txt")).await.unwrap();
    let mut r = r.until(Until::Permission).await;
    assert_eq!(early(&mut r.events).await, None);

    let deny = PermissionDecision::Deny {
        message: String::from("The user does not want b.txt."),
        then: AfterDeny::Continue,
    };
    r.session
        .respond(
            PermissionRequestId(String::from("0a710d2a-6061-4dba-96f1-3dc74607c276")),
            deny,
        )
        .await
        .unwrap();
    let mut r = r.until(Until::Turns(1)).await;
    assert_eq!(early(&mut r.events).await, None);

    // The ack waits for our interrupt; had it come first, `interrupt` would
    // time out on an ack it never matched.
    r.session.send_user(String::from("count")).await.unwrap();
    let r = r.until(Until::Delta).await;
    r.session.interrupt().await.unwrap();
    let mut r = r.until(Until::Turns(2)).await;
    assert_eq!(early(&mut r.events).await, None);

    r.session.send_user(String::from("pong")).await.unwrap();
    let r = r.until(Until::Turns(3)).await;
    assert_eq!(
        r.seen,
        [
            "turn_started 03ef5463-f8da-4412-a8bd-0371a11cce56",
            "assistant: tool_use Write Path(\"/tmp/s1-scratch/b.txt\")",
            "permission Write Path(\"/tmp/s1-scratch/b.txt\")",
            "user: tool_result error=true",
            "streams [\"The write to\", \" `\", \"b.txt` was denied\", \" \", \"(\\\"The user does not want b.\", \"txt.\\\"\", \"), so I did not retry and\", \" no file was created.\"]",
            "assistant: text \"The write to `b.txt` was denied (\\\"The user does not want b.txt.\\\"), so I did not retry and no file was created.\"",
            "finished success is_error=false Some(\"completed\")",
            "turn_started 03ef5463-f8da-4412-a8bd-0371a11cce56",
            "streams [\"1\"]",
            "assistant: text \"1\"",
            "user: text \"[Request interrupted by user]\"",
            "finished error_during_execution is_error=true Some(\"aborted_streaming\")",
            "turn_started 03ef5463-f8da-4412-a8bd-0371a11cce56",
            "streams [\"pong\"]",
            "assistant: text \"pong\"",
            "finished success is_error=false Some(\"completed\")",
        ]
    );
    assert_eq!(
        r.unknown,
        BTreeMap::from([
            ("stream_event/-".to_owned(), 31),
            ("system/status".to_owned(), 4),
        ])
    );
    assert_eq!(
        r.session.shutdown().await.unwrap().describe(),
        "exit status: 0"
    );
}
