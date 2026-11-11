    #[test]
    fn replies_and_events_wire_shapes() {
        assert_eq!(
            encode(&HelloReply::Welcome(Welcome {
                host: String::from("desk"),
                epoch: Epoch(1),
                head_seq: HostSeq(0),
            }))
            .unwrap(),
            frame(r#"{"reply":"welcome","host":"desk","epoch":1,"head_seq":0}"#)
        );
        assert_eq!(
            encode(&Event {
                seq: HostSeq(3),
                kind: EventKind::ApprovalResolved {
                    id: RequestId(2),
                    decision: Decision::Approve,
                    by: String::from("phone"),
                },
            })
            .unwrap(),
            frame(
                r#"{"seq":3,"kind":{"type":"approval_resolved","id":2,"decision":"approve","by":"phone"}}"#
            )
        );
        assert_eq!(
            encode(&Event {
                seq: HostSeq(4),
                kind: EventKind::TurnFinished,
            })
            .unwrap(),
            frame(r#"{"seq":4,"kind":{"type":"turn_finished"}}"#)
        );
        assert_eq!(
            encode(&CommandResult::Rejected(Rejection::AlreadyResolved {
                by: String::from("desk"),
            }))
            .unwrap(),
            frame(r#"{"result":"rejected","reason":"already_resolved","by":"desk"}"#)
        );
        assert_eq!(
            encode(&CommandResult::Accepted).unwrap(),
            frame(r#"{"result":"accepted"}"#)
        );
    }
