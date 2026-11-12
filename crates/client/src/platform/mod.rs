//! The endpoint the link binds, and the client's name, which differs per
//! target. The app runs only in the browser; native builds are the test
//! harness.

#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(target_family = "wasm")]
mod web;

use iroh::endpoint::{BindError, presets};
use iroh::{Endpoint, RelayMode};

#[cfg(not(target_family = "wasm"))]
pub use native::client_name;
#[cfg(target_family = "wasm")]
pub use web::client_name;

/// Which kind of endpoint the link binds, for its first dial and for every
/// hedge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// n0's relays and DNS lookup. A browser can't send UDP, so there it
    /// only ever talks through a relay.
    Public,
    /// Direct addresses only, with no relays and no lookup, as on loopback.
    Local,
}

pub(crate) async fn bind_endpoint(network: Network) -> Result<Endpoint, BindError> {
    match network {
        Network::Public => {
            Endpoint::builder(presets::N0)
                .relay_mode(RelayMode::Custom(n0_relays()))
                .bind()
                .await
        }
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
