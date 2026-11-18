//! The host's credentials: what each provider signs in with, a form for
//! each kind of credential its plugin describes, and a button to clear the
//! stored one. The forms are drawn from the host's description of them.

#![expect(
    clippy::disallowed_types,
    reason = "rsx! caches each template in a OnceLock static"
)]
#![expect(
    clippy::empty_enums,
    reason = "derive(Props) on each component's props struct declares empty error enums for missing and repeated props"
)]
#![expect(
    clippy::shadow_reuse,
    reason = "derive(Props) on each component's props struct rebinds every prop in its builder"
)]

use dioxus::prelude::*;
use ui::{CredentialIntent, Intent};

use crate::sessions::Hint;
use crate::{PaneHeader, Shell};

#[component]
pub fn CredentialsPane(pane: ui::CredentialsPane) -> Element {
    rsx! {
        section { class: "pane new-pane",
            PaneHeader { title: "Credentials", subtitle: "{pane.subtitle}" }
            div { class: "new-session",
                for provider in pane.providers {
                    ProviderCredentials { key: "{provider.heading}", provider }
                }
            }
        }
    }
}

/// The provider's heading, what it signs in with now, a button to clear a
/// stored credential, and a form for each kind it takes.
#[component]
fn ProviderCredentials(provider: ui::ProviderCredentials) -> Element {
    let shell = use_context::<Shell>();
    let which = provider.provider;
    rsx! {
        section { class: "credential", "aria-labelledby": "{provider.heading}",
            div { class: "credential-head",
                div {
                    h3 { id: "{provider.heading}", tabindex: "-1", "{provider.name}" }
                    p { class: "credential-state", "{provider.state}" }
                }
                if let Some(clear) = provider.clear {
                    button {
                        r#type: "button",
                        class: "clear",
                        "aria-label": "{clear.label}",
                        disabled: clear.disabled,
                        onclick: move |_| shell.dispatch(Intent::Credential(CredentialIntent::Clear(which))),
                        "Clear"
                    }
                }
            }
            for form in provider.forms {
                KindForm { key: "{form.kind}", form }
            }
            if let Some(hint) = provider.outcome {
                Hint { hint }
            }
        }
    }
}

#[component]
fn KindForm(form: ui::KindForm) -> Element {
    let shell = use_context::<Shell>();
    let provider = form.provider;
    let kind = form.kind.clone();
    rsx! {
        form {
            class: "credential-kind",
            "aria-label": "{form.label}",
            onsubmit: move |event: FormEvent| {
                event.prevent_default();
                shell.dispatch(Intent::Credential(CredentialIntent::Save {
                    provider,
                    kind: kind.clone(),
                }));
            },
            for field in form.fields {
                Field { key: "{field.field}", field }
            }
            button { r#type: "submit", class: "start", disabled: form.save_disabled, "{form.save_label}" }
        }
    }
}

#[component]
fn Field(field: ui::FieldView) -> Element {
    let shell = use_context::<Shell>();
    let provider = field.provider;
    let kind = field.kind.clone();
    let id = field.field.clone();
    rsx! {
        label { class: "field",
            span { class: "field-label", "{field.label}" }
            input {
                r#type: field.input,
                placeholder: "{field.placeholder}",
                autocomplete: field.autocomplete,
                autocapitalize: "off",
                autocorrect: "off",
                spellcheck: "false",
                value: "{field.value}",
                oninput: move |event| shell.dispatch(Intent::Credential(CredentialIntent::Type {
                    provider,
                    kind: kind.clone(),
                    field: id.clone(),
                    text: event.value(),
                })),
            }
        }
    }
}
