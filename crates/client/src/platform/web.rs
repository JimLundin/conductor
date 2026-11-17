use proto::ClientName;

use crate::device::{Touch, device_name};

/// The device this browser runs on, as `Hello` names it.
#[must_use]
pub fn client_name() -> ClientName {
    web_sys::window().map_or_else(
        || device_name("", Touch::None),
        |window| {
            let navigator = window.navigator();
            let touch = match navigator.max_touch_points() {
                0 | 1 => Touch::None,
                _ => Touch::Screen,
            };
            device_name(&navigator.user_agent().unwrap_or_default(), touch)
        },
    )
}
