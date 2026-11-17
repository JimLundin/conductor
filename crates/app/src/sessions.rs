//! The session list and the form that starts a session.

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
#![expect(
    clippy::useless_format,
    reason = "rsx! expands each \"{text}\" node to format!"
)]

use dioxus::prelude::*;
use ui::{FormIntent, Intent, Listing, Place};

use crate::{PaneHeader, Shell};

#[component]
pub fn SessionList(list: ui::SessionList) -> Element {
    let shell = use_context::<Shell>();
    rsx! {
        nav { class: "sidebar",
            header { class: "list-head",
                h1 { "Sessions" }
                button {
                    class: "new",
                    "aria-current": list.new_current,
                    onclick: move |_| shell.dispatch(Intent::Show(Place::NewSession)),
                    "New session"
                }
            }
            match list.listing {
                Listing::Rows(rows) => rsx! {
                    ul { class: "session-list",
                        for row in rows {
                            SessionRow { key: "{row.key}", row }
                        }
                    }
                },
                Listing::Empty(text) => rsx! { p { class: "list-empty", "{text}" } },
            }
        }
    }
}

#[component]
fn SessionRow(row: ui::SessionRow) -> Element {
    let shell = use_context::<Shell>();
    let id = row.id;
    rsx! {
        li {
            button {
                class: "session-row",
                "data-status": "{row.status}",
                "aria-current": row.current,
                onclick: move |_| shell.dispatch(Intent::Show(Place::Session(id))),
                span { class: "dot", "aria-hidden": "true" }
                span { class: "row-text",
                    span { class: "row-title", "{row.title}" }
                    span { class: "row-sub", "{row.sub}" }
                    if let Some(branch) = row.branch {
                        span { class: "row-branch", "{branch}" }
                    }
                }
                span { class: "badge", "{row.badge}" }
            }
        }
    }
}

#[component]
pub fn NewSession(form: ui::NewSessionForm) -> Element {
    let shell = use_context::<Shell>();
    let change = move |intent| shell.dispatch(Intent::Form(intent));
    rsx! {
        section { class: "pane new-pane",
            PaneHeader { title: "New session", subtitle: "Runs on the host, in a directory you choose." }
            form {
                class: "new-session",
                onsubmit: move |event: FormEvent| {
                    event.prevent_default();
                    change(FormIntent::Start);
                },
                fieldset { class: "providers",
                    legend { "Agent" }
                    div { class: "segments",
                        for choice in form.providers {
                            label { class: "segment", "data-available": choice.state,
                                input {
                                    r#type: "radio",
                                    name: "provider",
                                    checked: choice.checked,
                                    onchange: move |_| change(FormIntent::PickProvider(choice.provider)),
                                }
                                span { class: "segment-name", "{choice.label}" }
                                span { class: "segment-state", "{choice.state}" }
                            }
                        }
                    }
                    if let Some(hint) = form.provider_hint {
                        Hint { hint }
                    }
                }
                div { class: "choices",
                    label { class: "field",
                        span { class: "field-label", "Model" }
                        select {
                            name: "model",
                            onchange: move |event| change(FormIntent::PickModel(event.value())),
                            Options { choices: form.models }
                        }
                    }
                    label { class: "field",
                        span { class: "field-label", "Effort" }
                        select {
                            name: "effort",
                            onchange: move |event| change(FormIntent::PickEffort(event.value())),
                            Options { choices: form.efforts }
                        }
                    }
                }
                label { class: "field",
                    span { class: "field-label", "Directory" }
                    input {
                        r#type: "text",
                        name: "directory",
                        placeholder: "/home/you/src/project",
                        autocomplete: "off",
                        autocapitalize: "off",
                        autocorrect: "off",
                        spellcheck: "false",
                        enterkeyhint: "go",
                        "aria-invalid": form.directory_invalid,
                        value: "{form.directory}",
                        oninput: move |event| change(FormIntent::TypeDirectory(event.value())),
                    }
                    if let Some(hint) = form.directory_error {
                        Hint { hint }
                    }
                }
                if let Some(suggestions) = form.suggestions {
                    div { class: "suggestions",
                        p { class: "field-label", "Recent" }
                        ul {
                            for suggestion in suggestions {
                                Suggestion { suggestion }
                            }
                        }
                    }
                }
                if let Some(hint) = form.error {
                    Hint { hint }
                }
                button {
                    r#type: "submit",
                    class: "start",
                    disabled: form.start_disabled,
                    "{form.start_label}"
                }
            }
        }
    }
}

/// The `option`s of a `select`.
#[component]
pub fn Options(choices: Vec<ui::Choice>) -> Element {
    rsx! {
        for choice in choices {
            option {
                value: "{choice.value}",
                selected: choice.selected,
                disabled: choice.disabled,
                "{choice.label}"
            }
        }
    }
}

#[component]
fn Suggestion(suggestion: ui::Suggestion) -> Element {
    let shell = use_context::<Shell>();
    let path = suggestion.path.clone();
    rsx! {
        li {
            button {
                r#type: "button",
                class: "suggestion",
                onclick: move |_| shell.dispatch(Intent::Form(FormIntent::TypeDirectory(path.clone()))),
                span { class: "suggestion-name", "{suggestion.name}" }
                span { class: "suggestion-path", "{suggestion.path}" }
            }
        }
    }
}

#[component]
pub fn Hint(hint: ui::Hint) -> Element {
    rsx! {
        p { class: hint.class, role: hint.role, "{hint.text}" }
    }
}
