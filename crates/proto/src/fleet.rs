//! Signed fleet membership and configuration shared by the app and hosts.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::marker::PhantomData;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::Roster;

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

/// A value signed over the exact JSON bytes it was written as, and
/// carried as those bytes.
///
/// JSON has no canonical form, so nothing re-encodes it to verify it: a
/// verifier checks `signature` over [`Payload::DOMAIN`] then `payload`,
/// and only then parses `payload`.
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

/// What a fleet signs, under a domain of its own so that a signature over
/// one kind never verifies as another.
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
    /// Over the configuration's postcard, which only hosts read.
    pub signature: Vec<u8>,
    /// `config.snapshot` as clients get it.
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
    EnrollmentReady(crate::AuthToken),
    Enroll {
        token: crate::AuthToken,
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

/// A command about the fleet, from the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum FleetCommand {
    Status,
    Invite,
    Revoke(NodeId),
    Approve(PairRequest),
    Admit { ticket: crate::Ticket, name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum FleetResult {
    Accepted,
    Status(SignedSnapshot),
    Invite(Invite),
    Pairing(Box<Pairing>),
    /// Fleet changes need the root host.
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid fleet text")]
pub struct TextError;

macro_rules! text_artifact {
    ($kind:ty) => {
        impl $kind {
            /// Encodes the fleet artifact as fragment-safe text.
            ///
            /// # Errors
            ///
            /// If the artifact cannot be encoded or exceeds the exchange budget.
            pub fn to_text(&self) -> Result<String, TextError> {
                encode_text(self)
            }

            /// Parses a fleet artifact from fragment-safe text.
            ///
            /// # Errors
            ///
            /// If the text is malformed or exceeds the exchange budget.
            pub fn parse_text(text: &str) -> Result<Self, TextError> {
                decode_text(text)
            }
        }
    };
}

text_artifact!(Invite);
text_artifact!(PairRequest);
text_artifact!(Pairing);

/// Encodes the app's saved pairing and latest known roster.
///
/// # Errors
///
/// If the state cannot be encoded or exceeds the storage budget.
pub fn encode_saved(
    pairing: &Pairing,
    snapshot: Option<&Snapshot>,
) -> Result<String, TextError> {
    encode_text(&(pairing, snapshot))
}

/// Decodes the app's saved pairing and latest known roster.
///
/// # Errors
///
/// If the state is malformed or exceeds the storage budget.
pub fn decode_saved(
    text: &str,
) -> Result<(Pairing, Option<Snapshot>), TextError> {
    decode_text(text)
}

/// Encodes an invitation or pairing artifact as fragment-safe text.
///
/// # Errors
///
/// If the value cannot be encoded or exceeds the exchange budget.
fn encode_text<T: Serialize>(value: &T) -> Result<String, TextError> {
    let bytes = postcard::to_allocvec(value).map_err(|_| TextError)?;
    if bytes.len() > 16_384 {
        return Err(TextError);
    }
    Ok(data_encoding::BASE64URL_NOPAD.encode(&bytes))
}

/// Parses a fragment-safe invitation or pairing artifact.
///
/// # Errors
///
/// If the text is malformed or exceeds the exchange budget.
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
