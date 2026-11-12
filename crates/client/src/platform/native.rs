//! The app runs only in the browser. A native build of the client is the
//! test harness: the daemon's e2e tests and the headless example.

use iroh::endpoint::{BindError, Connection, presets};
use iroh::{Endpoint, RelayMode};

use proto::ClientName;

use crate::wire::WireError;

/// n0's relays and DNS lookup.
pub(crate) async fn public_endpoint() -> Result<Endpoint, BindError> {
    Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Custom(super::n0_relays()))
        .bind()
        .await
}

/// Its OS. The headless example and the tests give their own names.
pub fn client_name() -> ClientName {
    ClientName(String::from(std::env::consts::OS))
}

/// The native harness stays on iroh and never upgrades, so it has no WebRTC
/// connection. These stand in for the browser's types so the
/// link reads the same on both targets. No value of them exists, so their
/// methods never run.
#[derive(Debug, Clone)]
pub(crate) enum Session {}

impl Session {
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
