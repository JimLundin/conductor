//! Claude Code's subscription sign-in from the app, `claude auth login
//! --claudeai`, as data. On pipes it prints the URL to open after
//! `visit:`; the page it opens shows a code, which the CLI reads as a line
//! on stdin (`PROTOCOL.md`, observed with 2.1.316).

use agents::{Failure, HomeKind, Invocation, Pick, SignIn, Var, Word, plain};

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

const AUTHORIZATION: &str = "https://claude.com/cai/oauth/authorize?";

#[must_use]
pub fn setup_token_run() -> Invocation {
    let mut env = crate::cli(HomeKind::Private).env;
    env.extend(
        [
            "HOME",
            "USERPROFILE",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
            "APPDATA",
            "LOCALAPPDATA",
            "TMPDIR",
            "TEMP",
            "TMP",
        ]
        .map(|name| Var::Home(String::from(name))),
    );
    env.extend([
        Var::set("BROWSER", "false"),
        Var::set("TERM", "xterm-256color"),
    ]);
    Invocation::of(env, &["setup-token"])
}

#[must_use]
pub fn setup_token_link(line: &str) -> Option<String> {
    let text = plain(line);
    text.split_ascii_whitespace().find_map(|word| {
        let query = word.strip_prefix(AUTHORIZATION)?;
        let parameters = |key: &str| {
            let mut values = query.split('&').filter_map(|part| {
                let (name, value) = part.split_once('=')?;
                (name == key).then_some(value)
            });
            let value = values.next()?;
            (!value.is_empty() && values.next().is_none()).then_some(value)
        };
        (query.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || b"-._~%+&=".contains(&byte)
        }) && query.split('&').all(|part| {
            part.split_once('=').is_some_and(|(name, _)| {
                !name.is_empty()
                    && name.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'_')
            })
        }) && parameters("response_type") == Some("code")
            && parameters("code_challenge_method") == Some("S256")
            && parameters("code_challenge").is_some()
            && parameters("state").is_some()
            && parameters("redirect_uri")
                == Some("https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback"))
        .then(|| String::from(word))
    })
}

#[must_use]
pub fn setup_token_code(input: &str) -> Option<&str> {
    let code = input.trim();
    (!code.is_empty()
        && code.len() <= 1024
        && code.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || b"-_#=+/.%~".contains(&byte)
        }))
    .then_some(code)
}

#[derive(Default)]
pub struct SetupTokenOutput {
    succeeded: bool,
}

impl SetupTokenOutput {
    /// Pass complete terminal lines so the end of a read cannot truncate the credential.
    #[must_use]
    pub fn line(&mut self, line: &str) -> Option<String> {
        let text = plain(line);
        self.succeeded |= text
            .contains("Long-lived authentication token created successfully!");
        if !self.succeeded {
            return None;
        }
        text.split_ascii_whitespace().find_map(|word| {
            let (version, secret) =
                word.strip_prefix("sk-ant-oat")?.split_once('-')?;
            (!version.is_empty()
                && version.bytes().all(|byte| byte.is_ascii_digit())
                && secret.len() >= 20
                && secret.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || b"-_".contains(&byte)
                }))
            .then(|| String::from(word))
        })
    }
}

#[cfg(test)]
mod tests {
    use agents::{HomeKind, Shown, plain};

    use super::{
        SetupTokenOutput, setup_token_code, setup_token_link, sign_in,
    };

    const AUTH_URL: &str = "https://claude.com/cai/oauth/authorize?response_type=code&code_challenge_method=S256&code_challenge=challenge&state=state&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback";

    #[test]
    fn setup_token_publishes_the_first_party_manual_authorization_link() {
        let rendered = format!("Visit: \x1b[1m{AUTH_URL}\x1b[0m");
        assert!(setup_token_link(&rendered).is_some());
        assert!(
            setup_token_link(
                &AUTH_URL.replace(
                    "claude.com/cai",
                    "claude.com@malicious.example/cai"
                )
            )
            .is_none()
        );
        assert!(
            setup_token_link(
                &AUTH_URL.replace("platform.claude.com", "malicious.example")
            )
            .is_none()
        );
        assert!(
            setup_token_link(
                AUTH_URL.split("&state=").next().unwrap_or_default()
            )
            .is_none()
        );
    }

    #[test]
    fn setup_token_requires_success_and_accepts_only_terminal_text_as_code() {
        let mut output = SetupTokenOutput::default();
        let printed = "\x1b[32msk-ant-oat01-synthetic-credential-000\x1b[0m";
        assert!(output.line(printed).is_none());
        assert!(
            output
                .line("Long-lived authentication token created successfully!")
                .is_none()
        );
        assert!(output.line("sk-ant-oat01-partial").is_none());
        assert!(output.line(printed).is_some());
        assert!(setup_token_code(" code#state ").is_some());
        assert!(setup_token_code("code\rstate").is_none());
        assert!(setup_token_code("code\x1b[1m").is_none());
    }

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
