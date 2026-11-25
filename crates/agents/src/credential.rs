use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
)]
pub enum Account {
    Own,
    Named(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Auth {
    Stored { kind: String, url: Option<String> },
    SignedIn,
    Unmanaged,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema,
)]
pub enum FieldKind {
    Text,
    Url,
    Secret,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Field {
    pub id: String,
    pub label: String,
    pub kind: FieldKind,
    pub placeholder: String,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CredentialForm {
    pub kind: String,
    pub label: String,
    pub fields: Vec<Field>,
    pub account: String,
    pub setup_sign_in: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderInfo {
    pub label: String,
    pub note: Option<String>,
    pub credentials: Vec<CredentialForm>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Credential {
    pub kind: String,
    pub values: BTreeMap<String, String>,
}

impl core::fmt::Debug for Credential {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Credential")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl Credential {
    #[must_use]
    pub fn value(&self, id: &str) -> &str {
        self.values.get(id).map_or("", String::as_str)
    }
}

impl ProviderInfo {
    #[must_use]
    pub fn form(&self, kind: &str) -> Option<&CredentialForm> {
        self.credentials.iter().find(|form| form.kind == kind)
    }

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

    pub fn account(&self, credential: &Credential) -> Result<Account, String> {
        self.check(credential)?;
        let form = self.form(&credential.kind).ok_or_else(|| {
            format!("{} takes no {:?} credential", self.label, credential.kind)
        })?;
        let mut fields =
            form.fields.iter().filter(|field| field.id == form.account);
        let field = fields
            .next()
            .filter(|field| field.kind != FieldKind::Secret);
        if field.is_none() || fields.next().is_some() {
            return Err(format!(
                "{} has no unambiguous public account field",
                form.label
            ));
        }
        let value = credential.value(&form.account).trim();
        if value.is_empty() {
            return Err(format!("{} names no account", form.label));
        }
        Ok(Account::Named(String::from(value)))
    }

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
            .find_map(|field| {
                let value = credential.value(&field.id);
                let unique = self
                    .fields
                    .iter()
                    .filter(|other| other.id == field.id)
                    .count()
                    == 1;
                (unique && !value.is_empty() && field.check(value).is_ok())
                    .then_some(value)
            })
            .map(String::from)
    }
}

impl Field {
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

    use super::{Account, Auth};
    use super::{Credential, CredentialForm, Field, FieldKind, ProviderInfo};

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
                account: String::from("url"),
                setup_sign_in: None,
            }]),
        }
    }

    fn credential(kind: &str, values: &[(&str, &str)]) -> Credential {
        Credential {
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
    fn a_credential_signs_in_as_its_form_s_account_field() {
        assert_eq!(
            [
                credential(
                    "gateway",
                    &[("url", "https://g.example"), ("token", "t")],
                ),
                credential("gateway", &[("url", ""), ("token", "t")]),
            ]
            .map(|credential| info().account(&credential)),
            [
                Ok(Account::Named(String::from("https://g.example"))),
                Err(String::from("url is required")),
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
            r#"Credential { kind: "gateway", .. }"#
        );
    }
    #[test]
    fn account_metadata_cannot_name_a_secret_or_ambiguous_field() {
        let mut provider = info();
        let stored = credential(
            "gateway",
            &[("url", "https://g.example"), ("token", "synthetic token")],
        );
        provider.credentials[0].account = String::from("token");
        assert!(provider.account(&stored).is_err());
        provider.credentials[0].account = String::from("url");
        provider.credentials[0].fields.push(field(
            "url",
            FieldKind::Secret,
            false,
        ));
        assert!(provider.account(&stored).is_err());
        assert!(matches!(
            provider.auth(&stored),
            Auth::Stored { url: None, .. }
        ));
    }

    #[test]
    fn a_form_upgrade_revalidates_a_stored_value_before_displaying_it() {
        let mut provider = info();
        provider.credentials[0].fields[0].kind = FieldKind::Secret;
        let old = credential(
            "gateway",
            &[
                (
                    "url",
                    "https://synthetic:token@g.example/v1?q=hidden#hidden",
                ),
                ("token", "required"),
            ],
        );
        assert!(provider.check(&old).is_ok());
        provider.credentials[0].fields[0].kind = FieldKind::Url;
        assert!(matches!(
            provider.auth(&old),
            Auth::Stored { url: None, .. }
        ));
    }
}
