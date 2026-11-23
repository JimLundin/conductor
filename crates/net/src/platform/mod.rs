//! The endpoint the link binds.

use iroh::endpoint::{BindError, presets};
use iroh::{Endpoint, RelayMode};

/// Which kind of endpoint the link binds, for its first dial and for every
/// hedge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// n0's relays and DNS lookup.
    Public,
    /// Direct addresses only, with no relays and no lookup, as on loopback.
    Local,
}

pub(crate) async fn bind_endpoint(
    network: Network,
) -> Result<Endpoint, BindError> {
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

/// n0's public relays, named without the root dot ([`crate::without_root_dot`]).
///
/// Every endpoint, host included, uses this map: a host advertises its home
/// relay to peers, and a browser that dials the dotted name fails `WebKit`'s
/// certificate check.
pub fn n0_relays() -> iroh::RelayMap {
    iroh::defaults::prod::default_relay_map()
        .urls::<Vec<iroh::RelayUrl>>()
        .iter()
        .map(crate::without_root_dot)
        .collect()
}
