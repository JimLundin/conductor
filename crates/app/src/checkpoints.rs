//! Per-turn changes and undo.

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
use proto::CheckpointId;
use ui::{DiffPanel, Intent, Patch, SessionIntent};

use crate::Shell;

#[component]
pub fn TurnChanges(changes: ui::TurnChanges) -> Element {
    let shell = use_context::<Shell>();
    let checkpoint = changes.checkpoint;
    rsx! {
        section { class: "turn-changes",
            button {
                class: "show-changes",
                id: "{changes.button}",
                disabled: changes.button_disabled,
                onclick: move |_| shell.dispatch(Intent::Session(SessionIntent::ShowChanges(checkpoint))),
                "Changes"
            }
            match changes.panel {
                DiffPanel::Hidden => rsx! {},
                DiffPanel::Loading => rsx! {
                    p { role: "status", "Loading changes…" }
                    CloseDiff { checkpoint }
                },
                DiffPanel::Failed(reason) => rsx! { p { class: "error", role: "alert", "{reason}" } },
                DiffPanel::Loaded(diff) => rsx! { Diff { checkpoint, diff } },
            }
        }
    }
}

#[component]
fn CloseDiff(checkpoint: CheckpointId) -> Element {
    let shell = use_context::<Shell>();
    rsx! {
        button {
            class: "close-diff",
            onclick: move |_| shell.dispatch(Intent::Session(SessionIntent::CloseChanges(checkpoint))),
            "Close changes"
        }
    }
}

#[component]
fn Diff(checkpoint: CheckpointId, diff: ui::Diff) -> Element {
    let shell = use_context::<Shell>();
    rsx! {
        div { class: "diff-panel", "aria-label": "Changes for this turn",
            div { class: "diff-heading",
                h2 { "Changes for this turn" }
                CloseDiff { checkpoint }
            }
            div { class: "diff-files", tabindex: "0", role: "region", "aria-label": "Changed files",
                if let Some(note) = diff.empty {
                    p { class: "diff-note", "{note}" }
                }
                for file in diff.files {
                    section { class: "diff-file",
                        code { class: "diff-path", "{file.path}" }
                        match file.patch {
                            Patch::Text { text, label } => rsx! { pre { class: "diff-patch", tabindex: "0", "aria-label": "{label}", "{text}" } },
                            Patch::Note(note) => rsx! { p { class: "diff-note", "{note}" } },
                        }
                    }
                }
            }
            if let Some(note) = diff.truncated {
                p { class: "diff-note", "{note}" }
            }
            p { class: "undo-scope", "Undo removes this turn and every later turn from the conversation, and restores files to before this turn." }
            button {
                class: "undo-turn",
                disabled: diff.undo_disabled,
                onclick: move |_| shell.dispatch(Intent::Session(SessionIntent::Undo(checkpoint))),
                "Undo this turn"
            }
            p { class: "undo-hint", "{diff.undo_hint}" }
        }
    }
}
