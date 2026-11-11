use iroh::endpoint::{BindError, presets};
use iroh::{Endpoint, RelayMode};

/// Browsers can't send UDP, so this endpoint only ever talks through n0's
/// relays.
pub async fn bind_endpoint() -> Result<Endpoint, BindError> {
    Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Custom(super::n0_relays()))
        .bind()
        .await
}
