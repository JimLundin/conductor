//! The name a client gives in `Hello`: the kind of device it runs on, so a
//! resolved card can say which device approved.

use proto::ClientName;

/// Whether the device has a touchscreen, which tells an iPad that asks for
/// desktop sites from a Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Touch {
    None,
    Screen,
}

/// What a client knows about where it runs, read at the platform boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Platform {
    Browser {
        user_agent: String,
        touch: Touch,
    },
    /// `std::env::consts::OS`.
    Native {
        os: String,
    },
}

/// A word in a user agent, and the device it names without and with a
/// touchscreen.
struct Row {
    marker: &'static str,
    name: &'static str,
    touch_name: &'static str,
}

/// Checked in this order: an iPhone's or iPad's user agent also says "Mac
/// OS X", and Android's and ChromeOS's also say "Linux".
const DEVICES: &[Row] = &[
    Row {
        marker: "iPhone",
        name: "iPhone",
        touch_name: "iPhone",
    },
    Row {
        marker: "iPad",
        name: "iPad",
        touch_name: "iPad",
    },
    Row {
        marker: "Android",
        name: "Android",
        touch_name: "Android",
    },
    Row {
        marker: "CrOS",
        name: "ChromeOS",
        touch_name: "ChromeOS",
    },
    Row {
        marker: "Windows",
        name: "Windows",
        touch_name: "Windows",
    },
    Row {
        marker: "Macintosh",
        name: "Mac",
        touch_name: "iPad",
    },
    Row {
        marker: "Linux",
        name: "Linux",
        touch_name: "Linux",
    },
];

/// A native client is named by its OS as a browser on it would be, and an
/// OS with no such name keeps its own. iPadOS Safari sends a Mac's user
/// agent, so a "Mac" with a touchscreen is an iPad. A user agent that names
/// no known device is a "Browser".
pub(crate) fn device_name(platform: Platform) -> ClientName {
    match platform {
        Platform::Browser { user_agent, touch } => {
            let name = DEVICES
                .iter()
                .find(|row| user_agent.contains(row.marker))
                .map_or("Browser", |row| match touch {
                    Touch::None => row.name,
                    Touch::Screen => row.touch_name,
                });
            ClientName(String::from(name))
        }
        Platform::Native { os } => ClientName(match os.as_str() {
            "windows" => String::from("Windows"),
            "macos" => String::from("Mac"),
            "linux" => String::from("Linux"),
            _ => os,
        }),
    }
}

#[cfg(test)]
mod tests {
    use proto::ClientName;

    use super::{Platform, Touch, device_name};

    fn native(os: &str) -> ClientName {
        device_name(Platform::Native {
            os: String::from(os),
        })
    }

    #[test]
    fn a_native_client_is_named_by_its_os() {
        assert_eq!(native("windows"), ClientName(String::from("Windows")));
        assert_eq!(native("macos"), ClientName(String::from("Mac")));
        assert_eq!(native("freebsd"), ClientName(String::from("freebsd")));
    }

    /// A browser's user agent and touchscreen, and the name it should give.
    struct Case {
        user_agent: String,
        touch: Touch,
        name: String,
    }

    fn case(user_agent: &str, touch: Touch, name: &str) -> Case {
        Case {
            user_agent: String::from(user_agent),
            touch,
            name: String::from(name),
        }
    }

    #[test]
    fn a_user_agent_names_its_device() {
        let cases = Vec::from([
            case(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1",
                Touch::Screen,
                "iPhone",
            ),
            case(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148",
                Touch::Screen,
                "iPhone",
            ),
            case(
                "Mozilla/5.0 (iPad; CPU OS 12_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/12.1 Mobile/15E148 Safari/604.1",
                Touch::Screen,
                "iPad",
            ),
            case(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15",
                Touch::Screen,
                "iPad",
            ),
            case(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Safari/605.1.15",
                Touch::None,
                "Mac",
            ),
            case(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36 Edg/142.0.0.0",
                Touch::None,
                "Windows",
            ),
            case(
                "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Mobile Safari/537.36",
                Touch::Screen,
                "Android",
            ),
            case(
                "Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36",
                Touch::None,
                "ChromeOS",
            ),
            case(
                "Mozilla/5.0 (X11; Linux x86_64; rv:145.0) Gecko/20100101 Firefox/145.0",
                Touch::None,
                "Linux",
            ),
            case("", Touch::None, "Browser"),
        ]);
        let (got, want) = cases
            .into_iter()
            .map(|case| {
                (
                    device_name(Platform::Browser {
                        user_agent: case.user_agent,
                        touch: case.touch,
                    }),
                    ClientName(case.name),
                )
            })
            .unzip::<ClientName, ClientName, Vec<ClientName>, Vec<ClientName>>();
        assert_eq!(got, want);
    }
}
