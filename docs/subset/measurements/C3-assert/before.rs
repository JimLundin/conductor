#[tokio::test]
async fn unsupported_cli_version_is_refused() {
    let stdout = recording("future-cli", &[FUTURE_CLI]);
    let (claude, _) = fake_claude("future-cli", &stdout);
    match ClaudeSession::spawn(options(claude)).await {
        Err(Error::UnsupportedVersion { found, supported }) => {
            assert_eq!(
                (found.as_str(), supported.as_str()),
                ("2.2.0", "2.1.316 to 2.1.322")
            );
        }
        Err(e) => panic!("wrong error: {e}"),
        Ok(_) => panic!("2.2.0 was accepted"),
    }
}

#[test]
fn oversized_header_is_refused_before_reading_the_body() {
    assert!(matches!(
        body_len([0x01, 0x00, 0x00, 0x01]),
        Err(FrameError::TooLarge(16_777_217))
    ));
}
