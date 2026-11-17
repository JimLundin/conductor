//! The host's credentials: what each provider signs in with, a form to set
//! a gateway for it, and a button to clear it. A secret only passes
//! through: its field is cleared once it is sent, and nothing keeps it
//! (§6.3).

use crate::prelude::*;
use client::new_session::label;
use proto::{
    Auth, Availability, ClaudeCredential, CodexCredential, CommandBody,
    CommandResult, Credential, Provider, Providers,
};

use crate::new_session::{Hint, alert};
use crate::{Effect, Focus, Reply, ReplyTo};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialIntent {
    Type(Field, String),
    Save(Provider),
    Clear(Provider),
}

/// A field of a credential form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    GatewayUrl,
    GatewayToken,
    ProviderName,
    ProviderUrl,
    ProviderModel,
    ProviderKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialsPane {
    pub subtitle: String,
    pub forms: Vec<CredentialForm>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialForm {
    pub provider: Provider,
    /// The heading's id, which labels the form and takes the focus when
    /// the clear button goes.
    pub heading: &'static str,
    pub name: String,
    /// What the provider signs in with now.
    pub state: String,
    pub clear: Option<ClearButton>,
    pub fields: Vec<FieldView>,
    pub outcome: Option<Hint>,
    pub save_label: &'static str,
    pub save_disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearButton {
    pub label: String,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldView {
    pub field: Field,
    pub label: &'static str,
    pub kind: &'static str,
    pub placeholder: &'static str,
    /// A password manager ignores `off` on a password field, so a secret
    /// says it is new and isn't filled in from a saved one.
    pub autocomplete: &'static str,
    pub value: String,
}

/// What came of the last thing a form sent.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Sent {
    Nothing,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forms {
    claude: Gateway,
    codex: CodexProvider,
}

/// Claude Code's gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Gateway {
    base_url: String,
    token: String,
    sent: Sent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexProvider {
    name: String,
    base_url: String,
    model: String,
    key: String,
    sent: Sent,
}

impl Default for Forms {
    fn default() -> Self {
        Self {
            claude: Gateway {
                base_url: String::new(),
                token: String::new(),
                sent: Sent::Nothing,
            },
            codex: CodexProvider {
                name: String::new(),
                base_url: String::new(),
                model: String::new(),
                key: String::new(),
                sent: Sent::Nothing,
            },
        }
    }
}

impl Forms {
    pub fn update(
        self,
        intent: CredentialIntent,
        visit: u64,
    ) -> (Self, Vec<Effect>) {
        match intent {
            CredentialIntent::Type(field, text) => {
                (self.typed(field, text), Vec::new())
            }
            CredentialIntent::Save(Provider::Claude) => {
                let Gateway {
                    base_url, token, ..
                } = self.claude;
                let credential =
                    Credential::Claude(ClaudeCredential::Gateway {
                        base_url: base_url.trim().to_owned(),
                        token,
                    });
                (
                    Self {
                        claude: Gateway {
                            base_url,
                            token: String::new(),
                            sent: Sent::Sending,
                        },
                        ..self
                    },
                    Vec::from([store(credential, Provider::Claude, visit)]),
                )
            }
            CredentialIntent::Save(Provider::Codex) => {
                let CodexProvider {
                    name,
                    base_url,
                    model,
                    key,
                    ..
                } = self.codex;
                let credential = Credential::Codex(CodexCredential::Provider {
                    name: name.trim().to_owned(),
                    base_url: base_url.trim().to_owned(),
                    key,
                    model: model.trim().to_owned(),
                });
                (
                    Self {
                        codex: CodexProvider {
                            name,
                            base_url,
                            model,
                            key: String::new(),
                            sent: Sent::Sending,
                        },
                        ..self
                    },
                    Vec::from([store(credential, Provider::Codex, visit)]),
                )
            }
            CredentialIntent::Clear(provider) => (
                self.sent(provider, Sent::Sending),
                Vec::from([
                    Effect::Send {
                        body: CommandBody::ClearCredential { provider },
                        reply: Reply {
                            visit,
                            to: ReplyTo::Credential(provider, Done::Cleared),
                        },
                    },
                    // The button goes once the clear lands, so focus goes
                    // to the heading instead of the page.
                    Effect::Focus(Focus::Element(heading(provider).to_owned())),
                ]),
            ),
        }
    }

    pub fn replied(
        self,
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
        self.sent(provider, sent)
    }

    fn typed(self, field: Field, text: String) -> Self {
        let Self { claude, codex } = self;
        match field {
            Field::GatewayUrl => Self {
                claude: Gateway {
                    base_url: text,
                    ..claude
                },
                codex,
            },
            Field::GatewayToken => Self {
                claude: Gateway {
                    token: text,
                    ..claude
                },
                codex,
            },
            Field::ProviderName => Self {
                codex: CodexProvider {
                    name: text,
                    ..codex
                },
                claude,
            },
            Field::ProviderUrl => Self {
                codex: CodexProvider {
                    base_url: text,
                    ..codex
                },
                claude,
            },
            Field::ProviderModel => Self {
                codex: CodexProvider {
                    model: text,
                    ..codex
                },
                claude,
            },
            Field::ProviderKey => Self {
                codex: CodexProvider { key: text, ..codex },
                claude,
            },
        }
    }

    fn sent(self, provider: Provider, sent: Sent) -> Self {
        match provider {
            Provider::Claude => Self {
                claude: Gateway {
                    sent,
                    ..self.claude
                },
                ..self
            },
            Provider::Codex => Self {
                codex: CodexProvider { sent, ..self.codex },
                ..self
            },
        }
    }

    pub fn screen(
        &self,
        host: &str,
        providers: Option<&Providers>,
    ) -> CredentialsPane {
        let known = |provider| providers.and_then(|all| all.of(provider));
        let claude = &self.claude;
        let codex = &self.codex;
        CredentialsPane {
            subtitle: format!("How {host} reaches each agent's model."),
            forms: Vec::from([
                CredentialForm {
                    save_label: "Save gateway",
                    save_disabled: claude.base_url.trim().is_empty()
                        || claude.token.is_empty(),
                    fields: Vec::from([
                        field(
                            Field::GatewayUrl,
                            "Gateway URL",
                            "url",
                            "https://gateway.example.com",
                            &claude.base_url,
                        ),
                        field(
                            Field::GatewayToken,
                            "Token",
                            "password",
                            "",
                            &claude.token,
                        ),
                    ]),
                    ..form(
                        Provider::Claude,
                        known(Provider::Claude),
                        &claude.sent,
                        host,
                    )
                },
                CredentialForm {
                    save_label: "Save provider",
                    save_disabled: codex.name.trim().is_empty()
                        || codex.base_url.trim().is_empty()
                        || codex.model.trim().is_empty()
                        || codex.key.is_empty(),
                    fields: Vec::from([
                        field(
                            Field::ProviderName,
                            "Provider name",
                            "text",
                            "Gateway",
                            &codex.name,
                        ),
                        field(
                            Field::ProviderUrl,
                            "Base URL",
                            "url",
                            "https://gateway.example.com/v1",
                            &codex.base_url,
                        ),
                        field(
                            Field::ProviderModel,
                            "Model",
                            "text",
                            "gpt-6",
                            &codex.model,
                        ),
                        field(
                            Field::ProviderKey,
                            "Key",
                            "password",
                            "",
                            &codex.key,
                        ),
                    ]),
                    ..form(
                        Provider::Codex,
                        known(Provider::Codex),
                        &codex.sent,
                        host,
                    )
                },
            ]),
        }
    }
}

const fn store(
    credential: Credential,
    provider: Provider,
    visit: u64,
) -> Effect {
    Effect::Send {
        body: CommandBody::SetCredential { credential },
        reply: Reply {
            visit,
            to: ReplyTo::Credential(provider, Done::Stored),
        },
    }
}

/// A form's heading, state, clear button and outcome; its fields come
/// from the provider's form.
fn form(
    provider: Provider,
    availability: Option<&Availability>,
    sent: &Sent,
    host: &str,
) -> CredentialForm {
    CredentialForm {
        provider,
        heading: heading(provider),
        name: label(provider),
        state: describe(availability),
        clear: match availability {
            Some(Availability::Available {
                auth: Auth::Gateway { .. },
                ..
            }) => Some(ClearButton {
                label: format!("Clear the {} credential", label(provider)),
                disabled: *sent == Sent::Sending,
            }),
            Some(
                Availability::Available {
                    auth: Auth::SignedIn | Auth::Unmanaged,
                    ..
                }
                | Availability::Unavailable { .. },
            )
            | None => None,
        },
        fields: Vec::new(),
        outcome: outcome(sent, host),
        save_label: "",
        save_disabled: true,
    }
}

const fn heading(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "claude-credential",
        Provider::Codex => "codex-credential",
    }
}

fn field(
    field: Field,
    label: &'static str,
    kind: &'static str,
    placeholder: &'static str,
    value: &str,
) -> FieldView {
    FieldView {
        field,
        label,
        kind,
        placeholder,
        autocomplete: match kind {
            "password" => "new-password",
            _ => "off",
        },
        value: value.to_owned(),
    }
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
        Sent::Nothing => None,
        Sent::Sending => status(String::from("Sending…")),
        Sent::Done(Done::Stored) => status(format!(
            "Sent to {host} over the encrypted link. This app doesn't keep it; your browser may offer to save it."
        )),
        Sent::Done(Done::Cleared) => status(format!("Cleared on {host}.")),
        Sent::Failed(error) => Some(alert(error.clone())),
    }
}

/// What a provider signs in with, in words. Never more than the base URL.
fn describe(availability: Option<&Availability>) -> String {
    match availability {
        Some(Availability::Available {
            auth: Auth::Gateway { base_url },
            ..
        }) => format!("Gateway at {base_url}"),
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
