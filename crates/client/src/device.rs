//! The name a browser gives in `Hello`: the kind of device it runs on, so
//! a resolved card can say which device approved.

use proto::ClientName;

/// Whether the device has a touchscreen, which tells an iPad that asks for
/// desktop sites from a Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Touch {
    None,
    Screen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Device {
    IPhone,
    IPad,
    Android,
    ChromeOs,
    Windows,
    Mac,
    Linux,
}

/// Checked in this order: an iPhone's or iPad's user agent also says "Mac
/// OS X", and Android's and ChromeOS's also say "Linux".
const DEVICES: &[Device] = &[
    Device::IPhone,
    Device::IPad,
    Device::Android,
    Device::ChromeOs,
    Device::Windows,
    Device::Mac,
    Device::Linux,
];

impl Device {
    /// The word in a user agent that names the device.
    fn marker(self) -> String {
        String::from(match self {
            Device::IPhone => "iPhone",
            Device::IPad => "iPad",
            Device::Android => "Android",
            Device::ChromeOs => "CrOS",
            Device::Windows => "Windows",
            Device::Mac => "Macintosh",
            Device::Linux => "Linux",
        })
    }

    fn name(self) -> String {
        String::from(match self {
            Device::IPhone => "iPhone",
            Device::IPad => "iPad",
            Device::Android => "Android",
            Device::ChromeOs => "ChromeOS",
            Device::Windows => "Windows",
            Device::Mac => "Mac",
            Device::Linux => "Linux",
        })
    }
}

/// iPadOS Safari sends a Mac's user agent, so a "Mac" with a touchscreen is
/// an iPad. An agent that names no known device is a "Browser".
pub fn device_name(user_agent: &str, touch: Touch) -> ClientName {
    let found = DEVICES
        .iter()
        .copied()
        .find(|device| user_agent.contains(&device.marker()));
    let device = match (found, touch) {
        (Some(Device::Mac), Touch::Screen) => Some(Device::IPad),
        (found, Touch::None | Touch::Screen) => found,
    };
    ClientName(device.map_or_else(|| String::from("Browser"), Device::name))
}

#[cfg(test)]
mod tests {
    use proto::ClientName;

    use super::{Touch, device_name};

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
                    device_name(&case.user_agent, case.touch),
                    ClientName(case.name),
                )
            })
            .unzip::<ClientName, ClientName, Vec<ClientName>, Vec<ClientName>>();
        assert_eq!(got, want);
    }
}
