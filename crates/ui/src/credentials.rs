//! The host's credentials: what each provider signs in with, a form for
//! each kind of credential its plugin describes, and a button to clear the
//! stored one. A secret only passes through: its field is cleared once it
//! is sent, and nothing keeps it (§6.3).

use alloc::collections::BTreeMap;

use crate::prelude::*;
use proto::{
    Auth, Availability, Catalog, CommandBody, CommandResult, Credential,
    CredentialForm, Field, FieldKind, Provider, ProviderInfo, Providers,
};

use crate::new_session::{Hint, alert};
use crate::{Effect, Focus, Reply, ReplyTo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialIntent {
    /// `text` is typed into the field `field` of `provider`'s form of
    /// `kind`.
    Type {
        provider: Provider,
        kind: String,
        field: String,
        text: String,
    },
    Save {
        provider: Provider,
        kind: String,
    },
    Clear(Provider),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialsPane {
    pub subtitle: String,
    pub providers: Vec<ProviderCredentials>,
}

/// One provider: what it signs in with now, and a form for each kind of
/// credential it takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCredentials {
    pub provider: Provider,
    /// The heading's id, which labels the provider's section and takes
    /// the focus when the clear button goes.
    pub heading: String,
    pub name: String,
    /// What the provider signs in with now.
    pub state: String,
    pub clear: Option<ClearButton>,
    pub forms: Vec<KindForm>,
    pub outcome: Option<Hint>,
}

/// The form of one kind of credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindForm {
    pub provider: Provider,
    pub kind: String,
    pub label: String,
    pub fields: Vec<FieldView>,
    pub save_label: String,
    pub save_disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearButton {
    pub label: String,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldView {
    pub provider: Provider,
    pub kind: String,
    pub field: String,
    pub label: String,
    /// The input's `type`.
    pub input: &'static str,
    pub placeholder: String,
    /// A password manager ignores `off` on a password field, so a secret
    /// says it is new and isn't filled in from a saved one.
    pub autocomplete: &'static str,
    pub value: String,
}

/// What came of the last thing a provider's forms sent.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Sent {
    Sending,
    Done(Done),
    Failed(String),
}

/// What a credential command did once the host took it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Done {
    Stored,
    Cleared,
}

/// What each form's fields hold, and what each provider's forms sent last.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Forms {
    drafts: BTreeMap<(Provider, String), BTreeMap<String, String>>,
    sent: BTreeMap<Provider, Sent>,
}

impl Forms {
    pub fn update(
        mut self,
        intent: CredentialIntent,
        visit: u64,
        catalog: &Catalog,
    ) -> (Self, Vec<Effect>) {
        match intent {
            CredentialIntent::Type {
                provider,
                kind,
                field,
                text,
            } => {
                self.drafts
                    .entry((provider, kind))
                    .or_default()
                    .insert(field, text);
                (self, Vec::new())
            }
            CredentialIntent::Save { provider, kind } => {
                let described = catalog
                    .of(provider)
                    .and_then(|info| info.form(&kind))
                    .cloned();
                match described {
                    Some(form) => self.save(provider, &form, visit),
                    None => (self, Vec::new()),
                }
            }
            CredentialIntent::Clear(provider) => {
                self.sent.insert(provider, Sent::Sending);
                (
                    self,
                    Vec::from([
                        Effect::Send {
                            body: CommandBody::ClearCredential { provider },
                            reply: Reply {
                                visit,
                                to: ReplyTo::Credential(
                                    provider,
                                    Done::Cleared,
                                ),
                            },
                        },
                        // The button goes once the clear lands, so focus
                        // goes to the heading instead of the page.
                        Effect::Focus(Focus::Element(heading(provider))),
                    ]),
                )
            }
        }
    }

    /// Sends the form's values, the text ones trimmed, and empties its
    /// secrets.
    fn save(
        mut self,
        provider: Provider,
        form: &CredentialForm,
        visit: u64,
    ) -> (Self, Vec<Effect>) {
        let draft = self
            .drafts
            .remove(&(provider, form.kind.clone()))
            .unwrap_or_default();
        let values = form
            .fields
            .iter()
            .map(|field| (field.id.clone(), sent_value(field, &draft)))
            .collect::<BTreeMap<String, String>>();
        let kept = draft
            .into_iter()
            .filter(|(id, _)| !secret(form, id))
            .collect::<BTreeMap<String, String>>();
        self.drafts.insert((provider, form.kind.clone()), kept);
        self.sent.insert(provider, Sent::Sending);
        let credential = Credential {
            provider,
            kind: form.kind.clone(),
            values,
        };
        (
            self,
            Vec::from([Effect::Send {
                body: CommandBody::SetCredential { credential },
                reply: Reply {
                    visit,
                    to: ReplyTo::Credential(provider, Done::Stored),
                },
            }]),
        )
    }

    pub fn replied(
        mut self,
        provider: Provider,
        done: Done,
        answer: Result<CommandResult, String>,
    ) -> Self {
        let sent = match answer {
            Ok(CommandResult::Providers(_)) => Sent::Done(done),
            Ok(CommandResult::Rejected(rejection)) => {
                Sent::Failed(rejection.label())
            }
            Ok(
                CommandResult::Accepted
                | CommandResult::Started { .. }
                | CommandResult::Diff(_),
            ) => Sent::Failed(String::from(
                "The host gave an unexpected answer.",
            )),
            Err(error) => Sent::Failed(error),
        };
        self.sent.insert(provider, sent);
        self
    }

    /// A section for each provider the host's plugins describe.
    pub fn screen(
        &self,
        host: &str,
        providers: Option<&Providers>,
        catalog: &Catalog,
    ) -> CredentialsPane {
        CredentialsPane {
            subtitle: format!("How {host} reaches each agent's model."),
            providers: catalog
                .0
                .iter()
                .map(|(&provider, info)| {
                    self.provider(
                        provider,
                        info,
                        providers.and_then(|known| known.of(provider)),
                        host,
                    )
                })
                .collect(),
        }
    }

    fn provider(
        &self,
        provider: Provider,
        info: &ProviderInfo,
        availability: Option<&Availability>,
        host: &str,
    ) -> ProviderCredentials {
        let sent = self.sent.get(&provider);
        ProviderCredentials {
            provider,
            heading: heading(provider),
            name: info.label.clone(),
            state: describe(info, availability),
            clear: stored(availability).then(|| ClearButton {
                label: format!("Clear the {} credential", info.label),
                disabled: sent == Some(&Sent::Sending),
            }),
            forms: info
                .credentials
                .iter()
                .map(|form| self.form(provider, form))
                .collect(),
            outcome: sent.and_then(|last| outcome(last, host)),
        }
    }

    fn form(&self, provider: Provider, form: &CredentialForm) -> KindForm {
        let empty = BTreeMap::new();
        let draft = self
            .drafts
            .get(&(provider, form.kind.clone()))
            .unwrap_or(&empty);
        KindForm {
            provider,
            kind: form.kind.clone(),
            label: form.label.clone(),
            fields: form
                .fields
                .iter()
                .map(|field| FieldView {
                    provider,
                    kind: form.kind.clone(),
                    field: field.id.clone(),
                    label: field.label.clone(),
                    input: input(field.kind),
                    placeholder: field.placeholder.clone(),
                    autocomplete: autocomplete(field.kind),
                    value: draft.get(&field.id).cloned().unwrap_or_default(),
                })
                .collect(),
            save_label: format!("Save {}", form.label.to_lowercase()),
            save_disabled: form.fields.iter().any(|field| {
                field.required && sent_value(field, draft).is_empty()
            }),
        }
    }
}

/// What a field sends: a secret as typed, anything else trimmed.
fn sent_value(field: &Field, draft: &BTreeMap<String, String>) -> String {
    let typed = draft.get(&field.id).map_or("", String::as_str);
    match field.kind {
        FieldKind::Secret => String::from(typed),
        FieldKind::Text | FieldKind::Url => String::from(typed.trim()),
    }
}

fn secret(form: &CredentialForm, id: &str) -> bool {
    form.fields
        .iter()
        .any(|field| field.id == id && field.kind == FieldKind::Secret)
}

const fn input(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Text => "text",
        FieldKind::Url => "url",
        FieldKind::Secret => "password",
    }
}

const fn autocomplete(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Secret => "new-password",
        FieldKind::Text | FieldKind::Url => "off",
    }
}

fn heading(provider: Provider) -> String {
    format!("{}-credential", provider.name())
}

/// Whether the host stores a credential for the provider, which a person
/// may clear.
const fn stored(availability: Option<&Availability>) -> bool {
    matches!(
        availability,
        Some(Availability::Available {
            auth: Auth::Stored { .. },
            ..
        })
    )
}

fn outcome(sent: &Sent, host: &str) -> Option<Hint> {
    let status = |text| {
        Some(Hint {
            class: "hint muted",
            role: Some("status"),
            text,
        })
    };
    match sent {
        Sent::Sending => status(String::from("Sending…")),
        Sent::Done(Done::Stored) => status(format!(
            "Sent to {host} over the encrypted link. This app doesn't keep it; your browser may offer to save it."
        )),
        Sent::Done(Done::Cleared) => status(format!("Cleared on {host}.")),
        Sent::Failed(error) => Some(alert(error.clone())),
    }
}

/// What a provider signs in with, in words. Never more than a stored
/// credential's kind and URL.
fn describe(
    info: &ProviderInfo,
    availability: Option<&Availability>,
) -> String {
    match availability {
        Some(Availability::Available {
            auth: Auth::Stored { kind, url },
            ..
        }) => {
            let label = info
                .form(kind)
                .map_or_else(|| kind.clone(), |form| form.label.clone());
            url.as_ref()
                .map_or_else(|| label.clone(), |at| format!("{label} at {at}"))
        }
        Some(Availability::Available {
            auth: Auth::SignedIn,
            ..
        }) => String::from("Signed in on the host"),
        Some(Availability::Available {
            auth: Auth::Unmanaged,
            ..
        }) => String::from("The host's own CLI, signed in on its own"),
        Some(Availability::Unavailable { reason }) => {
            format!("Not available: {reason}")
        }
        None => String::from("Waiting for the host"),
    }
}
