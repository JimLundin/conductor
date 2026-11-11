mod tests {
    /// A control request the actor can't serve but must still answer by id.
    struct Unanswerable {
        why: &'static str,
        line: &'static str,
        request_id: &'static str,
        subtype: &'static str,
    }

    const UNANSWERABLE: &[Unanswerable] = &[
        Unanswerable {
            why: "an unknown subtype still yields its request id",
            line: r#"{"type":"control_request","request_id":"cli_3","request":{"subtype":"elicit_frobnicate","q":1}}"#,
            request_id: "cli_3",
            subtype: "elicit_frobnicate",
        },
        Unanswerable {
            why: "a malformed can_use_tool is still answered",
            line: r#"{"type":"control_request","request_id":"cli_5","request":{"subtype":"can_use_tool"}}"#,
            request_id: "cli_5",
            subtype: "can_use_tool",
        },
        Unanswerable {
            why: "a hook callback is unsupported, not unknown",
            line: r#"{"type":"control_request","request_id":"cli_4","request":{"subtype":"hook_callback","callback_id":"hook_0","input":{}}}"#,
            request_id: "cli_4",
            subtype: "hook_callback",
        },
    ];

    #[test]
    fn unanswerable_control_requests_keep_their_id() {
        UNANSWERABLE.iter().for_each(|case| {
            assert_eq!(
                parse_line(case.line).unwrap(),
                Inbound::UnsupportedControlRequest {
                    request_id: String::from(case.request_id),
                    subtype: String::from(case.subtype),
                },
                "{}",
                case.why
            );
        });
    }
}
