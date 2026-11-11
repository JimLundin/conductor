use iroh::endpoint::{BindError, presets};
use iroh::{Endpoint, RelayMode};
use iroh_mdns_address_lookup::MdnsAddressLookup;

/// n0's relays and DNS lookup, plus mDNS so hosts on the same LAN connect
/// with no relay (§5.3).
pub async fn bind_endpoint() -> Result<Endpoint, BindError> {
    Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Custom(super::n0_relays()))
        .address_lookup(MdnsAddressLookup::builder())
        .bind()
        .await
}
