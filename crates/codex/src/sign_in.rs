use agents::{Failure, HomeKind, Pick, SignIn, Word};

use crate::login;
use crate::prelude::*;

#[must_use]
pub fn sign_in() -> SignIn {
    SignIn {
        run: login(HomeKind::Private),
        link: Pick {
            after: String::from("Open this link"),
            word: Word::Link,
        },
        code: Some(Pick {
            after: String::from("one-time code"),
            word: Word::Code,
        }),
        paste: false,
        failures: Vec::from([
            Failure {
                says: String::from("device code login is not enabled"),
                means: String::from(
                    "Open ChatGPT Settings ? Security, enable device code authorization for Codex, then try again. If the setting is unavailable, you can use an OpenAI API key on this host.",
                ),
            },
            Failure {
                says: String::from("timed out"),
                means: String::from(
                    "The one-time code expired before the sign-in finished. Sign in again for a new one.",
                ),
            },
        ]),
    }
}

#[cfg(test)]
mod tests {
    use agents::{HomeKind, Shown, plain};

    use super::sign_in;

    #[test]
    fn the_link_and_the_code_are_read_through_the_colours() {
        let printed = include_str!("../../../fixtures/codex/sign-in.txt")
            .lines()
            .map(plain)
            .collect::<Vec<String>>();
        let flow = sign_in();
        assert_eq!(
            (flow.shown(&printed), flow.paste, flow.run),
            (
                Some(Shown {
                    link: String::from("https://auth.openai.com/codex/device"),
                    code: Some(String::from("ABCD-12345")),
                }),
                false,
                crate::login(HomeKind::Private),
            )
        );
    }

    #[test]
    fn its_failures_are_known_by_what_the_pinned_cli_says() {
        let flow = sign_in();
        assert!(
            flow.failure("device code login is not enabled")
                .is_some_and(|reason| {
                    reason.contains("ChatGPT Settings ? Security")
                        && reason.contains("OpenAI API key")
                })
        );
        assert_eq!(
            [
                "Error logging in with device code: device code login is not enabled for this Codex server. Use the browser login or verify the server URL.",
                "Error logging in with device code: device auth timed out after 15 minutes",
                "Error logging in: something else",
            ]
            .map(|line| flow.failure(line).is_some()),
            [true, true, false]
        );
    }
}
