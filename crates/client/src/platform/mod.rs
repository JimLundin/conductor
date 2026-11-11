//! What differs per target: how the iroh endpoint is built, and in the
//! browser, the WebRTC upgrade.

#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(target_family = "wasm")]
pub(crate) mod rtc;
#[cfg(target_family = "wasm")]
mod web;

#[cfg(not(target_family = "wasm"))]
pub use native::bind_endpoint;
#[cfg(target_family = "wasm")]
pub use web::bind_endpoint;

/// n0's public relays, named without the root dot (`proto::without_root_dot`).
/// Every endpoint, host included, uses this map: a host advertises its home
/// relay to peers, and a browser that dials the dotted name fails WebKit's
/// certificate check.
pub fn n0_relays() -> iroh::RelayMap {
    iroh::defaults::prod::default_relay_map()
        .urls::<Vec<_>>()
        .iter()
        .map(proto::without_root_dot)
        .collect()
}
