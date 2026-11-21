//! The credentials Codex takes, as the forms the app shows, and what their
//! values become.

use proto::{
    Credential, CredentialForm, Field, FieldKind, ModelId, ProviderInfo,
};

use crate::prelude::*;

const PROVIDER: &str = "provider";
const NAME: &str = "name";
const BASE_URL: &str = "base_url";
const MODEL: &str = "model";
const KEY: &str = "key";

const API_KEY: &str = "api_key";
const ACCOUNT_LABEL: &str = "account_label";

/// Codex's credential, read from its form's values. Its `Debug` shows
/// only the kind, so no value reaches a log.
#[derive(Clone, PartialEq, Eq)]
pub enum CodexCredential {
    ApiKey {
        key: String,
    },
    /// A custom model provider: Codex's `[model_providers.<id>]` with
    /// `name` and `base_url`, selected with `model`. Its `wire_api` is
    /// `responses`, the only one Codex 0.166 knows. The key reaches Codex in
    /// the environment variable the section's `env_key` names, never in its
    /// config.
    Provider {
        name: String,
        base_url: String,
        key: String,
        model: String,
    },
}

impl core::fmt::Debug for CodexCredential {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ApiKey { .. } => f.write_str("ApiKey"),
            Self::Provider { .. } => write!(f, "Provider"),
        }
    }
}

/// How Codex presents itself and the credentials it takes.
///
/// Its sandbox (`WorkspaceWrite`, asking `OnRequest`) lets it run commands
/// and write in the session's directory and `/tmp` unasked, which a new
/// session's form says.
#[must_use]
pub fn info() -> ProviderInfo {
    ProviderInfo {
        label: String::from("Codex"),
        note: Some(String::from(
            "Codex runs commands and edits files in this folder and in /tmp without asking. It asks before writing anywhere else, changing .git, or using the network.",
        )),
        credentials: Vec::from([
            CredentialForm {
                kind: String::from(PROVIDER),
                label: String::from("Model provider"),
                fields: Vec::from([
                    field(NAME, "Provider name", FieldKind::Text, "Gateway"),
                    field(
                        BASE_URL,
                        "Base URL",
                        FieldKind::Url,
                        "https://gateway.example.com/v1",
                    ),
                    field(MODEL, "Model", FieldKind::Text, "gpt-6"),
                    field(KEY, "Key", FieldKind::Secret, ""),
                ]),
                account: String::from(BASE_URL),
            },
            CredentialForm {
                kind: String::from(API_KEY),
                label: String::from("OpenAI API key"),
                fields: Vec::from([
                    field(
                        ACCOUNT_LABEL,
                        "Public account label",
                        FieldKind::Text,
                        "Work",
                    ),
                    field(KEY, "API key", FieldKind::Secret, ""),
                ]),
                account: String::from(ACCOUNT_LABEL),
            },
        ]),
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

/// The account a signed-in home is, as its `auth.json` keeps it: the
/// email in the ID token of a `ChatGPT` sign-in, else its account id. An
/// API key's sign-in names none, so a model provider's form does.
#[must_use]
pub fn account(auth: &str) -> Option<String> {
    let kept = serde_json::from_str::<serde_json::Value>(auth).ok()?;
    let tokens = kept.get("tokens")?;
    let reported_email = tokens
        .get("id_token")
        .and_then(serde_json::Value::as_str)
        .and_then(|token| token.split('.').nth(1))
        .and_then(|claims| {
            data_encoding::BASE64URL_NOPAD
                .decode(claims.trim_end_matches('=').as_bytes())
                .ok()
        })
        .and_then(|claims| {
            serde_json::from_slice::<serde_json::Value>(&claims).ok()
        })
        .and_then(|claims| {
            claims
                .get("email")
                .and_then(serde_json::Value::as_str)
                .map(String::from)
        });
    reported_email
        .filter(|email| !email.is_empty())
        .or_else(|| {
            tokens
                .get("account_id")
                .and_then(serde_json::Value::as_str)
                .map(String::from)
        })
        .filter(|account| !account.is_empty())
}

/// Reads a stored credential using this provider's form.
///
/// # Errors
/// The form is unknown or its values are invalid.
pub fn credential(credential: &Credential) -> Result<CodexCredential, String> {
    info().check(credential)?;
    if credential.kind == API_KEY {
        return Ok(CodexCredential::ApiKey {
            key: String::from(credential.value(KEY)),
        });
    }
    Ok(CodexCredential::Provider {
        name: String::from(credential.value(NAME)),
        base_url: String::from(credential.value(BASE_URL)),
        key: String::from(credential.value(KEY)),
        model: String::from(credential.value(MODEL)),
    })
}

/// The model a credential makes the default: a model provider's gateway
/// serves the one it names.
#[must_use]
pub fn default_model(credential: &CodexCredential) -> Option<ModelId> {
    match credential {
        CodexCredential::Provider { model, .. } => Some(ModelId(model.clone())),
        CodexCredential::ApiKey { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use proto::Credential;

    use super::{CodexCredential, account, credential, info};

    fn provider(kind: &str) -> Credential {
        Credential {
            kind: String::from(kind),
            values: [
                ("name", "Gateway"),
                ("base_url", "https://g.example/v1"),
                ("model", "gpt-6"),
                ("key", "k"),
            ]
            .map(|(id, value)| (String::from(id), String::from(value)))
            .into(),
        }
    }

    #[test]
    fn only_a_populated_email_takes_precedence_over_the_account_id() {
        for (claims, expected) in [
            (r#"{"email":""}"#, "acct-1"),
            ("{}", "acct-1"),
            (r#"{"email":"owner@example.com"}"#, "owner@example.com"),
        ] {
            let encoded_claims =
                data_encoding::BASE64URL_NOPAD.encode(claims.as_bytes());
            let auth = format!(
                r#"{{"tokens":{{"id_token":"e30.{encoded_claims}.sig","account_id":"acct-1"}}}}"#
            );
            assert_eq!(account(&auth).as_deref(), Some(expected));
        }
    }

    #[test]
    fn a_chatgpt_sign_in_is_its_id_token_s_email_else_its_account_id() {
        let claims = data_encoding::BASE64URL_NOPAD
            .encode(br#"{"email":"me@example.com","sub":"x"}"#);
        let signed = format!(
            r#"{{"tokens":{{"id_token":"e30.{claims}.sig","account_id":"acct-1"}}}}"#
        );
        assert_eq!(
            [
                signed.as_str(),
                r#"{"tokens":{"id_token":"bad","account_id":"acct-1"}}"#,
                r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-x"}"#,
                "not json",
            ]
            .map(account),
            [
                Some(String::from("me@example.com")),
                Some(String::from("acct-1")),
                None,
                None,
            ]
        );
    }

    #[test]
    fn the_provider_form_s_values_are_codex_s_model_provider() {
        let stored = provider("provider");
        assert_eq!(
            (
                credential(&stored),
                credential(&provider("gateway")),
                info().check(&stored),
            ),
            (
                Ok(CodexCredential::Provider {
                    name: String::from("Gateway"),
                    base_url: String::from("https://g.example/v1"),
                    key: String::from("k"),
                    model: String::from("gpt-6"),
                }),
                Err(String::from(r#"Codex takes no "gateway" credential"#)),
                Ok(()),
            )
        );
    }
    #[test]
    fn a_direct_key_names_the_public_label_and_keeps_the_key_private() {
        let stored = Credential {
            kind: String::from("api_key"),
            values: [("account_label", "Work"), ("key", "synthetic-api-key")]
                .map(|(id, value)| (String::from(id), String::from(value)))
                .into(),
        };
        assert_eq!(
            credential(&stored),
            Ok(CodexCredential::ApiKey {
                key: String::from("synthetic-api-key")
            })
        );
        assert_eq!(
            info().account(&stored),
            Ok(proto::Account::Named(String::from("Work")))
        );
        assert_eq!(
            info().auth(&stored),
            proto::Auth::Stored {
                kind: String::from("api_key"),
                url: None
            }
        );
        assert_eq!(format!("{:?}", credential(&stored)), "Ok(ApiKey)");
    }

    #[test]
    fn a_credential_shows_only_its_kind_when_logged() {
        let provider = CodexCredential::Provider {
            name: String::from("Gateway"),
            base_url: String::from("https://g.example/v1"),
            key: String::from("secret-key"),
            model: String::from("gpt-6"),
        };
        assert_eq!(format!("{provider:?}"), "Provider");
    }
}
