//! The one thing that differs per target: how the iroh endpoint is built.

#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(target_family = "wasm")]
mod web;

#[cfg(not(target_family = "wasm"))]
pub use native::bind_endpoint;
#[cfg(target_family = "wasm")]
pub use web::bind_endpoint;

/// iroh names its relays as fully qualified hosts ending in `.`. WebKit (Safari)
/// checks the TLS certificate against that literal name and refuses it, so
/// every relay URL a client dials goes through here. DNS treats both forms alike.
pub(crate) fn without_root_dot(url: &iroh::RelayUrl) -> iroh::RelayUrl {
    let mut plain = (**url).clone();
    if let Some(host) = plain.host_str().and_then(|h| h.strip_suffix('.')) {
        let host = host.to_owned();
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
        let fqdn: iroh::RelayUrl = "https://euc1-1.relay.n0.iroh.link./".parse().unwrap();
        assert_eq!(
            without_root_dot(&fqdn).to_string(),
            "https://euc1-1.relay.n0.iroh.link/"
        );
        let plain: iroh::RelayUrl = "https://relay.example.com/".parse().unwrap();
        assert_eq!(
            without_root_dot(&plain).to_string(),
            "https://relay.example.com/"
        );
    }
}
