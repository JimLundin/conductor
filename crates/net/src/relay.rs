use iroh_base::RelayUrl;
use url::Url;

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
    use iroh_base::RelayUrl;

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
