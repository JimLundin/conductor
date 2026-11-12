//! A frame is a `u32` big-endian body length followed by a postcard body.
//! Callers own the I/O: read [`HEADER_LEN`] bytes, pass them to [`body_len`],
//! read that many bytes, pass them to [`decode`].

use std::cmp::Ordering;

use serde::Serialize;
use serde::de::DeserializeOwned;

pub const HEADER_LEN: usize = 4;

/// Larger frames are refused before their body is read. 16 MiB.
pub(crate) const MAX_FRAME_LEN: u32 = 16_777_216;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame of {0} bytes exceeds the {MAX_FRAME_LEN}-byte limit")]
    TooLarge(u64),
    #[error("malformed frame body: {0}")]
    Body(#[from] postcard::Error),
}

pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    let body = postcard::to_allocvec(msg)?;
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
    Ok(postcard::from_bytes(body)?)
}

#[cfg(test)]
mod tests {
    use super::{HEADER_LEN, body_len, decode, encode};
    use crate::{
        Auth, AuthToken, Availability, ClientName, CodexCredential, Command, CommandBody,
        CommandId, CommandResult, Credential, Decision, Effort, Epoch, Event, EventKind, Hello,
        HelloReply, HostSeq, ModelChoice, ModelId, Models, Open, Providers, Rejection, RequestId,
        Resolution, SessionId, Subscribe, TurnOutcome, Welcome,
    };

    fn frame(body: &[u8]) -> Vec<u8> {
        u32::try_from(body.len())
            .unwrap()
            .to_be_bytes()
            .into_iter()
            .chain(body.iter().copied())
            .collect()
    }

    #[test]
    /// Postcard writes enum variants as their index and integers and lengths
    /// as varints.
    fn command_frame_is_length_then_postcard() {
        let open = Open::Command(Command {
            id: CommandId(7),
            body: CommandBody::SendMessage {
                session: SessionId(2),
                text: String::from("hi"),
            },
        });
        let bytes = encode(&open).unwrap();
        assert_eq!(bytes, b"\x00\x00\x00\x07\x02\x07\x01\x02\x02hi");
        let (header, body) = bytes.split_first_chunk::<HEADER_LEN>().unwrap();
        assert_eq!(body_len(header.to_owned()).unwrap(), 7);
        assert_eq!(decode::<Open>(body).unwrap(), open);
    }

    #[test]
    fn a_credential_goes_out_whole_and_debugs_as_its_kind() {
        let open = Open::Command(Command {
            id: CommandId(1),
            body: CommandBody::SetCredential {
                credential: Credential::Codex(CodexCredential::Provider {
                    name: String::from("n"),
                    base_url: String::from("u"),
                    key: String::from("k"),
                    model: String::from("m"),
                }),
            },
        });
        assert_eq!(
            encode(&open).unwrap(),
            frame(b"\x02\x01\x06\x01\x00\x01n\x01u\x01k\x01m")
        );
        assert_eq!(
            format!("{open:?}"),
            "Command(Command { id: CommandId(1), body: SetCredential { credential: Codex(Provider) } })"
        );
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
            frame(b"\x00\x05phone\xab\xab\xab\xab\xab\xab\xab\xab\xab\xab\xab\xab\xab\xab\xab\xab")
        );
        assert_eq!(
            encode(&Open::Subscribe(Subscribe {
                epoch: Epoch(9),
                after_seq: HostSeq(41)
            }))
            .unwrap(),
            frame(b"\x01\x09\x29")
        );
    }

    #[test]
    fn replies_and_events_wire_shapes() {
        assert_eq!(
            encode(&HelloReply::Welcome(Welcome {
                host: String::from("desk"),
                epoch: Epoch(1),
                head_seq: HostSeq(0),
                providers: Providers {
                    claude: Availability::Available {
                        auth: Auth::Gateway {
                            base_url: String::from("u"),
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
                    codex: Availability::Unavailable {
                        reason: String::from("no"),
                    },
                },
            }))
            .unwrap(),
            frame(
                b"\x00\x04desk\x01\x00\x00\x00\x01u\x01\x01h\x01H\x01d\x01\x02\x01\x01h\x01\x02no"
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
            })
            .unwrap(),
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
            })
            .unwrap(),
            frame(b"\x04\x01\x0c\x02\x04boom")
        );
        assert_eq!(
            encode(&CommandResult::Rejected(Rejection::ProviderUnavailable {
                reason: String::from("no codex"),
            }))
            .unwrap(),
            frame(b"\x03\x01\x08no codex")
        );
        assert_eq!(
            encode(&CommandResult::Started {
                session: SessionId(3),
            })
            .unwrap(),
            frame(b"\x01\x03")
        );
        assert_eq!(encode(&CommandResult::Accepted).unwrap(), frame(b"\x00"));
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
    fn an_unknown_variant_is_a_body_error() {
        assert_eq!(
            decode::<Open>(b"\x07").map_err(|error| error.to_string()),
            Err(String::from(
                "malformed frame body: Serde Deserialization Error"
            ))
        );
    }
}
