use iroh::endpoint::{BindError, Connection, presets};
use iroh::{Endpoint, RelayMode};
use iroh_mdns_address_lookup::MdnsAddressLookup;

use proto::ClientName;

use crate::CandidatePair;
use crate::device::{Platform, device_name};
use crate::wire::WireError;

/// n0's relays and DNS lookup, plus mDNS so hosts on the same LAN connect
/// with no relay (§5.3).
pub(crate) async fn public_endpoint() -> Result<Endpoint, BindError> {
    Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Custom(super::n0_relays()))
        .address_lookup(MdnsAddressLookup::builder())
        .bind()
        .await
}

/// The desktop app's name in `Hello`: its OS. The headless example and the
/// tests give their own.
pub fn client_name() -> ClientName {
    device_name(Platform::Native {
        os: String::from(std::env::consts::OS),
    })
}

/// Native clients hole-punch with iroh and never upgrade, so a native build
/// has no WebRTC connection. These stand in for the browser's types so the
/// link reads the same on both targets. No value of them exists, so their
/// methods never run.
#[derive(Debug, Clone)]
pub(crate) enum Session {}

impl Session {
    pub(crate) fn pair(&self) -> Option<CandidatePair> {
        None
    }

    pub(crate) async fn closed(&self) {
        std::future::pending().await
    }

    pub(crate) async fn open_channel(&self) -> Result<(ChannelSend, ChannelRecv), String> {
        std::future::pending().await
    }
}

pub(crate) enum ChannelSend {}

impl ChannelSend {
    pub(crate) fn write(&self, _frame: &[u8]) -> Result<(), String> {
        Ok(())
    }
}

pub(crate) enum ChannelRecv {}

impl ChannelRecv {
    pub(crate) async fn read_body(self) -> (ChannelRecv, Result<Option<Vec<u8>>, WireError>) {
        match self {}
    }
}

/// Never resolves: a native link stays on iroh.
pub(crate) async fn negotiate(
    _conn: Connection,
    _stun_servers: Vec<String>,
) -> Result<Session, String> {
    std::future::pending().await
}
