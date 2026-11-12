use iroh::endpoint::{BindError, presets};
use iroh::{Endpoint, RelayMode};
use proto::ClientName;

use crate::device::{Platform, Touch, device_name};

/// Browsers can't send UDP, so this endpoint only ever talks through n0's
/// relays.
pub(crate) async fn public_endpoint() -> Result<Endpoint, BindError> {
    Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Custom(super::n0_relays()))
        .bind()
        .await
}

/// The device this browser runs on, as `Hello` names it.
pub fn client_name() -> ClientName {
    match web_sys::window() {
        Some(window) => {
            let navigator = window.navigator();
            let touch = match navigator.max_touch_points() {
                0 | 1 => Touch::None,
                _ => Touch::Screen,
            };
            device_name(Platform::Browser {
                user_agent: navigator.user_agent().unwrap_or_default(),
                touch,
            })
        }
        None => device_name(Platform::Browser {
            user_agent: String::new(),
            touch: Touch::None,
        }),
    }
}
