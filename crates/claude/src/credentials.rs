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

/// An instance's credential as Claude's, if it is of a kind Claude takes.
#[must_use]
pub fn credential(credential: Option<&Credential>) -> Option<ClaudeCredential> {
    credential.and_then(read)
}

fn read(credential: &Credential) -> Option<ClaudeCredential> {
    (credential.kind == GATEWAY).then(|| ClaudeCredential::Gateway {
        base_url: String::from(credential.value(BASE_URL)),
        token: String::from(credential.value(TOKEN)),
    })
}

#[cfg(test)]
mod tests {
    use proto::Credential;

    use super::{ClaudeCredential, credential, info};

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
                credential(Some(&stored)),
                credential(Some(&gateway("key"))),
                credential(None),
                info().check(&stored),
            ),
            (
                Some(ClaudeCredential::Gateway {
                    base_url: String::from("https://g.example"),
                    token: String::from("t"),
                }),
                None,
                None,
                Ok(()),
            )
        );
    }
}
