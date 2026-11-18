//! Claude Code's subscription sign-in from the app, `claude auth login
//! --claudeai`, as data. On pipes it prints the URL to open after
//! `visit:`; the page it opens shows a code, which the CLI reads as a line
//! on stdin (`PROTOCOL.md`, observed with 2.1.316).

use agents::{Failure, HomeKind, Pick, SignIn, Word};

use crate::login;
use crate::prelude::*;

#[must_use]
pub fn sign_in() -> SignIn {
    SignIn {
        run: login(HomeKind::Private),
        link: Pick {
            after: String::from("visit:"),
            word: Word::Link,
        },
        code: None,
        paste: true,
        failures: Vec::from([Failure {
            says: String::from("code wasn't accepted"),
            means: String::from(
                "Claude didn't accept that code. Sign in again to get a new one.",
            ),
        }]),
    }
}

#[cfg(test)]
mod tests {
    use agents::{HomeKind, Shown, plain};

    use super::sign_in;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(plain).collect()
    }

    #[test]
    fn the_link_is_read_from_what_the_cli_prints_and_a_code_goes_back() {
        let printed =
            lines(include_str!("../../../fixtures/claude/sign-in.txt"));
        let flow = sign_in();
        assert_eq!(
            (
                flow.shown(&printed),
                flow.shown(printed.get(..1).unwrap_or_default()),
                flow.paste,
                flow.run,
            ),
            (
                Some(Shown {
                    link: String::from(
                        "https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=org%3Acreate_api_key+user%3Aprofile+user%3Ainference+user%3Asessions%3Aclaude_code+user%3Amcp_servers+user%3Afile_upload+user%3Aplugins&code_challenge=CHALLENGE&code_challenge_method=S256&state=STATE"
                    ),
                    code: None,
                }),
                None,
                true,
                crate::login(HomeKind::Private),
            )
        );
    }

    #[test]
    fn a_refused_code_says_so() {
        let refused =
            lines(include_str!("../../../fixtures/claude/sign-in-refused.txt"));
        let flow = sign_in();
        assert_eq!(
            refused
                .iter()
                .find_map(|line| flow.failure(line))
                .map(String::from),
            Some(String::from(
                "Claude didn't accept that code. Sign in again to get a new one."
            ))
        );
    }
}
