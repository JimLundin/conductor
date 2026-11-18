//! The credentials Codex takes, as the forms the app shows, and what their
//! values become.

use proto::{
    Credential, CredentialForm, Credentials, Field, FieldKind, ModelId,
    Provider, ProviderInfo,
};

use crate::prelude::*;

const PROVIDER: &str = "provider";
const NAME: &str = "name";
const BASE_URL: &str = "base_url";
const MODEL: &str = "model";
const KEY: &str = "key";

/// Codex's credential, read from its form's values. Its `Debug` shows
/// only the kind, so no value reaches a log.
#[derive(Clone, PartialEq, Eq)]
pub enum CodexCredential {
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
        credentials: Vec::from([CredentialForm {
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

/// Codex's credential among the host's, if it is of a kind Codex takes.
#[must_use]
pub fn credential(credentials: &Credentials) -> Option<CodexCredential> {
    credentials.of(Provider::Codex).and_then(read)
}

fn read(credential: &Credential) -> Option<CodexCredential> {
    (credential.kind == PROVIDER).then(|| CodexCredential::Provider {
        name: String::from(credential.value(NAME)),
        base_url: String::from(credential.value(BASE_URL)),
        key: String::from(credential.value(KEY)),
        model: String::from(credential.value(MODEL)),
    })
}

/// The model a credential makes the default: a model provider's gateway
/// serves the one it names.
#[must_use]
pub fn default_model(credential: &CodexCredential) -> ModelId {
    match credential {
        CodexCredential::Provider { model, .. } => ModelId(model.clone()),
    }
}

#[cfg(test)]
mod tests {
    use proto::{Credential, Credentials, Provider};

    use super::{CodexCredential, credential, info};

    fn provider(kind: &str) -> Credentials {
        Credentials::none().set(Credential {
            provider: Provider::Codex,
            kind: String::from(kind),
            values: [
                ("name", "Gateway"),
                ("base_url", "https://g.example/v1"),
                ("model", "gpt-6"),
                ("key", "k"),
            ]
            .map(|(id, value)| (String::from(id), String::from(value)))
            .into(),
        })
    }

    #[test]
    fn the_provider_form_s_values_are_codex_s_model_provider() {
        let stored = provider("provider");
        assert_eq!(
            (
                credential(&stored),
                credential(&provider("gateway")),
                stored.of(Provider::Codex).map(|it| info().check(it)),
            ),
            (
                Some(CodexCredential::Provider {
                    name: String::from("Gateway"),
                    base_url: String::from("https://g.example/v1"),
                    key: String::from("k"),
                    model: String::from("gpt-6"),
                }),
                None,
                Some(Ok(())),
            )
        );
    }
}
