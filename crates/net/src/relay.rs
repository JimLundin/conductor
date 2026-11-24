use iroh::RelayUrl;
use url::Url;

/// n0's public relays, named without the root dot ([`without_root_dot`]).
///
/// Every endpoint, host included, uses this map: a host advertises its home
/// relay to peers, and a browser that dials the dotted name fails `WebKit`'s
/// certificate check.
#[must_use]
pub fn n0_relays() -> iroh::RelayMap {
    iroh::defaults::prod::default_relay_map()
        .urls::<Vec<RelayUrl>>()
        .iter()
        .map(without_root_dot)
        .collect()
}

/// The relay URL with its host named without the root dot.
///
/// iroh names its relays as fully qualified hosts ending in `.`. `WebKit`
/// (Safari) checks the TLS certificate against that literal name and refuses
/// it, so every relay URL we configure or dial goes through here. DNS treats
/// both forms alike.
#[must_use]
pub fn without_root_dot(url: &RelayUrl) -> RelayUrl {
    let plain = Url::from(url.clone());
    plain
        .host_str()
        .and_then(|dotted| dotted.strip_suffix('.'))
        .map(str::to_owned)
        .and_then(|host| {
            let mut undotted = plain.clone();
            undotted.set_host(Some(&host)).ok().map(|()| undotted)
        })
        .map_or_else(|| url.clone(), RelayUrl::from)
}

#[cfg(test)]
mod tests {
    use iroh::RelayUrl;

    use super::without_root_dot;

    #[test]
    fn strips_the_root_dot_from_relay_hosts()
    -> Result<(), Box<dyn std::error::Error>> {
        let fqdn = "https://euc1-1.relay.n0.iroh.link./".parse::<RelayUrl>()?;
        assert_eq!(
            without_root_dot(&fqdn).to_string(),
            "https://euc1-1.relay.n0.iroh.link/"
        );
        let plain = "https://relay.example.com/".parse::<RelayUrl>()?;
        assert_eq!(
            without_root_dot(&plain).to_string(),
            "https://relay.example.com/"
        );
        Ok(())
    }
}
