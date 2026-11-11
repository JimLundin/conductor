    #[test]
    fn replies_and_events_wire_shapes() {
        Vec::from([
            (
                encode(&HelloReply::Welcome(Welcome {
                    host: String::from("desk"),
                    epoch: Epoch(1),
                    head_seq: HostSeq(0),
                })),
                r#"{"reply":"welcome","host":"desk","epoch":1,"head_seq":0}"#,
            ),
            (
                encode(&Event {
                    seq: HostSeq(3),
                    kind: EventKind::ApprovalResolved {
                        id: RequestId(2),
                        decision: Decision::Approve,
                        by: String::from("phone"),
                    },
                }),
                r#"{"seq":3,"kind":{"type":"approval_resolved","id":2,"decision":"approve","by":"phone"}}"#,
            ),
            (
                encode(&Event {
                    seq: HostSeq(4),
                    kind: EventKind::TurnFinished,
                }),
                r#"{"seq":4,"kind":{"type":"turn_finished"}}"#,
            ),
            (
                encode(&CommandResult::Rejected(Rejection::AlreadyResolved {
                    by: String::from("desk"),
                })),
                r#"{"result":"rejected","reason":"already_resolved","by":"desk"}"#,
            ),
            (encode(&CommandResult::Accepted), r#"{"result":"accepted"}"#),
        ])
        .into_iter()
        .for_each(|(bytes, body)| assert_eq!(bytes.unwrap(), frame(body), "{body}"));
    }
