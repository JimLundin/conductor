/// The next event, failing the test if none arrives in time.
async fn next(events: &mut mpsc::Receiver<ClaudeEvent>) -> String {
    let event = tokio::time::timeout(Duration::from_secs(20), events.recv())
        .await
        .expect("an event arrives")
        .expect("the event stream is open");
    summary(&event)
}

/// An event that arrives within `QUIET`, if any.
async fn early(events: &mut mpsc::Receiver<ClaudeEvent>) -> Option<String> {
    tokio::time::timeout(QUIET, events.recv())
        .await
        .ok()
        .flatten()
        .map(|event| summary(&event))
}

#[tokio::test]
async fn gated_recording_waits_for_the_message_and_the_answer() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/e2e/claude-approve.jsonl");
    let claude = fake_claude("gated-approve", &path);
    let (session, mut events) = ClaudeSession::spawn(options(&claude)).await.unwrap();
    assert_eq!(early(&mut events).await, None);

    session
        .send_user(String::from("Create a.txt"))
        .await
        .unwrap();
    let asked = Vec::from([
        next(&mut events).await,
        next(&mut events).await,
        next(&mut events).await,
    ]);
    assert_eq!(
        asked,
        [
            "session_started 5f66391b-ba0e-46b0-b334-94fd247e6edb",
            "assistant: tool_use Write",
            "permission Write \"/tmp/s1-scratch/a.txt\"",
        ]
    );
    assert_eq!(early(&mut events).await, None);

    session
        .respond(
            PermissionRequestId(String::from("aa325d90-e32d-4081-a2c3-2da64e7eda39")),
            PermissionDecision::Allow,
        )
        .await
        .unwrap();
    let answered = Vec::from([
        next(&mut events).await,
        next(&mut events).await,
        next(&mut events).await,
    ]);
    assert_eq!(
        answered,
        [
            "user: tool_result error=false",
            "assistant: text \"I created `a.txt` in `/tmp/s1-scratch` with the content `hello from s1`.\"",
            "finished success is_error=false Some(\"completed\")",
        ]
    );
    assert_eq!(
        session.shutdown().await.unwrap().describe(),
        "exit status: 0"
    );
}
