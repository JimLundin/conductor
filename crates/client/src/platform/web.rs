use iroh::Endpoint;
use iroh::endpoint::{BindError, presets};

/// Browsers can't send UDP, so this endpoint only ever talks through n0's
/// relays.
pub async fn bind_endpoint() -> Result<Endpoint, BindError> {
    Endpoint::builder(presets::N0).bind().await
}
