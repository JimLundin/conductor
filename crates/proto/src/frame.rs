//! A frame is a `u32` big-endian body length followed by a JSON body (D8).
//! Callers own the I/O: read [`HEADER_LEN`] bytes, pass them to [`body_len`],
//! read that many bytes, pass them to [`decode`].
//!
//! A WebRTC data channel delivers messages, not a byte stream, so a sender
//! splits each encoded frame into messages of at most [`CHUNK_LEN`] bytes and
//! the receiver feeds them to a [`FrameBuffer`].

use serde::Serialize;
use serde::de::DeserializeOwned;

pub const HEADER_LEN: usize = 4;

/// Larger frames are refused before their body is read.
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

/// The largest data channel message we send. Every browser accepts 16 KiB
/// messages (RFC 8831 §6.6), and the length prefix makes boundaries
/// irrelevant to the receiver.
pub const CHUNK_LEN: usize = 16 * 1024;

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

/// Reassembles frames from data channel messages, which split and join
/// frames arbitrarily.
#[derive(Debug, Default)]
pub struct FrameBuffer {
    bytes: Vec<u8>,
}

impl FrameBuffer {
    pub fn push(&mut self, message: &[u8]) {
        self.bytes.extend_from_slice(message);
    }

    /// The next complete frame's body, if all of it has arrived.
    pub fn next_body(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        let Some(header) = self.bytes.first_chunk::<HEADER_LEN>() else {
            return Ok(None);
        };
        let len = body_len(*header)?;
        if self.bytes.len() < HEADER_LEN + len {
            return Ok(None);
        }
        let body = self.bytes[HEADER_LEN..HEADER_LEN + len].to_vec();
        self.bytes.drain(..HEADER_LEN + len);
        Ok(Some(body))
    }

    /// True between frames: the channel can end here without losing data.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AuthToken, CandidateType, Command, CommandBody, CommandId, CommandResult, Decision, Epoch,
        Event, EventKind, Hello, HelloReply, HostSeq, Open, Rejection, RequestId, RtcOffer, Signal,
        Subscribe, Welcome,
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
    fn frame_buffer_reassembles_frames_split_and_joined_across_messages() {
        let first = encode(&Event {
            seq: HostSeq(1),
            kind: EventKind::TurnFinished,
        })
        .unwrap();
        let second = encode(&CommandResult::Accepted).unwrap();
        let stream = [first, second].concat();
        let mut buffer = FrameBuffer::default();
        let mut bodies = Vec::new();
        for message in stream.chunks(5) {
            buffer.push(message);
            while let Some(body) = buffer.next_body().unwrap() {
                bodies.push(String::from_utf8(body).unwrap());
            }
        }
        assert_eq!(
            bodies,
            [
                r#"{"seq":1,"kind":{"type":"turn_finished"}}"#,
                r#"{"result":"accepted"}"#
            ]
        );
        assert!(buffer.is_empty());
        buffer.push(&[0, 0]);
        assert_eq!(buffer.next_body().unwrap(), None);
        assert!(!buffer.is_empty());
    }

    #[test]
    fn rtc_signal_wire_shapes() {
        assert_eq!(
            encode(&Open::Rtc(RtcOffer { sdp: "v=0".into() })).unwrap(),
            frame(r#"{"open":"rtc","sdp":"v=0"}"#)
        );
        assert_eq!(
            encode(&Signal::Selected {
                local: CandidateType::Srflx,
                remote: CandidateType::Prflx,
            })
            .unwrap(),
            frame(r#"{"signal":"selected","local":"srflx","remote":"prflx"}"#)
        );
        assert_eq!(
            CandidateType::of_candidate(
                "candidate:1 1 udp 1686052607 203.0.113.9 51000 typ srflx raddr 0.0.0.0 rport 0"
            ),
            Some(CandidateType::Srflx)
        );
        assert_eq!(CandidateType::of_candidate("candidate:1 1 udp 1"), None);
    }

    #[test]
    fn unknown_message_type_is_a_json_error() {
        assert!(matches!(
            decode::<Open>(br#"{"open":"teleport"}"#),
            Err(FrameError::Json(_))
        ));
    }
}
