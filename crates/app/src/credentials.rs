//! The host's credentials: what each provider signs in with, a form to set
//! a gateway for it, and a button to clear it.

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
                for form in pane.forms {
                    CredentialForm { key: "{form.heading}", form }
                }
            }
        }
    }
}

/// The provider's heading, what it signs in with now, a button to clear a
/// stored credential, and the fields of a new one.
#[component]
fn CredentialForm(form: ui::CredentialForm) -> Element {
    let shell = use_context::<Shell>();
    let provider = form.provider;
    rsx! {
        form {
            class: "credential",
            "aria-labelledby": form.heading,
            onsubmit: move |event: FormEvent| {
                event.prevent_default();
                shell.dispatch(Intent::Credential(CredentialIntent::Save(provider)));
            },
            div { class: "credential-head",
                div {
                    h3 { id: form.heading, tabindex: "-1", "{form.name}" }
                    p { class: "credential-state", "{form.state}" }
                }
                if let Some(clear) = form.clear {
                    button {
                        r#type: "button",
                        class: "clear",
                        "aria-label": "{clear.label}",
                        disabled: clear.disabled,
                        onclick: move |_| shell.dispatch(Intent::Credential(CredentialIntent::Clear(provider))),
                        "Clear"
                    }
                }
            }
            for field in form.fields {
                Field { key: "{field.label}", field }
            }
            if let Some(hint) = form.outcome {
                Hint { hint }
            }
            button { r#type: "submit", class: "start", disabled: form.save_disabled, "{form.save_label}" }
        }
    }
}

#[component]
fn Field(field: ui::FieldView) -> Element {
    let shell = use_context::<Shell>();
    let which = field.field;
    rsx! {
        label { class: "field",
            span { class: "field-label", "{field.label}" }
            input {
                r#type: field.kind,
                placeholder: field.placeholder,
                autocomplete: field.autocomplete,
                autocapitalize: "off",
                autocorrect: "off",
                spellcheck: "false",
                value: "{field.value}",
                oninput: move |event| shell.dispatch(Intent::Credential(CredentialIntent::Type(which, event.value()))),
            }
        }
    }
}
