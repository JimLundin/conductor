//! Credentials as data (§6.3). Each provider's plugin describes the kinds
//! of credential it takes as forms of fields; the app renders those forms,
//! and a credential travels and is stored as its form's values. The host
//! checks the values against the form, and the provider's core turns them
//! into what its CLI needs.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::{Auth, Provider};

/// What a field holds, which says how the app shows it and how the host
/// checks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldKind {
    Text,
    /// A plain `http(s)://host/path`. It is shown to every client and may
    /// be written into a CLI's config, so it may not hide a secret in
    /// userinfo, a query or a fragment.
    Url,
    /// Write-only: the host never shows it again, and no `Debug` prints it.
    Secret,
}

/// One field of a credential form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    /// The key of its value in a [`Credential`].
    pub id: String,
    pub label: String,
    pub kind: FieldKind,
    pub placeholder: String,
    pub required: bool,
}

/// One kind of credential a provider takes, as the form it is entered in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialForm {
    /// The [`Credential::kind`] of what the form makes.
    pub kind: String,
    pub label: String,
    pub fields: Vec<Field>,
}

/// How a provider the host runs presents itself, as its plugin describes
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub label: String,
    /// What a new session's form says under the provider when it is
    /// picked, such as what its agent does without asking.
    pub note: Option<String>,
    /// The kinds of credential it takes.
    pub credentials: Vec<CredentialForm>,
}

/// What the host's plugins describe, by provider.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog(pub BTreeMap<Provider, ProviderInfo>);

/// One provider's credential: the values of its kind's form, by field id.
/// As `SetCredential` carries it and as the host stores it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    pub provider: Provider,
    pub kind: String,
    pub values: BTreeMap<String, String>,
}

/// The credentials a host stores, at most one per provider.
///
/// This is the unit the fleet config (§6.5) carries to every host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials(BTreeMap<Provider, Credential>);

/// Shows only the provider and the kind, so no value reaches a log.
impl core::fmt::Debug for Credential {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Credential")
            .field("provider", &self.provider)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl Credential {
    /// The value of the field `id`, empty when there is none.
    #[must_use]
    pub fn value(&self, id: &str) -> &str {
        self.values.get(id).map_or("", String::as_str)
    }
}

impl Credentials {
    #[must_use]
    pub const fn none() -> Self {
        Self(BTreeMap::new())
    }

    /// These credentials with `credential` in its provider's place.
    #[must_use]
    pub fn set(mut self, credential: Credential) -> Self {
        self.0.insert(credential.provider, credential);
        self
    }

    #[must_use]
    pub fn clear(mut self, provider: Provider) -> Self {
        self.0.remove(&provider);
        self
    }

    #[must_use]
    pub fn of(&self, provider: Provider) -> Option<&Credential> {
        self.0.get(&provider)
    }
}

impl Catalog {
    /// `None` when the host runs no plugin for `provider`.
    #[must_use]
    pub fn of(&self, provider: Provider) -> Option<&ProviderInfo> {
        self.0.get(&provider)
    }

    /// What the app calls `provider`: its plugin's label, else its name.
    #[must_use]
    pub fn label(&self, provider: Provider) -> String {
        self.of(provider).map_or_else(
            || String::from(provider.name()),
            |info| info.label.clone(),
        )
    }
}

impl ProviderInfo {
    #[must_use]
    pub fn form(&self, kind: &str) -> Option<&CredentialForm> {
        self.credentials.iter().find(|form| form.kind == kind)
    }

    /// Whether the provider takes `credential`: its kind is one of the
    /// provider's, and its values fill that form.
    ///
    /// # Errors
    ///
    /// Why the credential is refused.
    pub fn check(&self, credential: &Credential) -> Result<(), String> {
        self.form(&credential.kind)
            .ok_or_else(|| {
                format!(
                    "{} takes no {:?} credential",
                    self.label, credential.kind
                )
            })?
            .check(&credential.values)
    }

    /// What `Welcome` may say about a stored credential: its kind and the
    /// URL it reaches, never a value of another field.
    #[must_use]
    pub fn auth(&self, credential: &Credential) -> Auth {
        Auth::Stored {
            kind: credential.kind.clone(),
            url: self
                .form(&credential.kind)
                .and_then(|form| form.url(credential)),
        }
    }
}

impl CredentialForm {
    /// Every value is a field's, and every field takes its value.
    ///
    /// # Errors
    ///
    /// Why the values are refused.
    pub fn check(
        &self,
        values: &BTreeMap<String, String>,
    ) -> Result<(), String> {
        values
            .keys()
            .find(|id| self.fields.iter().all(|field| &&field.id != id))
            .map_or(Ok(()), |id| {
                Err(format!("{} has no field {id:?}", self.label))
            })?;
        self.fields.iter().try_for_each(|field| {
            field.check(values.get(&field.id).map_or("", String::as_str))
        })
    }

    fn url(&self, credential: &Credential) -> Option<String> {
        self.fields
            .iter()
            .filter(|field| field.kind == FieldKind::Url)
            .map(|field| credential.value(&field.id))
            .find(|value| !value.is_empty())
            .map(String::from)
    }
}

impl Field {
    /// Whether `value` fills the field: a required one isn't empty, and a
    /// URL is plain `http(s)://host/path`.
    ///
    /// # Errors
    ///
    /// Why the value is refused.
    pub fn check(&self, value: &str) -> Result<(), String> {
        match (value.is_empty(), self.required, self.kind) {
            (true, true, _) => Err(format!("{} is required", self.label)),
            (false, _, FieldKind::Url) => self.plain_url(value),
            (true, false, _)
            | (false, _, FieldKind::Text | FieldKind::Secret) => Ok(()),
        }
    }

    fn plain_url(&self, value: &str) -> Result<(), String> {
        let url = url::Url::parse(value)
            .map_err(|err| format!("{}: {err}", self.label))?;
        match (
            url.scheme(),
            url.username(),
            url.password(),
            url.query(),
            url.fragment(),
        ) {
            ("http" | "https", "", None, None, None) => Ok(()),
            ("http" | "https", _, _, _, _) => Err(format!(
                "{} may not carry a user, a password, a query or a fragment",
                self.label
            )),
            (_, _, _, _, _) => {
                Err(format!("{} must be http or https", self.label))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::vec::Vec;

    use super::{Credential, CredentialForm, Field, FieldKind, ProviderInfo};
    use crate::{Auth, Provider};

    fn field(id: &str, kind: FieldKind, required: bool) -> Field {
        Field {
            id: String::from(id),
            label: String::from(id),
            kind,
            placeholder: String::new(),
            required,
        }
    }

    fn info() -> ProviderInfo {
        ProviderInfo {
            label: String::from("Agent"),
            note: None,
            credentials: Vec::from([CredentialForm {
                kind: String::from("gateway"),
                label: String::from("Gateway"),
                fields: Vec::from([
                    field("url", FieldKind::Url, true),
                    field("token", FieldKind::Secret, true),
                    field("label", FieldKind::Text, false),
                ]),
            }]),
        }
    }

    fn credential(kind: &str, values: &[(&str, &str)]) -> Credential {
        Credential {
            provider: Provider::ALL[0],
            kind: String::from(kind),
            values: values
                .iter()
                .map(|(id, value)| (String::from(*id), String::from(*value)))
                .collect::<BTreeMap<String, String>>(),
        }
    }

    #[test]
    fn a_credential_fills_its_kind_s_form() {
        let checked = [
            credential(
                "gateway",
                &[("url", "https://g.example/v1"), ("token", "t")],
            ),
            credential("key", &[("url", "https://g.example"), ("token", "t")]),
            credential(
                "gateway",
                &[("url", "https://g.example"), ("token", "")],
            ),
            credential(
                "gateway",
                &[("url", "https://g.example"), ("token", "t"), ("extra", "x")],
            ),
        ]
        .map(|credential| info().check(&credential));
        assert_eq!(
            checked,
            [
                Ok(()),
                Err(String::from(r#"Agent takes no "key" credential"#)),
                Err(String::from("token is required")),
                Err(String::from(r#"Gateway has no field "extra""#)),
            ]
        );
    }

    #[test]
    fn a_url_field_may_not_hide_a_secret() {
        let url = field("Base URL", FieldKind::Url, true);
        let checked = [
            "https://gateway.example/v1",
            "https://user:pw@gateway.example",
            "https://gateway.example/?key=1",
            "https://gateway.example/#k",
            "ftp://gateway.example",
            "gateway.example",
        ]
        .map(|value| url.check(value));
        let hidden = Err(String::from(
            "Base URL may not carry a user, a password, a query or a fragment",
        ));
        assert_eq!(
            checked,
            [
                Ok(()),
                hidden.clone(),
                hidden.clone(),
                hidden,
                Err(String::from("Base URL must be http or https")),
                Err(String::from("Base URL: relative URL without a base")),
            ]
        );
        assert_eq!(field("optional", FieldKind::Url, false).check(""), Ok(()));
    }

    #[test]
    fn welcome_shows_a_credential_s_kind_and_url_and_debug_no_value() {
        let stored = credential(
            "gateway",
            &[("url", "https://g.example"), ("token", "secret-token")],
        );
        assert_eq!(
            info().auth(&stored),
            Auth::Stored {
                kind: String::from("gateway"),
                url: Some(String::from("https://g.example")),
            }
        );
        assert_eq!(
            alloc::format!("{stored:?}"),
            r#"Credential { provider: Claude, kind: "gateway", .. }"#
        );
    }
}
