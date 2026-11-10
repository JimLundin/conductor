//! A frame is a `u32` big-endian body length followed by a JSON body (D8).
//! Callers own the I/O: read [`HEADER_LEN`] bytes, pass them to [`body_len`],
//! read that many bytes, pass them to [`decode`].

use serde::Serialize;
use serde::de::DeserializeOwned;

pub const HEADER_LEN: usize = 4;

/// Larger frames are refused before their body is read.
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame of {0} bytes exceeds the {MAX_FRAME_LEN}-byte limit")]
    TooLarge(u64),
    #[error("malformed frame body: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(msg)?;
    let len = u32::try_from(body.len())
        .ok()
        .filter(|len| *len <= MAX_FRAME_LEN)
        .ok_or(FrameError::TooLarge(body.len() as u64))?;
    let mut frame = Vec::with_capacity(HEADER_LEN + body.len());
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

pub fn body_len(header: [u8; HEADER_LEN]) -> Result<usize, FrameError> {
    let len = u32::from_be_bytes(header);
    if len > MAX_FRAME_LEN {
        return Err(FrameError::TooLarge(len.into()));
    }
    Ok(len as usize)
}

pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, FrameError> {
    Ok(serde_json::from_slice(body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AuthToken, Command, CommandBody, CommandId, CommandResult, Decision, Epoch, Event,
        EventKind, Hello, HelloReply, HostSeq, Open, Rejection, RequestId, Subscribe, Welcome,
    };

    fn frame(body: &str) -> Vec<u8> {
        let mut bytes = (body.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(body.as_bytes());
        bytes
    }

    #[test]
    fn prompt_command_frame_is_length_then_json() {
        let open = Open::Command(Command {
            id: CommandId(7),
            body: CommandBody::Prompt { text: "hi".into() },
        });
        let bytes = encode(&open).unwrap();
        assert_eq!(
            bytes,
            b"\x00\x00\x00\x3e{\"open\":\"command\",\"id\":7,\"body\":{\"type\":\"prompt\",\"text\":\"hi\"}}"
        );
        assert_eq!(body_len(bytes[..4].try_into().unwrap()).unwrap(), 62);
        assert_eq!(decode::<Open>(&bytes[4..]).unwrap(), open);
    }

    #[test]
    fn hello_and_subscribe_wire_shapes() {
        let token = AuthToken::from_bytes([0xab; 16]);
        assert_eq!(
            encode(&Open::Hello(Hello {
                client: "phone".into(),
                token
            }))
            .unwrap(),
            frame(
                r#"{"open":"hello","client":"phone","token":"abababababababababababababababab"}"#
            )
        );
        assert_eq!(
            encode(&Open::Subscribe(Subscribe {
                epoch: Epoch(9),
                after_seq: HostSeq(41)
            }))
            .unwrap(),
            frame(r#"{"open":"subscribe","epoch":9,"after_seq":41}"#)
        );
    }

    #[test]
    fn replies_and_events_wire_shapes() {
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (
                encode(&HelloReply::Welcome(Welcome {
                    host: "desk".into(),
                    epoch: Epoch(1),
                    head_seq: HostSeq(0),
                }))
                .unwrap(),
                r#"{"reply":"welcome","host":"desk","epoch":1,"head_seq":0}"#,
            ),
            (
                encode(&Event {
                    seq: HostSeq(3),
                    kind: EventKind::ApprovalResolved {
                        id: RequestId(2),
                        decision: Decision::Approve,
                        by: "phone".into(),
                    },
                })
                .unwrap(),
                r#"{"seq":3,"kind":{"type":"approval_resolved","id":2,"decision":"approve","by":"phone"}}"#,
            ),
            (
                encode(&Event {
                    seq: HostSeq(4),
                    kind: EventKind::TurnFinished,
                })
                .unwrap(),
                r#"{"seq":4,"kind":{"type":"turn_finished"}}"#,
            ),
            (
                encode(&CommandResult::Rejected(Rejection::AlreadyResolved {
                    by: "desk".into(),
                }))
                .unwrap(),
                r#"{"result":"rejected","reason":"already_resolved","by":"desk"}"#,
            ),
            (
                encode(&CommandResult::Accepted).unwrap(),
                r#"{"result":"accepted"}"#,
            ),
        ];
        for (bytes, body) in cases {
            assert_eq!(bytes, frame(body), "{body}");
        }
    }

    #[test]
    fn oversized_header_is_refused_before_reading_the_body() {
        assert!(matches!(
            body_len([0x01, 0x00, 0x00, 0x01]),
            Err(FrameError::TooLarge(16_777_217))
        ));
        assert_eq!(body_len([0x01, 0x00, 0x00, 0x00]).unwrap(), 16_777_216);
    }

    #[test]
    fn unknown_message_type_is_a_json_error() {
        assert!(matches!(
            decode::<Open>(br#"{"open":"teleport"}"#),
            Err(FrameError::Json(_))
        ));
    }
}
