use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::marker::PhantomData;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::EnrollmentToken;
use crate::registry::Roster;

pub const ALPN: &[u8] = b"conductor-fleet/4";

pub type NodeId = [u8; 32];

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum Role {
    Owner,
    Host,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Identity {
    pub fleet: NodeId,
    pub device: NodeId,
    pub name: String,
    pub role: Role,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(bound = "")]
pub struct Signed<T> {
    pub payload: Vec<u8>,
    pub signature: Vec<u8>,
    #[serde(skip)]
    kind: PhantomData<fn() -> T>,
}

impl<T> Signed<T> {
    #[must_use]
    pub const fn new(payload: Vec<u8>, signature: Vec<u8>) -> Self {
        Self {
            payload,
            signature,
            kind: PhantomData,
        }
    }
}

pub trait Payload: Serialize + DeserializeOwned {
    const DOMAIN: &'static [u8];
}

impl Payload for Identity {
    const DOMAIN: &'static [u8] = b"conductor fleet certificate v2";
}

impl Payload for Snapshot {
    const DOMAIN: &'static [u8] = b"conductor fleet snapshot v2";
}

pub type Certificate = Signed<Identity>;
pub type SignedSnapshot = Signed<Snapshot>;

impl<T> Clone for Signed<T> {
    fn clone(&self) -> Self {
        Self {
            payload: self.payload.clone(),
            signature: self.signature.clone(),
            kind: PhantomData,
        }
    }
}

impl<T> PartialEq for Signed<T> {
    fn eq(&self, other: &Self) -> bool {
        self.payload == other.payload && self.signature == other.signature
    }
}

impl<T> Eq for Signed<T> {}

impl<T> fmt::Debug for Signed<T> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Signed")
            .field("payload", &self.payload)
            .field("signature", &self.signature)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostEntry {
    pub node: NodeId,
    pub name: String,
    pub relay: String,
    pub added_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Snapshot {
    pub fleet: NodeId,
    pub root: NodeId,
    pub version: u64,
    pub hosts: Vec<HostEntry>,
    pub revoked: Vec<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Configuration {
    pub snapshot: Snapshot,
    pub instances: Roster,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SignedConfiguration {
    pub config: Configuration,
    pub signature: Vec<u8>,
    pub public: SignedSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Peer {
    pub certificate: Certificate,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Sync {
    pub peer: Peer,
    pub config: Option<SignedConfiguration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Request {
    EnrollmentReady(EnrollmentToken),
    Enroll {
        token: EnrollmentToken,
        certificate: Certificate,
        config: Box<SignedConfiguration>,
    },
    Sync(Peer),
    Join {
        invite: InviteSecret,
        host: HostEntry,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Reply {
    EnrollmentReady,
    Enrolled,
    Sync(Sync),
    Joined {
        certificate: Certificate,
        config: SignedConfiguration,
    },
    Refused,
    Revoked(SignedSnapshot),
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InviteSecret(pub [u8; 32]);

impl core::fmt::Debug for InviteSecret {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("InviteSecret")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Invite {
    pub fleet: NodeId,
    pub root: NodeId,
    pub relay: String,
    pub secret: InviteSecret,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceHello {
    pub certificate: Certificate,
    pub proof: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Pairing {
    pub certificate: Certificate,
    pub root: HostEntry,
    pub signed: SignedSnapshot,
}

#[must_use]
pub fn device_proof(fleet: &NodeId, host: &NodeId, peer: &NodeId) -> Vec<u8> {
    b"conductor fleet device session v1"
        .iter()
        .chain(fleet)
        .chain(host)
        .chain(peer)
        .copied()
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PairRequest {
    pub device: NodeId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum FleetCommand {
    Status,
    Invite,
    Revoke(NodeId),
    Approve(PairRequest),
    Admit {
        node: NodeId,
        relay: String,
        name: String,
        token: EnrollmentToken,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum FleetResult {
    Accepted,
    Status(SignedSnapshot),
    Invite(Invite),
    Pairing(Box<Pairing>),
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid fleet text")]
pub struct TextError;

impl Invite {
    pub fn to_text(&self) -> Result<String, TextError> {
        encode_text(self)
    }

    pub fn parse_text(text: &str) -> Result<Self, TextError> {
        decode_text(text)
    }
}

impl PairRequest {
    pub fn to_text(&self) -> Result<String, TextError> {
        encode_text(self)
    }

    pub fn parse_text(text: &str) -> Result<Self, TextError> {
        decode_text(text)
    }
}

impl Pairing {
    pub fn to_text(&self) -> Result<String, TextError> {
        encode_text(self)
    }

    pub fn parse_text(text: &str) -> Result<Self, TextError> {
        decode_text(text)
    }
}

pub fn encode_saved(
    pairing: &Pairing,
    snapshot: Option<&Snapshot>,
) -> Result<String, TextError> {
    encode_text(&(pairing, snapshot))
}

pub fn decode_saved(
    text: &str,
) -> Result<(Pairing, Option<Snapshot>), TextError> {
    decode_text(text)
}

fn encode_text<T: Serialize>(value: &T) -> Result<String, TextError> {
    let bytes = postcard::to_allocvec(value).map_err(|_| TextError)?;
    if bytes.len() > 16_384 {
        return Err(TextError);
    }
    Ok(data_encoding::BASE64URL_NOPAD.encode(&bytes))
}

fn decode_text<T: serde::de::DeserializeOwned>(
    value: &str,
) -> Result<T, TextError> {
    let trimmed = value.trim();
    if trimmed.len() > 21_846 {
        return Err(TextError);
    }
    let bytes = data_encoding::BASE64URL_NOPAD
        .decode(trimmed.as_bytes())
        .map_err(|_| TextError)?;
    postcard::from_bytes(&bytes).map_err(|_| TextError)
}
