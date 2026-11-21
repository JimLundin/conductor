//! Signed fleet membership and configuration shared by the app and hosts.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::{ModelId, Provider, Roster};

pub const ALPN: &[u8] = b"conductor-fleet/1";
pub const SNAPSHOT_DOMAIN: &[u8] = b"conductor fleet snapshot v1";
pub const CERTIFICATE_DOMAIN: &[u8] = b"conductor fleet certificate v1";

pub type NodeId = [u8; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Owner,
    Host,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub fleet: NodeId,
    pub device: NodeId,
    pub name: String,
    pub role: Role,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Certificate {
    pub identity: Identity,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEntry {
    pub node: NodeId,
    pub name: String,
    pub relay: String,
    pub added_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pins(pub BTreeMap<Provider, Pin>);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub version: String,
    pub digests: BTreeMap<String, String>,
}

impl Pins {
    #[must_use]
    pub fn get(&self, provider: Provider) -> Option<&Pin> {
        self.0.get(&provider)
    }

    #[must_use]
    pub fn version(&self, provider: Provider) -> Option<&str> {
        self.get(provider).map(|pin| pin.version.as_str())
    }
}

pub type Flags = BTreeMap<String, bool>;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub models: BTreeMap<Provider, ModelId>,
    pub flags: BTreeMap<Provider, Flags>,
}

impl Settings {
    #[must_use]
    pub fn provider_flags(&self, provider: Provider) -> Flags {
        self.flags.get(&provider).cloned().unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    pub pins: Pins,
    pub settings: Settings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub fleet: NodeId,
    pub root: NodeId,
    pub version: u64,
    pub hosts: Vec<HostEntry>,
    pub revoked: Vec<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Configuration {
    pub snapshot: Snapshot,
    pub pins: Pins,
    pub settings: Settings,
    pub instances: Roster,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedConfiguration {
    pub config: Configuration,
    pub signature: Vec<u8>,
    pub public_signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedSnapshot {
    pub snapshot: Snapshot,
    pub signature: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    pub certificate: Certificate,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sync {
    pub peer: Peer,
    pub config: Option<SignedConfiguration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InviteSecret(pub [u8; 32]);

impl core::fmt::Debug for InviteSecret {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("InviteSecret")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub fleet: NodeId,
    pub root: NodeId,
    pub relay: String,
    pub secret: InviteSecret,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceHello {
    pub certificate: Certificate,
    pub proof: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pairing {
    pub certificate: Certificate,
    pub root: HostEntry,
    pub signed: SignedSnapshot,
}

impl Pairing {
    #[must_use]
    pub fn matches_owner_browser(&self, device: NodeId) -> bool {
        let identity = &self.certificate.identity;
        let snapshot = &self.signed.snapshot;
        identity.device == device
            && identity.role == Role::Owner
            && snapshot.fleet == identity.fleet
            && snapshot.root == self.root.node
            && snapshot.hosts.contains(&self.root)
            && !snapshot.revoked.contains(&device)
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRequest {
    pub device: NodeId,
    pub name: String,
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
