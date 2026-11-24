//! A frame's body on the client link is its JSON, and only the link's own
//! frames ([`Frame`]) travel in it. `net::wire` frames the body.

use alloc::vec::Vec;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// A message that opens or answers a stream on the client link.
pub trait Frame: Serialize + DeserializeOwned {}

impl Frame for crate::Open {}

impl Frame for crate::HelloReply {}

impl Frame for crate::CommandResult {}

impl Frame for crate::Feed {}

/// The frame body of `msg`.
///
/// # Errors
///
/// When `msg` doesn't serialize.
pub fn encode<T: Frame>(msg: &T) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(msg)
}

/// The value a frame body holds.
///
/// # Errors
///
/// When the body is not a `T`.
pub fn decode<T: Frame>(body: &[u8]) -> Result<T, serde_json::Error> {
    serde_json::from_slice(body)
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;

    use super::{decode, encode};
    use crate::AuthToken;
    use crate::CommandId;
    use agents::{
        Auth, ClientName, Credential, Decision, Effort, ModelChoice, ModelId,
        Models, Provider, TurnOutcome,
    };
    use host::log::{Epoch, Event, HostSeq};
    use host::registry::{
        Availability, Catalog, InstanceCommand, InstanceId, InstanceStatus,
        Instances,
    };
    use host::session::{
        EventKind, RequestId, Resolution, SessionCommand, SessionId,
        SessionRefusal, SessionResult,
    };

    use crate::{
        Command, CommandResult, Feed, Hello, HelloReply, Open, Subscribe,
        Welcome,
    };

    fn welcome() -> Welcome {
        Welcome {
            host: String::from("desk"),
            epoch: Epoch(1),
            head_seq: HostSeq(0),
            instances: Instances(BTreeMap::from([
                (
                    InstanceId::own(Provider::new("claude")),
                    InstanceStatus {
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
                    },
                ),
                (
                    InstanceId::named(Provider::new("codex"), "a@b"),
                    InstanceStatus {
                        availability: Availability::Unavailable {
                            reason: String::from("no"),
                        },
                    },
                ),
            ])),
            catalog: Catalog::default(),
            home: None,
        }
    }

    #[test]
    /// `serde_json` writes an enum variant as its name, a command id as its
    /// UUID and a session id as its 26-digit ULID.
    fn command_frame_is_json() -> Result<(), Box<dyn std::error::Error>> {
        let open = Open::Command {
            id: CommandId(uuid::Uuid::from_u128(7)),
            body: Command::Session(SessionCommand::SendMessage {
                session: SessionId(u128::MAX),
                text: String::from("hi"),
            }),
        };
        let body = br#"{"Command":{"id":"00000000-0000-0000-0000-000000000007","body":{"Session":{"SendMessage":{"session":"7ZZZZZZZZZZZZZZZZZZZZZZZZZ","text":"hi"}}}}}"#;
        assert_eq!(encode(&open)?, body);
        assert_eq!(decode::<Open>(body)?, open);
        Ok(())
    }

    #[test]
    fn a_credential_goes_out_whole_and_debugs_as_its_kind()
    -> Result<(), Box<dyn std::error::Error>> {
        let open = Open::Command {
            id: CommandId(uuid::Uuid::from_u128(1)),
            body: Command::Instance(InstanceCommand::SetCredential {
                provider: Provider::new("claude"),
                credential: Credential {
                    kind: String::from("gateway"),
                    values: BTreeMap::from([(
                        String::from("key"),
                        String::from("k"),
                    )]),
                },
            }),
        };
        assert_eq!(
            encode(&open)?,
            br#"{"Command":{"id":"00000000-0000-0000-0000-000000000001","body":{"Instance":{"SetCredential":{"provider":"claude","credential":{"kind":"gateway","values":{"key":"k"}}}}}}}"#
        );
        assert_eq!(
            format!("{open:?}"),
            r#"Command { id: CommandId(00000000-0000-0000-0000-000000000001), body: Instance(SetCredential { provider: Provider("claude"), credential: Credential { kind: "gateway", .. } }) }"#
        );
        Ok(())
    }

    #[test]
    /// An epoch is random, so most pass 2^53: it travels as its digits.
    fn hello_and_subscribe_wire_shapes()
    -> Result<(), Box<dyn std::error::Error>> {
        let token = AuthToken::from_bytes([0xab; 16]);
        assert_eq!(
            encode(&Open::Hello(Hello {
                client: ClientName(String::from("phone")),
                token
            }))?,
            br#"{"Hello":{"client":"phone","token":[171,171,171,171,171,171,171,171,171,171,171,171,171,171,171,171]}}"#
        );
        let subscribe = Open::Subscribe(Subscribe {
            epoch: Epoch(u64::MAX),
            after_seq: HostSeq(41),
        });
        let body =
            br#"{"Subscribe":{"epoch":"18446744073709551615","after_seq":41}}"#;
        assert_eq!(encode(&subscribe)?, body);
        assert_eq!(decode::<Open>(body)?, subscribe);
        Ok(())
    }

    #[test]
    /// JSON keys are only strings, so the instances are `[id, status]`
    /// pairs.
    fn the_welcome_lists_its_instances_as_pairs()
    -> Result<(), Box<dyn std::error::Error>> {
        let reply = HelloReply::Welcome(welcome());
        let body = br#"{"Welcome":{"host":"desk","epoch":"1","head_seq":0,"instances":[[{"provider":"claude","account":"Own"},{"availability":{"Available":{"auth":{"Stored":{"kind":"g","url":"u"}},"models":{"choices":[{"id":"h","name":"H","description":"d","efforts":["High"]}],"default":"h"}}}}],[{"provider":"codex","account":{"Named":"a@b"}},{"availability":{"Unavailable":{"reason":"no"}}}]],"catalog":{},"home":null}}"#;
        assert_eq!(encode(&reply)?, body);
        assert_eq!(decode::<HelloReply>(body)?, reply);
        Ok(())
    }

    #[test]
    fn an_instance_given_twice_is_refused() {
        let own = r#"[{"provider":"claude","account":"Own"},{"availability":{"Unavailable":{"reason":"no"}}}]"#;
        let body = format!(r#"{{"Instances":[{own},{own}]}}"#);
        assert!(decode::<Feed>(body.as_bytes()).is_err_and(|error| {
            error.to_string().starts_with("a key is given twice")
        }));
    }

    #[test]
    fn replies_and_events_wire_shapes() -> Result<(), Box<dyn std::error::Error>>
    {
        assert_eq!(
            encode(&Feed::Event(Event {
                seq: HostSeq(3),
                session: SessionId(1),
                kind: EventKind::RequestResolved {
                    request: RequestId(2),
                    resolution: Resolution::Decided {
                        decision: Decision::Approve,
                        by: ClientName(String::from("phone")),
                    },
                },
            }))?,
            br#"{"Event":{"seq":3,"session":"00000000000000000000000001","kind":{"RequestResolved":{"request":2,"resolution":{"Decided":{"decision":"Approve","by":"phone"}}}}}}"#
        );
        assert_eq!(
            encode(&Feed::Event(Event {
                seq: HostSeq(4),
                session: SessionId(1),
                kind: EventKind::TurnCompleted {
                    outcome: TurnOutcome::Failed {
                        message: String::from("boom"),
                    },
                },
            }))?,
            br#"{"Event":{"seq":4,"session":"00000000000000000000000001","kind":{"TurnCompleted":{"outcome":{"Failed":{"message":"boom"}}}}}}"#
        );
        assert_eq!(
            encode(&CommandResult::Session(SessionResult::Refused(
                SessionRefusal::InstanceUnavailable {
                    reason: String::from("no codex"),
                }
            )))?,
            br#"{"Session":{"Refused":{"InstanceUnavailable":{"reason":"no codex"}}}}"#
        );
        assert_eq!(
            encode(&CommandResult::Session(SessionResult::Started {
                session: SessionId(3),
            }))?,
            br#"{"Session":{"Started":{"session":"00000000000000000000000003"}}}"#
        );
        assert_eq!(
            encode(&CommandResult::Session(SessionResult::Accepted))?,
            br#"{"Session":"Accepted"}"#
        );
        Ok(())
    }

    #[test]
    fn an_id_or_epoch_not_in_its_text_is_refused() {
        let session = |text: &str| {
            decode::<CommandResult>(
                format!(r#"{{"Session":{{"Started":{{"session":{text}}}}}}}"#)
                    .as_bytes(),
            )
            .is_err()
        };
        assert!(session("3"));
        assert!(session(r#""0000000000000000000000000U""#));
        assert!(session(r#""80000000000000000000000000""#));
        assert!(session(r#""0000000000000000000000003""#));
        let epoch = |text: &str| {
            decode::<Open>(
                format!(r#"{{"Subscribe":{{"epoch":{text},"after_seq":0}}}}"#)
                    .as_bytes(),
            )
            .is_err()
        };
        assert!(epoch("1"));
        assert!(epoch(r#""+1""#));
        assert!(!epoch(r#""1""#));
    }

    #[test]
    /// Stores and host traffic keep the postcard they had before the client
    /// link became JSON.
    fn postcard_keeps_its_bytes() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            postcard::to_allocvec(&Command::Session(
                SessionCommand::SendMessage {
                    session: SessionId(2),
                    text: String::from("hi"),
                }
            ))?,
            b"\x00\x01\x02\x02hi"
        );
        assert_eq!(
            postcard::to_allocvec(&Subscribe {
                epoch: Epoch(9),
                after_seq: HostSeq(41)
            })?,
            b"\x09\x29"
        );
        let welcome = welcome();
        let bytes = postcard::to_allocvec(&welcome)?;
        assert_eq!(
            bytes,
            b"\x04desk\x01\x00\x02\x06claude\x00\x00\x00\x01g\x01\x01u\x01\x01h\x01H\x01d\x01\x02\x01\x01h\x05codex\x01\x03a@b\x01\x02no\x00\x00"
        );
        assert_eq!(postcard::from_bytes::<Welcome>(&bytes)?, welcome);
        Ok(())
    }

    #[test]
    fn an_unknown_variant_is_a_body_error() {
        assert!(decode::<Open>(br#""Nope""#).is_err_and(|error| {
            error.to_string().starts_with("unknown variant `Nope`")
        }));
    }
}
