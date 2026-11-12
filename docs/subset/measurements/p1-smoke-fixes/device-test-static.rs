#[cfg(test)]
mod tests {
    use proto::ClientName;

    use super::{Touch, device_name};

    /// A browser's user agent and touchscreen, and the name it should give.
    struct Case {
        user_agent: &'static str,
        touch: Touch,
        name: &'static str,
    }

    #[test]
    fn a_user_agent_names_its_device() {
        let cases = Vec::from([
            Case { user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1", touch: Touch::Screen, name: "iPhone" },
            Case { user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148", touch: Touch::Screen, name: "iPhone" },
            Case { user_agent: "Mozilla/5.0 (iPad; CPU OS 12_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/12.1 Mobile/15E148 Safari/604.1", touch: Touch::Screen, name: "iPad" },
            Case { user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15", touch: Touch::Screen, name: "iPad" },
            Case { user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15", touch: Touch::None, name: "Mac" },
            Case { user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36 Edg/142.0.0.0", touch: Touch::None, name: "Windows" },
            Case { user_agent: "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Mobile Safari/537.36", touch: Touch::Screen, name: "Android" },
            Case { user_agent: "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36", touch: Touch::None, name: "ChromeOS" },
            Case { user_agent: "Mozilla/5.0 (X11; Linux x86_64; rv:145.0) Gecko/20100101 Firefox/145.0", touch: Touch::None, name: "Linux" },
            Case { user_agent: "", touch: Touch::None, name: "Browser" },
        ]);
        let (got, want) = cases
            .into_iter()
            .map(|case| {
                (
                    device_name(case.user_agent, case.touch),
                    ClientName(String::from(case.name)),
                )
            })
            .unzip::<ClientName, ClientName, Vec<ClientName>, Vec<ClientName>>();
        assert_eq!(got, want);
    }
}
