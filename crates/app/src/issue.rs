//! An issue's pane: its status and fields, body, checklist and notes.

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
use ui::{Intent, IssueIntent, Place};

use crate::{PaneHeader, Shell};

#[component]
pub fn IssueScreen(screen: ui::IssueScreen) -> Element {
    let shell = use_context::<Shell>();
    let issue = move |intent| shell.dispatch(Intent::Issue(intent));
    let disabled = !screen.enabled;
    rsx! {
        section { class: "pane new-pane issue-pane", "data-issue": "{screen.key}",
            PaneHeader { title: "{screen.title}", subtitle: "{screen.subtitle}",
                button {
                    r#type: "button",
                    class: "clear to-projects",
                    "aria-label": "Back to projects",
                    onclick: move |_| shell.dispatch(Intent::Show(Place::Projects)),
                    "Projects"
                }
            }
            div { class: "notices", role: "alert",
                if let Some(text) = screen.notice {
                    button { class: "notice", onclick: move |_| shell.dispatch(Intent::DismissNotice), "{text}" }
                }
                for error in screen.submission_errors {
                    p { class: "notice", "{error}" }
                }
            }
            div { class: "new-session",
                div { class: "issue-fields",
                    label { "Status"
                        select {
                            name: "status",
                            disabled,
                            onchange: move |event| issue(IssueIntent::SetStatus(event.value())),
                            for pick in screen.status {
                                option { value: pick.word, selected: pick.selected, "{pick.label}" }
                            }
                        }
                    }
                    for picker in screen.fields {
                        label { key: "{picker.field}", "{picker.field}"
                            select {
                                name: "{picker.field}",
                                disabled,
                                onchange: {
                                    let field = picker.field.clone();
                                    move |event: FormEvent| issue(IssueIntent::SetField { field: field.clone(), value: event.value() })
                                },
                                for pick in picker.picks {
                                    option { value: pick.word, selected: pick.selected, "{pick.label}" }
                                }
                            }
                        }
                    }
                }
                p { class: "hint muted issue-detail", "{screen.detail}" }
                match screen.editing {
                    Some(form) => rsx! { Editing { form, disabled } },
                    None => rsx! {
                        if !screen.labels.is_empty() {
                            p { class: "issue-labels", "{screen.labels}" }
                        }
                        p { class: "issue-body", "{screen.body}" }
                        button { r#type: "button", class: "clear edit", disabled, onclick: move |_| issue(IssueIntent::Edit), "Edit" }
                    },
                }
                h3 { "Checklist" }
                ul { class: "checklist",
                    for item in screen.checklist {
                        li { key: "{item.key}", "data-done": item.done,
                            label {
                                input {
                                    r#type: "checkbox",
                                    checked: item.done,
                                    disabled,
                                    onchange: move |_| issue(item.toggle.clone()),
                                }
                                "{item.text}"
                            }
                            button {
                                r#type: "button",
                                class: "clear remove-item",
                                "aria-label": "Remove {item.text}",
                                disabled,
                                onclick: move |_| issue(item.remove.clone()),
                                "Remove"
                            }
                        }
                    }
                }
                form { class: "add-item",
                    onsubmit: move |event: FormEvent| {
                        event.prevent_default();
                        issue(IssueIntent::AddItem);
                    },
                    input {
                        name: "item",
                        placeholder: "Add an item",
                        "aria-label": "New checklist item",
                        value: "{screen.item}",
                        oninput: move |event| issue(IssueIntent::TypeItem(event.value())),
                    }
                    button { r#type: "submit", class: "clear", disabled: disabled || screen.item_pending, "Add" }
                }
                h3 { "Sessions" }
                ul { class: "issue-sessions",
                    for session in screen.sessions {
                        li { key: "{session.key}",
                            button {
                                r#type: "button",
                                class: "issue-row",
                                onclick: move |_| shell.dispatch(session.open.clone()),
                                "{session.label}"
                            }
                        }
                    }
                }
                button {
                    r#type: "button",
                    class: "clear start-on-issue",
                    onclick: move |_| shell.dispatch(screen.start.clone()),
                    "Start a session"
                }
                h3 { "Notes" }
                ul { class: "notes",
                    for note in screen.notes {
                        li { class: "note", "{note}" }
                    }
                }
                form { class: "add-note",
                    onsubmit: move |event: FormEvent| {
                        event.prevent_default();
                        issue(IssueIntent::AddNote);
                    },
                    textarea {
                        name: "note",
                        rows: "2",
                        placeholder: "Add a note",
                        "aria-label": "New note",
                        value: "{screen.note}",
                        oninput: move |event| issue(IssueIntent::TypeNote(event.value())),
                    }
                    button { r#type: "submit", class: "clear", disabled: disabled || screen.note_pending, "Add note" }
                }
            }
        }
    }
}

#[component]
fn Editing(form: ui::EditForm, disabled: bool) -> Element {
    let shell = use_context::<Shell>();
    let issue = move |intent| shell.dispatch(Intent::Issue(intent));
    rsx! {
        form { class: "edit-issue",
            onsubmit: move |event: FormEvent| {
                event.prevent_default();
                issue(IssueIntent::Save);
            },
            label { "Title"
                input {
                    name: "title",
                    value: "{form.title}",
                    oninput: move |event| issue(IssueIntent::TypeTitle(event.value())),
                }
            }
            label { "Labels"
                input {
                    name: "labels",
                    placeholder: "app, credentials",
                    value: "{form.labels}",
                    oninput: move |event| issue(IssueIntent::TypeLabels(event.value())),
                }
            }
            label { "What and why"
                textarea {
                    name: "body",
                    rows: "6",
                    value: "{form.body}",
                    oninput: move |event| issue(IssueIntent::TypeBody(event.value())),
                }
            }
            div { class: "actions",
                button { r#type: "button", class: "clear", onclick: move |_| issue(IssueIntent::Cancel), "Cancel" }
                button { r#type: "submit", disabled: disabled || form.save_disabled, "Save" }
            }
        }
    }
}
