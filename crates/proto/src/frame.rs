//! A frame is a `u32` big-endian body length followed by a JSON body (D8).
//! Callers own the I/O: read [`HEADER_LEN`] bytes, pass them to [`body_len`],
//! read that many bytes, pass them to [`decode`].
//!
//! A WebRTC data channel delivers messages, not a byte stream, so a sender
//! splits each encoded frame into messages of at most [`CHUNK_LEN`] bytes and
//! the receiver feeds them to a [`FrameBuffer`].

use std::cmp::Ordering;

use serde::Serialize;
use serde::de::DeserializeOwned;

pub const HEADER_LEN: usize = 4;

/// Larger frames are refused before their body is read. 16 MiB.
pub(crate) const MAX_FRAME_LEN: u32 = 16_777_216;

/// The largest data channel message we send, 16 KiB. Every browser accepts
/// 16 KiB messages (RFC 8831 §6.6), and the length prefix makes boundaries
/// irrelevant to the receiver.
pub const CHUNK_LEN: usize = 16_384;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame of {0} bytes exceeds the {MAX_FRAME_LEN}-byte limit")]
    TooLarge(u64),
    #[error("malformed frame body: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let body = serde_json::to_vec(msg)?;
    match u32::try_from(body.len()).map(|len| (len, len.cmp(&MAX_FRAME_LEN))) {
        Ok((len, Ordering::Less | Ordering::Equal)) => Ok(len
            .to_be_bytes()
            .into_iter()
            .chain(body)
            .collect::<Vec<u8>>()),
        Ok((_, Ordering::Greater)) | Err(_) => Err(FrameError::TooLarge(
            u64::try_from(body.len()).unwrap_or(u64::MAX),
        )),
    }
}

pub fn body_len(header: [u8; HEADER_LEN]) -> Result<usize, FrameError> {
    let len = u32::from_be_bytes(header);
    match len.cmp(&MAX_FRAME_LEN) {
        Ordering::Less | Ordering::Equal => {
            usize::try_from(len).map_err(|_| FrameError::TooLarge(u64::from(len)))
        }
        Ordering::Greater => Err(FrameError::TooLarge(u64::from(len))),
    }
}

pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, FrameError> {
    Ok(serde_json::from_slice(body)?)
}

/// Reassembles frames from data channel messages, which split and join
/// frames arbitrarily.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameBuffer {
    bytes: Vec<u8>,
}

/// What [`FrameBuffer::next_body`] found at the front of the buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reassembly {
    /// A whole frame's body, and the buffer that remains after it.
    Frame { body: Vec<u8>, rest: FrameBuffer },
    /// The next frame has not fully arrived.
    Incomplete,
}

/// What is left in a [`FrameBuffer`] when its channel ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remainder {
    /// The buffer sits between frames: the channel can end here without
    /// losing data.
    Nothing,
    PartialFrame,
}

impl FrameBuffer {
    pub fn empty() -> FrameBuffer {
        FrameBuffer { bytes: Vec::new() }
    }

    pub fn push(self, message: &[u8]) -> FrameBuffer {
        let mut bytes = self.bytes;
        bytes.extend_from_slice(message);
        FrameBuffer { bytes }
    }

    /// The next complete frame's body, if all of it has arrived.
    pub fn next_body(&self) -> Result<Reassembly, FrameError> {
        match self.bytes.split_first_chunk::<HEADER_LEN>() {
            None => Ok(Reassembly::Incomplete),
            Some((header, rest)) => Ok(match rest.split_at_checked(body_len(header.to_owned())?) {
                Some((body, after)) => Reassembly::Frame {
                    body: body.to_vec(),
                    rest: FrameBuffer {
                        bytes: after.to_vec(),
                    },
                },
                None => Reassembly::Incomplete,
            }),
        }
    }

    pub fn remainder(&self) -> Remainder {
        match self.bytes.first() {
            None => Remainder::Nothing,
            Some(_) => Remainder::PartialFrame,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameBuffer, HEADER_LEN, Reassembly, Remainder, body_len, decode, encode};
    use crate::{
        AuthToken, CandidateType, ClientName, Command, CommandBody, CommandId, CommandResult,
        Decision, Epoch, Event, EventKind, Hello, HelloReply, HostSeq, Open, Rejection, RequestId,
        Resolution, RtcOffer, SessionId, Signal, Subscribe, TurnOutcome, Welcome,
    };

    fn frame(body: &str) -> Vec<u8> {
        u32::try_from(body.len())
            .unwrap()
            .to_be_bytes()
            .into_iter()
            .chain(body.bytes())
            .collect()
    }

    /// A buffer and the bodies taken out of it so far.
    struct Drained {
        buffer: FrameBuffer,
        bodies: Vec<String>,
    }

    fn drain(buffer: FrameBuffer, bodies: Vec<String>) -> Drained {
        match buffer.next_body().unwrap() {
            Reassembly::Frame { body, rest } => drain(
                rest,
                bodies
                    .into_iter()
                    .chain(std::iter::once(String::from_utf8(body).unwrap()))
                    .collect(),
            ),
            Reassembly::Incomplete => Drained { buffer, bodies },
        }
    }

    #[test]
    fn command_frame_is_length_then_json() {
        let open = Open::Command(Command {
            id: CommandId(7),
            body: CommandBody::SendMessage {
                session: SessionId(2),
                text: String::from("hi"),
            },
        });
        let bytes = encode(&open).unwrap();
        assert_eq!(
            bytes,
            b"\x00\x00\x00\x50{\"open\":\"command\",\"id\":7,\"body\":{\"type\":\"send_message\",\"session\":2,\"text\":\"hi\"}}"
        );
        let (header, body) = bytes.split_first_chunk::<HEADER_LEN>().unwrap();
        assert_eq!(body_len(header.to_owned()).unwrap(), 80);
        assert_eq!(decode::<Open>(body).unwrap(), open);
    }

    #[test]
    fn hello_and_subscribe_wire_shapes() {
        let token = AuthToken::from_bytes([0xab; 16]);
        assert_eq!(
            encode(&Open::Hello(Hello {
                client: ClientName(String::from("phone")),
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
                session: SessionId(1),
                kind: EventKind::RequestResolved {
                    request: RequestId(2),
                    resolution: Resolution::Decided {
                        decision: Decision::Approve,
                        by: ClientName(String::from("phone")),
                    },
                },
            })
            .unwrap(),
            frame(
                r#"{"seq":3,"session":1,"kind":{"type":"request_resolved","request":2,"resolution":{"resolution":"decided","decision":"approve","by":"phone"}}}"#
            )
        );
        assert_eq!(
            encode(&Event {
                seq: HostSeq(4),
                session: SessionId(1),
                kind: EventKind::TurnCompleted {
                    outcome: TurnOutcome::Failed {
                        message: String::from("boom"),
                    },
                },
            })
            .unwrap(),
            frame(
                r#"{"seq":4,"session":1,"kind":{"type":"turn_completed","outcome":{"outcome":"failed","message":"boom"}}}"#
            )
        );
        assert_eq!(
            encode(&CommandResult::Rejected(Rejection::ProviderUnavailable {
                message: String::from("no codex"),
            }))
            .unwrap(),
            frame(r#"{"result":"rejected","reason":"provider_unavailable","message":"no codex"}"#)
        );
        assert_eq!(
            encode(&CommandResult::Started {
                session: SessionId(3),
            })
            .unwrap(),
            frame(r#"{"result":"started","session":3}"#)
        );
        assert_eq!(
            encode(&CommandResult::Accepted).unwrap(),
            frame(r#"{"result":"accepted"}"#)
        );
    }

    #[test]
    fn oversized_header_is_refused_before_reading_the_body() {
        assert_eq!(
            body_len([0x01, 0x00, 0x00, 0x01]).map_err(|error| error.to_string()),
            Err(String::from(
                "frame of 16777217 bytes exceeds the 16777216-byte limit"
            ))
        );
        assert_eq!(body_len([0x01, 0x00, 0x00, 0x00]).unwrap(), 16_777_216);
    }

    #[test]
    fn frame_buffer_reassembles_frames_split_and_joined_across_messages() {
        let first = encode(&Event {
            seq: HostSeq(1),
            session: SessionId(1),
            kind: EventKind::SessionReady,
        })
        .unwrap();
        let second = encode(&CommandResult::Accepted).unwrap();
        let stream = first.into_iter().chain(second).collect::<Vec<u8>>();
        let drained = stream.chunks(5).fold(
            Drained {
                buffer: FrameBuffer::empty(),
                bodies: Vec::new(),
            },
            |drained, message| drain(drained.buffer.push(message), drained.bodies),
        );
        assert_eq!(
            drained.bodies,
            [
                r#"{"seq":1,"session":1,"kind":{"type":"session_ready"}}"#,
                r#"{"result":"accepted"}"#
            ]
        );
        assert_eq!(drained.buffer.remainder(), Remainder::Nothing);
        let partial = drained.buffer.push(&[0_u8; 2]);
        assert_eq!(partial.next_body().unwrap(), Reassembly::Incomplete);
        assert_eq!(partial.remainder(), Remainder::PartialFrame);
    }

    #[test]
    fn rtc_signal_wire_shapes() {
        assert_eq!(
            encode(&Open::Rtc(RtcOffer {
                sdp: String::from("v=0")
            }))
            .unwrap(),
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
        assert_eq!(
            decode::<Open>(br#"{"open":"teleport"}"#).map_err(|error| error.to_string()),
            Err(String::from(
                "malformed frame body: unknown variant `teleport`, expected one of `hello`, `subscribe`, `command`, `rtc` at line 1 column 18"
            ))
        );
    }
}
