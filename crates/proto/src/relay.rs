use iroh_base::RelayUrl;

/// iroh names its relays as fully qualified hosts ending in `.`. WebKit
/// (Safari) checks the TLS certificate against that literal name and refuses
/// it, so every relay URL we configure or dial goes through here. DNS treats
/// both forms alike.
pub fn without_root_dot(url: &RelayUrl) -> RelayUrl {
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
        let fqdn: iroh_base::RelayUrl = "https://euc1-1.relay.n0.iroh.link./".parse().unwrap();
        assert_eq!(
            without_root_dot(&fqdn).to_string(),
            "https://euc1-1.relay.n0.iroh.link/"
        );
        let plain: iroh_base::RelayUrl = "https://relay.example.com/".parse().unwrap();
        assert_eq!(
            without_root_dot(&plain).to_string(),
            "https://relay.example.com/"
        );
    }
}
