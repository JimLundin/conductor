//! Replays recorded `claude` stdout (fixtures/claude) through a real
//! `ClaudeSession`. A stand-in process prints the recording and then swallows
//! stdin, so the actor, the boundary mapping and the exactly-once permission
//! bookkeeping all run against the CLI's real bytes.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use agents::claude::{
    Block, ClaudeEvent, ClaudeOptions, ClaudeSession, PermissionDecision, PermissionMode,
    PermissionRequestId, Role, SettingSource,
};
use serde_json::Value;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/claude")
        .join(name)
}

fn fake_claude(fixture: &Path) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agents-replay-{}-{}",
        std::process::id(),
        fixture.file_stem().unwrap().to_str().unwrap()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("claude");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ncat '{}'\ncat >/dev/null\n", fixture.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

fn summary(event: &ClaudeEvent) -> String {
    match event {
        ClaudeEvent::SessionStarted { session_id } => format!("session_started {}", session_id.0),
        ClaudeEvent::Message { role, content } => {
            let blocks: Vec<String> = content
                .iter()
                .map(|b| match b {
                    Block::Text(t) => format!("text {t:?}"),
                    Block::Thinking(_) => "thinking".into(),
                    Block::ToolUse { name, .. } => format!("tool_use {name}"),
                    Block::ToolResult { is_error, .. } => format!("tool_result error={is_error}"),
                    Block::Other(_) => "other".into(),
                })
                .collect();
            let role = match role {
                Role::Assistant => "assistant",
                Role::User => "user",
            };
            format!("{role}: {}", blocks.join(", "))
        }
        ClaudeEvent::PermissionRequested {
            tool_name, input, ..
        } => format!("permission {tool_name} {}", input["file_path"]),
        ClaudeEvent::PermissionCancelled { .. } => "permission_cancelled".into(),
        ClaudeEvent::TurnFinished {
            subtype,
            is_error,
            terminal_reason,
            ..
        } => format!("finished {subtype} is_error={is_error} {terminal_reason:?}"),
        ClaudeEvent::Exited { status } => format!("exited {status}"),
        ClaudeEvent::Unknown(v) => format!(
            "unknown {}/{}",
            v["type"].as_str().unwrap_or("?"),
            v["subtype"].as_str().unwrap_or("-")
        ),
    }
}

/// Replays a fixture, answering each permission request as `decide` says
/// (`None` leaves it open, as the live run did before interrupting), and
/// returns the event summaries plus the result of answering each id again
/// once the recording has played out.
async fn replay(
    name: &str,
    mut decide: impl FnMut(&str) -> Option<PermissionDecision>,
) -> (Vec<String>, Vec<String>) {
    let path = fixture(name);
    let (session, mut events) = ClaudeSession::spawn(ClaudeOptions {
        claude_path: fake_claude(&path),
        cwd: std::env::temp_dir(),
        model: None,
        permission_mode: PermissionMode::Default,
        resume: None,
        setting_sources: Some(vec![SettingSource::Project]),
        stdout_log: None,
    })
    .await
    .expect("initialize is answered by the recording's first line");

    let lines = std::fs::read_to_string(&path).unwrap().lines().count();
    let results = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter(|l| serde_json::from_str::<Value>(l).unwrap()["type"] == "result")
        .count();

    let mut seen = Vec::new();
    let mut ids: Vec<PermissionRequestId> = Vec::new();
    let mut finished = 0;
    while finished < results {
        let event = events.recv().await.expect("event stream ended early");
        if let ClaudeEvent::PermissionRequested { id, tool_name, .. } = &event {
            if let Some(decision) = decide(tool_name) {
                session.respond(id.clone(), decision).await.unwrap();
            }
            ids.push(id.clone());
        }
        if matches!(event, ClaudeEvent::TurnFinished { .. }) {
            finished += 1;
        }
        seen.push(summary(&event));
    }
    assert!(seen.len() <= lines);

    let mut second = Vec::new();
    for id in ids {
        second.push(match session.respond(id, PermissionDecision::Allow).await {
            Ok(()) => "ok".into(),
            Err(e) => e.to_string(),
        });
    }
    let status = session.shutdown().await.unwrap();
    assert!(status.success());
    (seen, second)
}

#[tokio::test]
async fn approve_deny_interrupt_recording() {
    let mut n = 0;
    let (events, second) = replay("session1-approve-deny-interrupt.jsonl", |_| {
        n += 1;
        match n {
            1 => Some(PermissionDecision::Allow),
            2 => Some(PermissionDecision::Deny {
                message: "no".into(),
                interrupt: false,
            }),
            _ => None,
        }
    })
    .await;
    assert_eq!(
        events,
        [
            // 1: approve a Write.
            "session_started 5f66391b-ba0e-46b0-b334-94fd247e6edb",
            "assistant: tool_use Write",
            "permission Write \"/tmp/s1-scratch/a.txt\"",
            "user: tool_result error=false",
            "assistant: text \"I created `a.txt` in `/tmp/s1-scratch` with the content `hello from s1`.\"",
            "finished success is_error=false Some(\"completed\")",
            // 2: deny a Write.
            "assistant: tool_use Write",
            "permission Write \"/tmp/s1-scratch/b.txt\"",
            "user: tool_result error=true",
            "assistant: text \"The Write to `b.txt` was denied (\\\"The user does not want b.txt.\\\"), so I didn't retry and no file was created.\"",
            "finished success is_error=false Some(\"completed\")",
            // 3: interrupt right after the first assistant output.
            "assistant: thinking",
            "assistant: text \"starting\"",
            "user: text \"[Request interrupted by user]\"",
            "finished error_during_execution is_error=true Some(\"aborted_streaming\")",
            "assistant: text \"pong\"",
            "finished success is_error=false Some(\"completed\")",
            // 3b: interrupt with a permission request open; the CLI withdraws it.
            "assistant: tool_use Write",
            "permission Write \"/tmp/s1-scratch/d.txt\"",
            "permission_cancelled",
            "user: tool_result error=true",
            "user: text \"[Request interrupted by user for tool use]\"",
            "finished error_during_execution is_error=true Some(\"aborted_tools\")",
        ]
    );
    // a.txt and b.txt were answered above and d.txt was withdrawn by the CLI,
    // so none of them can be answered now.
    assert_eq!(
        second,
        [
            "no open permission request with id aa325d90-e32d-4081-a2c3-2da64e7eda39",
            "no open permission request with id 53dea1ad-8c6b-41e5-bcfb-0dd51b0cc027",
            "no open permission request with id f7c32034-7592-4119-aab5-cde3fe5b2730",
        ]
    );
}

#[tokio::test]
async fn resume_and_accept_edits_recording() {
    let (events, second) = replay("session2-resume-accept-edits.jsonl", |_| {
        panic!("acceptEdits must not ask")
    })
    .await;
    assert_eq!(
        events,
        [
            "session_started 5f66391b-ba0e-46b0-b334-94fd247e6edb",
            "assistant: text \"a.txt\"",
            "finished success is_error=false Some(\"completed\")",
            "unknown system/status",
            "assistant: tool_use Write",
            "user: tool_result error=false",
            "assistant: text \"I created `c.txt` in `/tmp/s1-scratch` with the content `accepted`.\"",
            "finished success is_error=false Some(\"completed\")",
        ]
    );
    assert!(second.is_empty());
}
