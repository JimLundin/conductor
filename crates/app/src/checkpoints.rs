//! Per-turn changes and undo. The host's durable events own undo state.

use client::host_view::{Composer, UndoState};
use dioxus::prelude::*;
use proto::{
    Change, CheckpointDiff, CheckpointId, CommandBody, CommandResult, DiffExtent, FilePatch,
    SessionId, UndoAvailability,
};

use crate::{Link, run_command, send};

#[derive(Clone, PartialEq)]
enum DiffState {
    Hidden,
    Loading(u64),
    Loaded(CheckpointDiff),
    Failed(String),
}

#[component]
pub(crate) fn TurnChanges(
    session: SessionId,
    checkpoint: CheckpointId,
    composer: Composer,
    availability: UndoAvailability,
    undo: UndoState,
    link: Signal<Link>,
    notice: Signal<Option<String>>,
) -> Element {
    let diff = use_signal(|| DiffState::Hidden);
    let generation = use_signal(|| 0u64);
    let loading = std::iter::once(diff()).any(|state| matches!(state, DiffState::Loading(_)));
    let busy = std::iter::once(&undo).any(|undo| matches!(undo, UndoState::Undoing(_)));
    let allowed = std::iter::once((&composer, &availability))
        .any(|pair| matches!(pair, (Composer::Open, UndoAvailability::Available)));
    rsx! {
        section { class: "turn-changes",
            button {
                class: "show-changes",
                id: "{changes_id(checkpoint)}",
                disabled: busy || loading,
                onclick: move |_| load_diff(session, checkpoint, link, diff, generation),
                "Changes"
            }
            match diff() {
                DiffState::Hidden => rsx! {},
                DiffState::Loading(_) => rsx! {
                    p { role: "status", "Loading changes…" }
                    button { class: "close-diff", onclick: move |_| close_diff(diff, checkpoint), "Close changes" }
                },
                DiffState::Failed(reason) => rsx! { p { class: "error", role: "alert", "{reason}" } },
                DiffState::Loaded(changes) => rsx! {
                    div { class: "diff-panel", "aria-label": "Changes for this turn",
                        div { class: "diff-heading",
                            h2 { "Changes for this turn" }
                            button { class: "close-diff", onclick: move |_| close_diff(diff, checkpoint), "Close changes" }
                        }
                        DiffFiles { changes }
                        p { class: "undo-scope", "Undo removes this turn and every later turn from the conversation, and restores files to before this turn." }
                        button {
                            class: "undo-turn",
                            disabled: !allowed,
                            onclick: move |_| run_command(link, notice, CommandBody::UndoTurn { session, checkpoint }),
                            "Undo this turn"
                        }
                        match availability {
                            UndoAvailability::Available => rsx! { p { class: "undo-hint", "Undo is available when the session is idle." } },
                            UndoAvailability::Unavailable { reason } => rsx! { p { class: "undo-hint", "Undo unavailable: {reason}" } },
                        }
                    }
                },
            }
        }
    }
}

fn load_diff(
    session: SessionId,
    checkpoint: CheckpointId,
    link: Signal<Link>,
    diff: Signal<DiffState>,
    generation: Signal<u64>,
) {
    match diff() {
        DiffState::Loading(_) => {}
        DiffState::Hidden | DiffState::Loaded(_) | DiffState::Failed(_) => {
            begin_diff(session, checkpoint, link, diff, generation)
        }
    }
}

fn begin_diff(
    session: SessionId,
    checkpoint: CheckpointId,
    link: Signal<Link>,
    mut diff: Signal<DiffState>,
    mut generation: Signal<u64>,
) {
    let request = generation().saturating_add(1);
    generation.set(request);
    diff.set(DiffState::Loading(request));
    spawn(async move {
        let answer = match send(
            link(),
            CommandBody::GetDiff {
                session,
                checkpoint,
            },
        )
        .await
        {
            Ok(CommandResult::Diff(changes)) => DiffState::Loaded(changes),
            Ok(CommandResult::Rejected(reason)) => DiffState::Failed(reason.label()),
            Ok(
                CommandResult::Accepted
                | CommandResult::Started { .. }
                | CommandResult::Providers(_),
            ) => DiffState::Failed(String::from("The host did not return changes.")),
            Err(reason) => DiffState::Failed(reason),
        };
        match (diff(), request) {
            (DiffState::Loading(active), request) if active == request => diff.set(answer),
            (_, _) => {}
        }
    });
}

#[component]
fn DiffFiles(changes: CheckpointDiff) -> Element {
    rsx! {
        div { class: "diff-files", tabindex: "0", role: "region", "aria-label": "Changed files",
            match (changes.files.first(), &changes.extent) {
                (None, DiffExtent::Complete) => rsx! { p { class: "diff-note", "No file changes in this turn." } },
                (None, DiffExtent::Truncated) => rsx! { p { class: "diff-note", "No files fit in this preview." } },
                (Some(_), _) => rsx! {},
            }
            for file in changes.files {
                section { class: "diff-file",
                    code { class: "diff-path", "{change_label(file.change)} {file.path}" }
                    match file.patch {
                        FilePatch::Text(patch) => rsx! { pre { class: "diff-patch", tabindex: "0", "aria-label": "{file.path} patch", "{patch}" } },
                        FilePatch::Binary => rsx! { p { class: "diff-note", "Binary file changed. Preview unavailable." } },
                        FilePatch::TooLarge => rsx! { p { class: "diff-note", "File exceeds the preview limit." } },
                        FilePatch::Unsupported { reason } => rsx! { p { class: "diff-note", "Preview unavailable: {reason}" } },
                    }
                }
            }
        }
        match changes.extent {
            DiffExtent::Complete => rsx! {},
            DiffExtent::Truncated => rsx! { p { class: "diff-note", "Preview limit reached. Some changes are omitted." } },
        }
    }
}

fn change_label(change: Change) -> String {
    String::from(match change {
        Change::Write => "Added",
        Change::Edit => "Modified",
        Change::Delete => "Deleted",
    })
}

fn changes_id(checkpoint: CheckpointId) -> String {
    format!("checkpoint-{}-changes", checkpoint.0)
}

fn close_diff(mut diff: Signal<DiffState>, checkpoint: CheckpointId) {
    diff.set(DiffState::Hidden);
    document::eval(&format!(
        "requestAnimationFrame(() => document.getElementById('{}')?.focus())",
        changes_id(checkpoint)
    ));
}
