//! A frame is a `u32` big-endian body length followed by a postcard body.
//! Callers own the I/O: read [`HEADER_LEN`] bytes, pass them to [`body_len`],
//! read that many bytes, pass them to [`decode`].

use alloc::vec::Vec;

use serde::Serialize;
use serde::de::DeserializeOwned;

pub const HEADER_LEN: usize = 4;

/// Larger frames are refused before their body is read. 16 MiB.
pub const MAX_FRAME_LEN: u32 = 16_777_216;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame of {0} bytes exceeds the {MAX_FRAME_LEN}-byte limit")]
    TooLarge(u64),
    #[error("malformed frame body: {0}")]
    Body(#[from] postcard::Error),
}

/// The frame of `msg`.
///
/// # Errors
///
/// [`FrameError::Body`] when `msg` doesn't serialize, and
/// [`FrameError::TooLarge`] when its body exceeds the 16 MiB limit.
pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let body = postcard::to_allocvec(msg)?;
    match u32::try_from(body.len()) {
        Ok(len @ ..=MAX_FRAME_LEN) => Ok(len
            .to_be_bytes()
            .into_iter()
            .chain(body)
            .collect::<Vec<u8>>()),
        Ok(_) | Err(_) => Err(FrameError::TooLarge(
            u64::try_from(body.len()).unwrap_or(u64::MAX),
        )),
    }
}

/// The body length a frame header announces.
///
/// # Errors
///
/// [`FrameError::TooLarge`] when it exceeds the 16 MiB limit.
pub fn body_len(header: [u8; HEADER_LEN]) -> Result<usize, FrameError> {
    let len = u32::from_be_bytes(header);
    match len {
        ..=MAX_FRAME_LEN => usize::try_from(len)
            .map_err(|_| FrameError::TooLarge(u64::from(len))),
        _ => Err(FrameError::TooLarge(u64::from(len))),
    }
}

/// The value a frame body holds.
///
/// # Errors
///
/// [`FrameError::Body`] when the body is not a `T`.
pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, FrameError> {
    Ok(postcard::from_bytes(body)?)
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;

    use super::{HEADER_LEN, body_len, decode, encode};
    use crate::{
        Auth, AuthToken, Availability, Catalog, ClientName, Command,
        CommandBody, CommandId, CommandResult, Credential, Decision, Effort,
        Epoch, Event, EventKind, Hello, HelloReply, HostSeq, InstanceId,
        InstanceStatus, Instances, ModelChoice, ModelId, Models, Open,
        Provider, Rejection, RequestId, Resolution, SessionId, Subscribe,
        TurnOutcome, Welcome,
    };

    fn frame(body: &[u8]) -> Vec<u8> {
        u32::try_from(body.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes()
            .into_iter()
            .chain(body.iter().copied())
            .collect()
    }

    #[test]
    /// Postcard writes enum variants as their index and integers and lengths
    /// as varints.
    fn command_frame_is_length_then_postcard()
    -> Result<(), Box<dyn std::error::Error>> {
        let open = Open::Command(Command {
            id: CommandId(7),
            body: CommandBody::SendMessage {
                session: SessionId(2),
                text: String::from("hi"),
            },
        });
        let bytes = encode(&open)?;
        assert_eq!(bytes, b"\x00\x00\x00\x07\x02\x07\x01\x02\x02hi");
        let (header, body) = bytes
            .split_first_chunk::<HEADER_LEN>()
            .ok_or("expected a value")?;
        assert_eq!(body_len(header.to_owned())?, 7);
        assert_eq!(decode::<Open>(body)?, open);
        Ok(())
    }

    #[test]
    fn checklist_and_subagent_frames_have_their_wire_shapes()
    -> Result<(), Box<dyn std::error::Error>> {
        let checklist =
            decode::<Event>(&[1, 7, 24, 2, 1, b'a', 2, 1, b'a', 0])?;
        assert_eq!(
            checklist.kind,
            EventKind::ChecklistChanged {
                tasks: Vec::from([
                    crate::ChecklistEntry {
                        text: String::from("a"),
                        status: crate::ChecklistStatus::Completed
                    },
                    crate::ChecklistEntry {
                        text: String::from("a"),
                        status: crate::ChecklistStatus::Pending
                    },
                ])
            }
        );
        let child = decode::<Event>(&[1, 7, 26, 1, b'a', 0, 1, b'c', 1, b'd'])?;
        assert_eq!(
            child,
            Event {
                seq: HostSeq(1),
                session: SessionId(7),
                kind: EventKind::Subagent {
                    agent: crate::SubagentId(String::from("a")),
                    event: crate::SubagentEvent::Started {
                        call: crate::ToolCallId(String::from("c")),
                        description: String::from("d"),
                    },
                },
            }
        );
        Ok(())
    }

    #[test]
    fn a_credential_goes_out_whole_and_debugs_as_its_kind()
    -> Result<(), Box<dyn std::error::Error>> {
        let open = Open::Command(Command {
            id: CommandId(1),
            body: CommandBody::SetCredential {
                provider: Provider::Claude,
                credential: Credential {
                    kind: String::from("gateway"),
                    values: BTreeMap::from([(
                        String::from("key"),
                        String::from("k"),
                    )]),
                },
            },
        });
        assert_eq!(
            encode(&open)?,
            frame(b"\x02\x01\x06\x00\x07gateway\x01\x03key\x01k")
        );
        assert_eq!(
            format!("{open:?}"),
            r#"Command(Command { id: CommandId(1), body: SetCredential { provider: Claude, credential: Credential { kind: "gateway", .. } } })"#
        );
        Ok(())
    }

    #[test]
    fn hello_and_subscribe_wire_shapes()
    -> Result<(), Box<dyn std::error::Error>> {
        let token = AuthToken::from_bytes([0xab; 16]);
        assert_eq!(
            encode(&Open::Hello(Hello {
                client: ClientName(String::from("phone")),
                token
            }))
            ?,
            frame(b"\x00\x05phone\xab\xab\xab\xab\xab\xab\xab\xab\xab\xab\xab\xab\xab\xab\xab\xab")
        );
        assert_eq!(
            encode(&Open::Subscribe(Subscribe {
                epoch: Epoch(9),
                after_seq: HostSeq(41)
            }))?,
            frame(b"\x01\x09\x29")
        );
        Ok(())
    }

    #[test]
    fn replies_and_events_wire_shapes() -> Result<(), Box<dyn std::error::Error>>
    {
        assert_eq!(
            encode(&HelloReply::Welcome(Welcome {
                host: String::from("desk"),
                epoch: Epoch(1),
                head_seq: HostSeq(0),
                instances: Instances(BTreeMap::from([
                    (InstanceId::own(Provider::Claude), InstanceStatus {
                        availability: Availability::Available {
                            auth: Auth::Stored {
                                kind: String::from("g"),
                                url: Some(String::from("u")),
                            },
                            models: Models {
                                choices: Vec::from([ModelChoice {
                                    id: ModelId(String::from("h")),
                                    name: String::from("H"),
                                    description: String::from("d"),
                                    efforts: Vec::from([Effort::High]),
                                }]),
                                default: Some(ModelId(String::from("h"))),
                            },
                        },
                    }),
                    (InstanceId::named(Provider::Codex, "a@b"), InstanceStatus {
                        availability: Availability::Unavailable {
                            reason: String::from("no"),
                        },
                    }),
                ])),
                catalog: Catalog::default(),
                home: None,
            }))
            ?,
            frame(
                b"\x00\x04desk\x01\x00\x02\x00\x00\x00\x00\x01g\x01\x01u\x01\x01h\x01H\x01d\x01\x02\x01\x01h\x01\x01\x03a@b\x01\x02no\x00\x00"
            )
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
            })?,
            frame(b"\x03\x01\x0b\x02\x00\x00\x05phone")
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
            })?,
            frame(b"\x04\x01\x0c\x02\x04boom")
        );
        assert_eq!(
            encode(&CommandResult::Rejected(Rejection::InstanceUnavailable {
                reason: String::from("no codex"),
            }))?,
            frame(b"\x02\x01\x08no codex")
        );
        assert_eq!(
            encode(&CommandResult::Started {
                session: SessionId(3),
            })?,
            frame(b"\x01\x03")
        );
        assert_eq!(encode(&CommandResult::Accepted)?, frame(b"\x00"));
        Ok(())
    }

    #[test]
    fn oversized_header_is_refused_before_reading_the_body()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            body_len([0x01, 0x00, 0x00, 0x01])
                .map_err(|error| error.to_string()),
            Err(String::from(
                "frame of 16777217 bytes exceeds the 16777216-byte limit"
            ))
        );
        assert_eq!(body_len([0x01, 0x00, 0x00, 0x00])?, 16_777_216);
        Ok(())
    }

    #[test]
    fn an_unknown_variant_is_a_body_error() {
        assert_eq!(
            decode::<Open>(b"\x07").map_err(|error| error.to_string()),
            Err(String::from(
                "malformed frame body: Serde Deserialization Error"
            ))
        );
    }
}
