//! The host's credentials: what each provider signs in with, a form to set
//! a gateway for it, and a button to clear it. A secret only passes through:
//! the field is cleared once it is sent, and nothing keeps it (§6.3).

use client::new_session::label;
use dioxus::prelude::*;
use proto::{
    Auth, Availability, ClaudeCredential, CodexCredential, CommandBody, CommandResult, Credential,
    Provider, Providers, WireApi,
};

use crate::{Link, PaneHeader, send};

/// What came of the last thing a form sent.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Sent {
    Nothing,
    Sending,
    Stored,
    Cleared,
    Failed(String),
}

/// Whether a form can be sent.
enum Save {
    Empty,
    Ready,
}

#[component]
pub(crate) fn CredentialsPane(
    host: String,
    providers: Option<Providers>,
    link: Signal<Link>,
    on_back: EventHandler<()>,
) -> Element {
    let claude = providers.as_ref().map(|providers| providers.claude.clone());
    let codex = providers.as_ref().map(|providers| providers.codex.clone());
    rsx! {
        section { class: "pane new-pane",
            PaneHeader { title: "Credentials", subtitle: "How {host} reaches each agent's model.", on_back }
            div { class: "new-session",
                ClaudeGateway { host: host.clone(), availability: claude, link }
                CodexProvider { host, availability: codex, link }
            }
        }
    }
}

#[component]
fn ClaudeGateway(host: String, availability: Option<Availability>, link: Signal<Link>) -> Element {
    let base_url = use_signal(String::new);
    let mut token = use_signal(String::new);
    let sent = use_signal(|| Sent::Nothing);
    let save = match (base_url().trim(), token().as_str()) {
        ("", _) | (_, "") => Save::Empty,
        (_, _) => Save::Ready,
    };
    rsx! {
        form {
            class: "credential",
            "aria-labelledby": "claude-credential",
            onsubmit: move |event: FormEvent| {
                event.prevent_default();
                let credential = Credential::Claude(ClaudeCredential::Gateway {
                    base_url: base_url().trim().to_string(),
                    token: token.replace(String::new()),
                });
                submit(link, sent, CommandBody::SetCredential { credential }, Sent::Stored);
            },
            State { id: "claude-credential", provider: Provider::Claude, availability, link, sent }
            Field { label: "Gateway URL", kind: "url", placeholder: "https://gateway.example.com", value: base_url }
            Field { label: "Token", kind: "password", placeholder: "", value: token }
            Outcome { host, sent: sent() }
            button { r#type: "submit", class: "start", disabled: !matches!(save, Save::Ready), "Save gateway" }
        }
    }
}

#[component]
fn CodexProvider(host: String, availability: Option<Availability>, link: Signal<Link>) -> Element {
    let name = use_signal(String::new);
    let base_url = use_signal(String::new);
    let model = use_signal(String::new);
    let mut key = use_signal(String::new);
    let sent = use_signal(|| Sent::Nothing);
    let save = match (
        name().trim(),
        base_url().trim(),
        model().trim(),
        key().as_str(),
    ) {
        ("", _, _, _) | (_, "", _, _) | (_, _, "", _) | (_, _, _, "") => Save::Empty,
        (_, _, _, _) => Save::Ready,
    };
    rsx! {
        form {
            class: "credential",
            "aria-labelledby": "codex-credential",
            onsubmit: move |event: FormEvent| {
                event.prevent_default();
                let credential = Credential::Codex(CodexCredential::Provider {
                    name: name().trim().to_string(),
                    base_url: base_url().trim().to_string(),
                    wire_api: WireApi::Responses,
                    key: key.replace(String::new()),
                    model: model().trim().to_string(),
                });
                submit(link, sent, CommandBody::SetCredential { credential }, Sent::Stored);
            },
            State { id: "codex-credential", provider: Provider::Codex, availability, link, sent }
            Field { label: "Provider name", kind: "text", placeholder: "Gateway", value: name }
            Field { label: "Base URL", kind: "url", placeholder: "https://gateway.example.com/v1", value: base_url }
            Field { label: "Model", kind: "text", placeholder: "gpt-6", value: model }
            Field { label: "Key", kind: "password", placeholder: "", value: key }
            Outcome { host, sent: sent() }
            button { r#type: "submit", class: "start", disabled: !matches!(save, Save::Ready), "Save provider" }
        }
    }
}

/// The provider's heading, what it signs in with now, and a button to clear
/// a stored credential.
#[component]
fn State(
    id: String,
    provider: Provider,
    availability: Option<Availability>,
    link: Signal<Link>,
    sent: Signal<Sent>,
) -> Element {
    let state = describe(availability.as_ref());
    rsx! {
        div { class: "credential-head",
            div {
                h3 { id: "{id}", "{label(provider)}" }
                p { class: "credential-state", "{state}" }
            }
            match availability {
                Some(Availability::Available { auth: Auth::Gateway { .. } }) => rsx! {
                    button {
                        r#type: "button",
                        class: "clear",
                        "aria-label": "Clear the {label(provider)} credential",
                        disabled: matches!(sent(), Sent::Sending),
                        onclick: move |_| submit(link, sent, CommandBody::ClearCredential { provider }, Sent::Cleared),
                        "Clear"
                    }
                },
                Some(Availability::Available { auth: Auth::SignedIn | Auth::Unmanaged })
                | Some(Availability::Unavailable { .. })
                | None => rsx! {},
            }
        }
    }
}

#[component]
fn Field(label: String, kind: String, placeholder: String, mut value: Signal<String>) -> Element {
    rsx! {
        label { class: "field",
            span { class: "field-label", "{label}" }
            input {
                r#type: "{kind}",
                placeholder: "{placeholder}",
                autocomplete: "off",
                autocapitalize: "off",
                autocorrect: "off",
                spellcheck: "false",
                value: "{value}",
                oninput: move |event| value.set(event.value()),
            }
        }
    }
}

#[component]
fn Outcome(host: String, sent: Sent) -> Element {
    match sent {
        Sent::Nothing => rsx! {},
        Sent::Sending => rsx! { p { class: "hint muted", role: "status", "Sending…" } },
        Sent::Stored => rsx! {
            p { class: "hint muted", role: "status",
                "Sent to {host} over the encrypted link. This app doesn't keep it."
            }
        },
        Sent::Cleared => rsx! { p { class: "hint muted", role: "status", "Cleared on {host}." } },
        Sent::Failed(error) => rsx! { p { class: "hint error", role: "alert", "{error}" } },
    }
}

/// What a provider signs in with, in words. Never more than the base URL.
fn describe(availability: Option<&Availability>) -> String {
    match availability {
        Some(Availability::Available {
            auth: Auth::Gateway { base_url },
        }) => format!("Gateway at {base_url}"),
        Some(Availability::Available {
            auth: Auth::SignedIn,
        }) => String::from("Signed in on the host"),
        Some(Availability::Available {
            auth: Auth::Unmanaged,
        }) => String::from("The host's own CLI, signed in on its own"),
        Some(Availability::Unavailable { reason }) => format!("Not available: {reason}"),
        None => String::from("Waiting for the host"),
    }
}

fn submit(link: Signal<Link>, mut sent: Signal<Sent>, body: CommandBody, done: Sent) {
    sent.set(Sent::Sending);
    spawn(async move {
        sent.set(match send(link(), body).await {
            Ok(CommandResult::Providers(_)) => done,
            Ok(CommandResult::Rejected(rejection)) => Sent::Failed(rejection.label()),
            Ok(CommandResult::Accepted | CommandResult::Started { .. }) => {
                Sent::Failed(String::from("The host gave an unexpected answer."))
            }
            Err(err) => Sent::Failed(err),
        });
    });
}
