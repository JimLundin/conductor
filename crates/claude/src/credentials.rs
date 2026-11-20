//! The credentials Claude Code takes, as the forms the app shows, and what
//! their values become.

use proto::{Credential, CredentialForm, Field, FieldKind, ProviderInfo};

use crate::prelude::*;

const GATEWAY: &str = "gateway";
const BASE_URL: &str = "base_url";
const TOKEN: &str = "token";

/// Claude's credential, read from its form's values. Its `Debug` shows
/// only the kind, so no value reaches a log.
#[derive(Clone, PartialEq, Eq)]
pub enum ClaudeCredential {
    /// Claude Code's `ANTHROPIC_BASE_URL` and `ANTHROPIC_AUTH_TOKEN`.
    Gateway { base_url: String, token: String },
}

impl core::fmt::Debug for ClaudeCredential {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Gateway { .. } => write!(f, "Gateway"),
        }
    }
}

/// How Claude presents itself and the credentials it takes.
#[must_use]
pub fn info() -> ProviderInfo {
    ProviderInfo {
        label: String::from("Claude Code"),
        note: None,
        credentials: Vec::from([CredentialForm {
            kind: String::from(GATEWAY),
            label: String::from("Gateway"),
            fields: Vec::from([
                field(
                    BASE_URL,
                    "Gateway URL",
                    FieldKind::Url,
                    "https://gateway.example.com",
                ),
                field(TOKEN, "Token", FieldKind::Secret, ""),
            ]),
            account: String::from(BASE_URL),
        }]),
    }
}

fn field(id: &str, label: &str, kind: FieldKind, placeholder: &str) -> Field {
    Field {
        id: String::from(id),
        label: String::from(label),
        kind,
        placeholder: String::from(placeholder),
        required: true,
    }
}

/// The account a signed-in home is, as `claude auth status` prints it:
/// the email of a Claude subscription. A gateway's or an API key's
/// sign-in names none, so its credential's form does.
#[must_use]
pub fn account(status: &str) -> Option<String> {
    let printed = serde_json::from_str::<serde_json::Value>(status).ok()?;
    printed
        .get("email")
        .and_then(serde_json::Value::as_str)
        .filter(|email| !email.is_empty())
        .map(String::from)
}

/// Reads a stored credential using this provider's form.
///
/// # Errors
/// The form is unknown or its values are invalid.
pub fn credential(credential: &Credential) -> Result<ClaudeCredential, String> {
    info().check(credential)?;
    Ok(ClaudeCredential::Gateway {
        base_url: String::from(credential.value(BASE_URL)),
        token: String::from(credential.value(TOKEN)),
    })
}

#[cfg(test)]
mod tests {
    use proto::Credential;

    use super::{ClaudeCredential, account, credential, info};

    /// `claude auth status` with an API key and signed out, as 2.1.316
    /// printed them, and with a subscription, as its binary writes it.
    #[test]
    fn a_subscription_s_account_is_its_email_and_a_key_names_none() {
        assert_eq!(
            [
                r#"{"loggedIn": true, "authMethod": "claude.ai", "apiProvider": "firstParty", "email": "me@example.com", "orgId": "o", "orgName": "Me", "subscriptionType": "max"}"#,
                r#"{"loggedIn": true, "authMethod": "api_key", "apiProvider": "firstParty", "apiKeySource": "ANTHROPIC_API_KEY"}"#,
                r#"{"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty"}"#,
                "",
            ]
            .map(account),
            [Some(String::from("me@example.com")), None, None, None]
        );
    }

    fn gateway(kind: &str) -> Credential {
        Credential {
            kind: String::from(kind),
            values: [("base_url", "https://g.example"), ("token", "t")]
                .map(|(id, value)| (String::from(id), String::from(value)))
                .into(),
        }
    }

    #[test]
    fn the_gateway_form_s_values_are_claude_s_gateway() {
        let stored = gateway("gateway");
        assert_eq!(
            (
                credential(&stored),
                credential(&gateway("key")),
                info().check(&stored),
            ),
            (
                Ok(ClaudeCredential::Gateway {
                    base_url: String::from("https://g.example"),
                    token: String::from("t"),
                }),
                Err(String::from(r#"Claude Code takes no "key" credential"#)),
                Ok(()),
            )
        );
    }
    #[test]
    fn a_credential_shows_only_its_kind_when_logged() {
        let gateway = ClaudeCredential::Gateway {
            base_url: String::from("https://g.example"),
            token: String::from("secret-token"),
        };
        assert_eq!(format!("{gateway:?}"), "Gateway");
    }
}
