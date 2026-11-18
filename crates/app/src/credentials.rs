//! The host's instances and their credentials: what each instance signs in
//! with, a form for each kind of credential its provider's plugin
//! describes, a button to clear the stored one, its label, a button to
//! remove it, and a form per provider to add another instance. The forms
//! are drawn from the host's description of them.

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
use ui::{ADD_HEADING, CredentialIntent, Intent};

use crate::sessions::Hint;
use crate::{PaneHeader, Shell};

#[component]
pub fn CredentialsPane(pane: ui::CredentialsPane) -> Element {
    rsx! {
        section { class: "pane new-pane",
            PaneHeader { title: "Credentials", subtitle: "{pane.subtitle}" }
            div { class: "new-session",
                for instance in pane.instances {
                    InstanceCredentials { key: "{instance.heading}", instance }
                }
                section { class: "credential", "aria-labelledby": ADD_HEADING,
                    h3 { id: ADD_HEADING, tabindex: "-1", "Add an instance" }
                    if let Some(hint) = pane.removed {
                        Hint { hint }
                    }
                    for form in pane.add {
                        AddForm { key: "{form.button}", form }
                    }
                }
            }
        }
    }
}

/// The instance's heading, what it signs in with now, its label, buttons
/// to clear a stored credential and to remove it, and a form for each kind
/// of credential it takes.
#[component]
fn InstanceCredentials(instance: ui::InstanceCredentials) -> Element {
    let shell = use_context::<Shell>();
    let clear = instance.instance.clone();
    let remove = instance.instance.clone();
    rsx! {
        section { class: "credential", "aria-labelledby": "{instance.heading}",
            div { class: "credential-head",
                div {
                    h3 { id: "{instance.heading}", tabindex: "-1", "{instance.name}" }
                    p { class: "credential-provider", "{instance.provider}" }
                    p { class: "credential-state", "{instance.state}" }
                }
                if let Some(button) = instance.clear {
                    button {
                        r#type: "button",
                        class: "clear",
                        "aria-label": "{button.label}",
                        disabled: button.disabled,
                        onclick: move |_| shell.dispatch(Intent::Credential(CredentialIntent::Clear(clear.clone()))),
                        "Clear"
                    }
                }
                button {
                    r#type: "button",
                    class: "clear remove",
                    "aria-label": "{instance.remove.label}",
                    disabled: instance.remove.disabled,
                    onclick: move |_| shell.dispatch(Intent::Credential(CredentialIntent::Remove(remove.clone()))),
                    "Remove"
                }
            }
            Rename { instance: instance.instance.clone(), form: instance.rename }
            for form in instance.forms {
                KindForm { key: "{form.kind}", form }
            }
            p { class: "hint muted", "{instance.login}" }
            if let Some(hint) = instance.outcome {
                Hint { hint }
            }
        }
    }
}

#[component]
fn Rename(instance: proto::InstanceId, form: ui::RenameForm) -> Element {
    let shell = use_context::<Shell>();
    let typed = instance.clone();
    rsx! {
        form {
            class: "credential-kind rename",
            "aria-label": "{form.label}",
            onsubmit: move |event: FormEvent| {
                event.prevent_default();
                shell.dispatch(Intent::Credential(CredentialIntent::Rename(instance.clone())));
            },
            label { class: "field",
                span { class: "field-label", "Label" }
                input {
                    r#type: "text",
                    autocomplete: "off",
                    value: "{form.value}",
                    oninput: move |event| shell.dispatch(Intent::Credential(CredentialIntent::TypeLabel(
                        typed.clone(),
                        event.value(),
                    ))),
                }
            }
            button { r#type: "submit", class: "start", disabled: form.disabled, "Rename" }
        }
    }
}

#[component]
fn AddForm(form: ui::AddForm) -> Element {
    let shell = use_context::<Shell>();
    let provider = form.provider;
    rsx! {
        form {
            class: "credential-kind add",
            "aria-label": "{form.button}",
            onsubmit: move |event: FormEvent| {
                event.prevent_default();
                shell.dispatch(Intent::Credential(CredentialIntent::Add(provider)));
            },
            label { class: "field",
                span { class: "field-label", "Label" }
                input {
                    r#type: "text",
                    autocomplete: "off",
                    placeholder: "{form.placeholder}",
                    value: "{form.value}",
                    oninput: move |event| shell.dispatch(Intent::Credential(CredentialIntent::TypeNewLabel(
                        provider,
                        event.value(),
                    ))),
                }
            }
            button { r#type: "submit", class: "start", disabled: form.disabled, "{form.button}" }
            if let Some(hint) = form.outcome {
                Hint { hint }
            }
        }
    }
}

#[component]
fn KindForm(form: ui::KindForm) -> Element {
    let shell = use_context::<Shell>();
    let instance = form.instance.clone();
    let kind = form.kind.clone();
    rsx! {
        form {
            class: "credential-kind",
            "aria-label": "{form.label}",
            onsubmit: move |event: FormEvent| {
                event.prevent_default();
                shell.dispatch(Intent::Credential(CredentialIntent::Save {
                    instance: instance.clone(),
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
    let instance = field.instance.clone();
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
                    instance: instance.clone(),
                    kind: kind.clone(),
                    field: id.clone(),
                    text: event.value(),
                })),
            }
        }
    }
}
