//! One session: its header, transcript and composer.

use client::host_view::{
    self, Interrupt, Item, ReplyState, RequestState, SessionStatus, SessionView, ToolState,
};
use client::new_session::label;
use dioxus::prelude::*;
use proto::{
    CloseReason, CommandBody, Decision, RequestId, Resolution, SessionId, ToolKind, ToolOutcome,
    TurnOutcome,
};

use crate::sessions::short;
use crate::{Link, PaneHeader, run_command};

#[component]
pub(crate) fn SessionPane(
    id: SessionId,
    session: SessionView,
    link: Signal<Link>,
    notice: Signal<Option<String>>,
    on_back: EventHandler<()>,
) -> Element {
    let controls = session.controls();
    let provider = label(session.provider);
    let status = session.status.word();
    let empty = match (&session.status, session.items.first()) {
        (_, Some(_)) => None,
        (SessionStatus::Starting, None) => Some(format!("Starting {provider}…")),
        (SessionStatus::Idle | SessionStatus::Running, None) => {
            Some(format!("{provider} is ready. Send a message to begin."))
        }
        (SessionStatus::Closed(_), None) => {
            Some(String::from("This session ended before it began."))
        }
    };
    rsx! {
        section { class: "pane session-pane", "data-status": "{status}",
            PaneHeader { title: "{short(&session.directory)}", subtitle: "{session.directory.0}", on_back,
                span { class: "pill", "data-status": "{status}", "{provider}, {status}" }
                match controls.interrupt {
                    Interrupt::Offered => rsx! {
                        button {
                            class: "interrupt",
                            onclick: move |_| run_command(link, notice, CommandBody::Interrupt { session: id }),
                            "Interrupt"
                        }
                    },
                    Interrupt::Hidden => rsx! {},
                }
            }
            div { class: "transcript",
                div { class: "items",
                    match empty {
                        Some(text) => rsx! { p { class: "transcript-empty", "{text}" } },
                        None => rsx! {},
                    }
                    for (i, item) in session.items.into_iter().enumerate() {
                        TranscriptItem { key: "{i}", item, on_decide: move |(request, decision)| {
                            run_command(link, notice, CommandBody::Resolve { session: id, request, decision })
                        } }
                    }
                }
            }
            match notice() {
                Some(text) => rsx! {
                    button { class: "notice", role: "alert", onclick: move |_| notice.set(None), "{text}" }
                },
                None => rsx! {},
            }
            match session.status {
                SessionStatus::Closed(reason) => rsx! { p { class: "closed-note", "{closed(&reason)}" } },
                SessionStatus::Starting | SessionStatus::Idle | SessionStatus::Running => rsx! {
                    Composer {
                        composer: controls.composer,
                        provider: provider.clone(),
                        on_send: move |text| run_command(link, notice, CommandBody::SendMessage { session: id, text }),
                    }
                },
            }
        }
    }
}

fn closed(reason: &CloseReason) -> String {
    match reason {
        CloseReason::FailedToStart { message } => format!("The agent did not start: {message}"),
        CloseReason::Exited { status } => format!("The session has ended ({status})."),
    }
}

#[component]
fn TranscriptItem(item: Item, on_decide: EventHandler<(RequestId, Decision)>) -> Element {
    match item {
        Item::User { text, by } => rsx! {
            div { class: "user",
                p { class: "bubble", "{text}" }
                p { class: "by", "{by.0}" }
            }
        },
        Item::Reply { text, state } => rsx! {
            p { class: match state { ReplyState::Done => "reply", ReplyState::Streaming => "reply streaming" }, "{text}" }
        },
        Item::Tool { tool, state, .. } => {
            let state = match state {
                ToolState::Running => "running",
                ToolState::Finished(ToolOutcome::Succeeded) => "succeeded",
                ToolState::Finished(ToolOutcome::Failed) => "failed",
                ToolState::Finished(ToolOutcome::Declined) => "declined",
            };
            rsx! {
                div { class: "tool", "data-state": state, title: state,
                    span { class: "tool-mark", "aria-hidden": "true" }
                    match tool {
                        ToolKind::Command { command } => rsx! {
                            code { class: "tool-detail", "{command}" }
                        },
                        ToolKind::FileChange { paths } => rsx! {
                            span { class: "tool-name", "Edit" }
                            span { class: "tool-paths",
                                for path in paths {
                                    code { class: "tool-detail", "{path}" }
                                }
                            }
                        },
                        ToolKind::Other { name, detail } => rsx! {
                            span { class: "tool-name", "{name}" }
                            code { class: "tool-detail", "{detail}" }
                        },
                    }
                }
            }
        }
        Item::Request {
            request,
            tool,
            detail,
            state,
        } => rsx! {
            section { class: "request", "data-open": matches!(state, RequestState::Open),
                match &state {
                    RequestState::Open => rsx! {
                        p { class: "request-title", "Allow {tool}?" }
                        pre { "{detail}" }
                        div { class: "actions",
                            button { class: "deny", onclick: move |_| on_decide((request, Decision::Deny)), "Deny" }
                            button { class: "approve", onclick: move |_| on_decide((request, Decision::Approve)), "Approve" }
                        }
                    },
                    RequestState::Resolved(resolution) => rsx! {
                        p { class: "request-title", "{tool}" }
                        pre { "{detail}" }
                        p { class: "resolved",
                            match resolution {
                                Resolution::Decided { decision: Decision::Approve, by } => format!("Approved by {}", by.0),
                                Resolution::Decided { decision: Decision::Deny, by } => format!("Denied by {}", by.0),
                                Resolution::Withdrawn => String::from("Withdrawn"),
                            }
                        }
                    },
                }
            }
        },
        Item::TurnCompleted(outcome) => {
            let (attr, text) = match outcome {
                TurnOutcome::Completed => ("completed", String::from("Turn finished")),
                TurnOutcome::Interrupted => ("interrupted", String::from("Interrupted")),
                TurnOutcome::Failed { message } => ("failed", format!("Failed: {message}")),
            };
            rsx! { p { class: "turn-end", "data-outcome": attr, "{text}" } }
        }
        Item::Error(message) => rsx! { p { class: "error", "{message}" } },
    }
}

#[component]
fn Composer(
    composer: host_view::Composer,
    provider: String,
    on_send: EventHandler<String>,
) -> Element {
    let mut draft = use_signal(String::new);
    let placeholder = match composer {
        host_view::Composer::Open => format!("Message {provider}"),
        host_view::Composer::Busy => format!("{provider} is working…"),
        host_view::Composer::Closed => String::from("This session has ended"),
    };
    rsx! {
        form { class: "composer",
            onsubmit: move |event: FormEvent| {
                event.prevent_default();
                submit(draft, composer, on_send);
            },
            textarea {
                name: "prompt",
                rows: "1",
                placeholder: "{placeholder}",
                autocomplete: "off",
                enterkeyhint: "send",
                value: "{draft}",
                oninput: move |event| draft.set(event.value()),
                onkeydown: move |event| send_on_enter(event, draft, composer, on_send),
            }
            button {
                r#type: "submit",
                class: "send",
                "aria-label": "Send",
                disabled: !matches!(composer, host_view::Composer::Open),
                svg { view_box: "0 0 20 20", width: "20", height: "20", "aria-hidden": "true",
                    path { d: "M10 16V4M4.5 9.5 10 4l5.5 5.5", fill: "none", stroke: "currentColor", stroke_width: "2.2", stroke_linecap: "round", stroke_linejoin: "round" }
                }
            }
        }
    }
}

/// Return sends and Shift-Return starts a new line, except while an input
/// method is composing a character.
fn send_on_enter(
    event: KeyboardEvent,
    draft: Signal<String>,
    composer: host_view::Composer,
    on_send: EventHandler<String>,
) {
    if let Some(event) = Some(event).filter(|event| {
        event.key() == Key::Enter && !event.modifiers().shift() && !event.is_composing()
    }) {
        event.prevent_default();
        submit(draft, composer, on_send);
    }
}

fn submit(mut draft: Signal<String>, composer: host_view::Composer, on_send: EventHandler<String>) {
    match (composer, draft().trim()) {
        (host_view::Composer::Open, "")
        | (host_view::Composer::Busy | host_view::Composer::Closed, _) => {}
        (host_view::Composer::Open, text) => {
            on_send(text.to_string());
            draft.set(String::new());
        }
    }
}
