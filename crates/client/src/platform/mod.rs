//! What differs per target: how the iroh endpoint is built, and in the
//! browser, the WebRTC upgrade. Native builds get stand-ins for the WebRTC
//! types that no value can inhabit, so the code above this module reads the
//! same on both targets.

use std::ops::Deref;

#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(target_family = "wasm")]
mod rtc;
#[cfg(target_family = "wasm")]
mod web;

#[cfg(not(target_family = "wasm"))]
pub use native::bind_endpoint;
#[cfg(target_family = "wasm")]
pub use web::bind_endpoint;

#[cfg(not(target_family = "wasm"))]
pub(crate) use native::{ChannelRecv, ChannelSend, Session, negotiate};
#[cfg(target_family = "wasm")]
pub(crate) use rtc::{ChannelRecv, ChannelSend, Session, negotiate};

/// iroh names its relays as fully qualified hosts ending in `.`. WebKit (Safari)
/// checks the TLS certificate against that literal name and refuses it, so
/// every relay URL a client dials goes through here. DNS treats both forms alike.
pub(crate) fn without_root_dot(url: &iroh::RelayUrl) -> iroh::RelayUrl {
    let mut plain = url.deref().clone();
    let host = plain
        .host_str()
        .and_then(|host| host.strip_suffix('.'))
        .map(String::from);
    if let Some(host) = host {
        plain
            .set_host(Some(&host))
            .expect("a relay host stays valid without its root dot");
    }
    plain.into()
}

#[cfg(test)]
mod tests {
    use super::without_root_dot;

    #[test]
    fn strips_the_root_dot_from_relay_hosts() {
        let fqdn = "https://euc1-1.relay.n0.iroh.link./".parse::<iroh::RelayUrl>();
        assert_eq!(
            fqdn.ok().map(|url| without_root_dot(&url).to_string()),
            Some(String::from("https://euc1-1.relay.n0.iroh.link/"))
        );
        let plain = "https://relay.example.com/".parse::<iroh::RelayUrl>();
        assert_eq!(
            plain.ok().map(|url| without_root_dot(&url).to_string()),
            Some(String::from("https://relay.example.com/"))
        );
    }
}
