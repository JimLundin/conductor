use iroh::endpoint::{BindError, presets};
use iroh::{Endpoint, RelayMap, RelayMode};

/// Browsers can't send UDP, so this endpoint only ever talks through n0's
/// relays, named without the root dot that Safari rejects.
pub async fn bind_endpoint() -> Result<Endpoint, BindError> {
    let relays: RelayMap = iroh::defaults::prod::default_relay_map()
        .urls::<Vec<_>>()
        .iter()
        .map(super::without_root_dot)
        .collect();
    Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Custom(relays))
        .bind()
        .await
}
