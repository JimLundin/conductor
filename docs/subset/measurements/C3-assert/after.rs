#[tokio::test]
async fn unsupported_cli_version_is_refused() {
    let stdout = recording("future-cli", &[FUTURE_CLI]);
    let (claude, _) = fake_claude("future-cli", &stdout);
    let refused = ClaudeSession::spawn(options(claude)).await.err().map(|e| e.to_string());
    assert_eq!(
        refused.as_deref(),
        Some("claude 2.2.0 is not supported (supported: 2.1.316 to 2.1.322); set allow_untested_version to run it anyway")
    );
}

#[test]
fn oversized_header_is_refused_before_reading_the_body() {
    assert_eq!(
        body_len([0x01, 0x00, 0x00, 0x01]).err().map(|e| e.to_string()),
        Some(String::from("frame body of 16777217 bytes is too large"))
    );
}
