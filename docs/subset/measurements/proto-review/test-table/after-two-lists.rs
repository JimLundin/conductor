    #[test]
    fn replies_and_events_wire_shapes() {
        assert_eq!(
            [
                encode(&HelloReply::Welcome(Welcome {
                    host: String::from("desk"),
                    epoch: Epoch(1),
                    head_seq: HostSeq(0),
                })),
                encode(&Event {
                    seq: HostSeq(3),
                    kind: EventKind::ApprovalResolved {
                        id: RequestId(2),
                        decision: Decision::Approve,
                        by: String::from("phone"),
                    },
                }),
                encode(&Event {
                    seq: HostSeq(4),
                    kind: EventKind::TurnFinished,
                }),
                encode(&CommandResult::Rejected(Rejection::AlreadyResolved {
                    by: String::from("desk"),
                })),
                encode(&CommandResult::Accepted),
            ]
            .map(Result::unwrap),
            [
                r#"{"reply":"welcome","host":"desk","epoch":1,"head_seq":0}"#,
                r#"{"seq":3,"kind":{"type":"approval_resolved","id":2,"decision":"approve","by":"phone"}}"#,
                r#"{"seq":4,"kind":{"type":"turn_finished"}}"#,
                r#"{"result":"rejected","reason":"already_resolved","by":"desk"}"#,
                r#"{"result":"accepted"}"#,
            ]
            .map(frame)
        );
    }
