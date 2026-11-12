//! What differs per target: the client's name, and in the browser, the
//! WebRTC upgrade. The app runs only in the browser; native builds are the
//! test harness, and get stand-ins for the WebRTC
//! types that no value can inhabit, so the code above this module reads the
//! same on both targets.

#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(target_family = "wasm")]
mod rtc;
#[cfg(target_family = "wasm")]
mod web;

use iroh::endpoint::{BindError, presets};
use iroh::{Endpoint, RelayMode};

#[cfg(not(target_family = "wasm"))]
pub use native::client_name;
#[cfg(not(target_family = "wasm"))]
use native::public_endpoint;
#[cfg(target_family = "wasm")]
pub use web::client_name;
#[cfg(target_family = "wasm")]
use web::public_endpoint;

#[cfg(not(target_family = "wasm"))]
pub(crate) use native::{ChannelRecv, ChannelSend, Session, negotiate};
#[cfg(target_family = "wasm")]
pub(crate) use rtc::{ChannelRecv, ChannelSend, Session, negotiate};

/// Which kind of endpoint the link binds, for its first dial and for every
/// hedge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// n0's relays and DNS lookup.
    Public,
    /// Direct addresses only, with no relays and no lookup, as on loopback.
    Local,
}

pub(crate) async fn bind_endpoint(network: Network) -> Result<Endpoint, BindError> {
    match network {
        Network::Public => public_endpoint().await,
        Network::Local => {
            Endpoint::builder(presets::Minimal)
                .relay_mode(RelayMode::Disabled)
                .bind()
                .await
        }
    }
}

/// n0's public relays, named without the root dot (`proto::without_root_dot`).
/// Every endpoint, host included, uses this map: a host advertises its home
/// relay to peers, and a browser that dials the dotted name fails WebKit's
/// certificate check.
pub fn n0_relays() -> iroh::RelayMap {
    iroh::defaults::prod::default_relay_map()
        .urls::<Vec<iroh::RelayUrl>>()
        .iter()
        .map(proto::without_root_dot)
        .collect()
}
