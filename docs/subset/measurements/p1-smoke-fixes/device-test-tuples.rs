#[cfg(test)]
mod tests {
    use proto::ClientName;

    use super::{Touch, device_name};

    #[test]
    fn a_user_agent_names_its_device() {
        let (got, want) = Vec::from([
            ("Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1", Touch::Screen, "iPhone"),
            ("Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148", Touch::Screen, "iPhone"),
            ("Mozilla/5.0 (iPad; CPU OS 12_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/12.1 Mobile/15E148 Safari/604.1", Touch::Screen, "iPad"),
            ("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15", Touch::Screen, "iPad"),
            ("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15", Touch::None, "Mac"),
            ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36 Edg/142.0.0.0", Touch::None, "Windows"),
            ("Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Mobile Safari/537.36", Touch::Screen, "Android"),
            ("Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36", Touch::None, "ChromeOS"),
            ("Mozilla/5.0 (X11; Linux x86_64; rv:145.0) Gecko/20100101 Firefox/145.0", Touch::None, "Linux"),
            ("", Touch::None, "Browser"),
        ])
        .into_iter()
        .map(|(user_agent, touch, name)| {
            (
                device_name(user_agent, touch),
                ClientName(String::from(name)),
            )
        })
            .unzip::<ClientName, ClientName, Vec<ClientName>, Vec<ClientName>>();
        assert_eq!(got, want);
    }
}
