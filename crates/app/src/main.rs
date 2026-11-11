//! The Conductor UI. One host: the status line, then each session's
//! transcript with its request cards and a message box. `dx serve --platform
//! web` runs it in a browser.

// rsx! expands each "{text}" node to format!, which clippy flags.
#![allow(clippy::useless_format)]

mod ticket;

use std::convert::Infallible;
use std::rc::Rc;

use client::host_view::{self, HostView, Item, ReplyState, RequestState, SessionView, ToolState};
use client::{HostLink, LinkOptions, LinkState, PathKind, SendError, Status, Target, Update};
use dioxus::prelude::*;
use proto::{
    CommandBody, CommandResult, Decision, Provider, RequestId, Resolution, SessionId, Ticket,
    ToolKind, ToolOutcome, TurnOutcome,
};
use tokio::sync::mpsc;

const CSS: Asset = asset!("/assets/main.css");

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let initial = use_hook(ticket::initial);
    let mut paired = use_signal(|| initial.as_ref().ok().cloned().flatten());
    let first_error = initial
        .err()
        .map(|error| format!("That join link is not valid: {error}"));
    rsx! {
        document::Stylesheet { href: CSS }
        match paired() {
            Some(ticket) => rsx! {
                Host {
                    ticket,
                    on_change_host: move |()| {
                        ticket::forget();
                        paired.set(None);
                    },
                }
            },
            None => rsx! {
                Pair {
                    error: first_error,
                    on_pair: move |ticket: Ticket| {
                        ticket::save(&ticket);
                        paired.set(Some(ticket));
                    },
                }
            },
        }
    }
}

/// Shown until a host is known. The host prints its join link when it starts;
/// opening that link pairs Safari, and pasting it here pairs a Home Screen app.
#[component]
fn Pair(error: Option<String>, on_pair: EventHandler<Ticket>) -> Element {
    let mut text = use_signal(String::new);
    let mut error = use_signal(move || error);
    rsx! {
        main { class: "empty",
            h1 { "Connect to a host" }
            p { "Paste the join link your host printed when it started." }
            form {
                class: "pair",
                onsubmit: move |event: FormEvent| {
                    event.prevent_default();
                    match ticket::parse_join(&text()) {
                        Ok(ticket) => on_pair.call(ticket),
                        Err(err) => error.set(Some(format!("That join link is not valid: {err}"))),
                    }
                },
                input {
                    r#type: "url",
                    placeholder: "https://…#join=…",
                    autocomplete: "off",
                    value: "{text}",
                    oninput: move |event| text.set(event.value()),
                }
                button { r#type: "submit", disabled: text().trim().is_empty(), "Connect" }
            }
            if let Some(message) = error() {
                p { class: "error", "{message}" }
            }
        }
    }
}

#[component]
fn Host(ticket: Ticket, on_change_host: EventHandler<()>) -> Element {
    let status = use_signal(|| None::<Status>);
    let view = use_signal(HostView::empty);
    let link = use_signal(|| None::<Rc<HostLink>>);
    let notice = use_signal(|| None::<String>);
    use_future(move || run_link(ticket.clone(), status, view, link));
    rsx! {
        div { class: "app",
            StatusLine { status: status(), on_change_host }
            main { class: "transcript",
                for (id, session) in view.read().sessions.iter() {
                    SessionPane { key: "{id.0}", id: *id, session: session.clone(), link, notice }
                }
            }
            if let Some(notice) = notice() {
                p { class: "notice", "{notice}" }
            }
        }
    }
}

async fn run_link(
    ticket: Ticket,
    status: Signal<Option<Status>>,
    view: Signal<HostView>,
    mut link: Signal<Option<Rc<HostLink>>>,
) {
    let options = LinkOptions::new(ticket::client_name());
    match HostLink::spawn(Target::of_ticket(&ticket), options).await {
        Ok((handle, updates)) => {
            link.set(Some(Rc::new(handle)));
            let Err(err) = follow(updates, status, view).await;
            tracing::warn!(%err, "the link ended");
        }
        Err(err) => tracing::error!(%err, "could not start iroh"),
    }
}

async fn follow(
    mut updates: mpsc::Receiver<Update>,
    mut status: Signal<Option<Status>>,
    mut view: Signal<HostView>,
) -> Result<Infallible, SendError> {
    loop {
        match updates.recv().await {
            Some(Update::Status(next)) => {
                tracing::info!(state = ?next.state, path = ?next.path, error = ?next.error, "link status");
                status.set(Some(next));
            }
            Some(Update::Reset(_)) => view.set(HostView::empty()),
            Some(Update::Event(event)) => {
                let folded = view.replace(HostView::empty()).apply(event);
                view.set(folded);
            }
            None => break Err(SendError::Stopped),
        }
    }
}

fn run_command(
    link: Signal<Option<Rc<HostLink>>>,
    mut notice: Signal<Option<String>>,
    body: CommandBody,
) {
    match link() {
        Some(link) => {
            spawn(async move {
                notice.set(match link.send(body).await {
                    Ok(CommandResult::Accepted | CommandResult::Started { .. }) => None,
                    Ok(CommandResult::Rejected(rejection)) => Some(rejection.label()),
                    Err(err) => Some(err.to_string()),
                });
            });
        }
        None => notice.set(Some(String::from("Not connected yet."))),
    }
}

#[component]
fn StatusLine(status: Option<Status>, on_change_host: EventHandler<()>) -> Element {
    let status = status.unwrap_or(Status {
        state: LinkState::Disconnected,
        path: PathKind::Unknown,
        host: None,
        error: None,
        upgrade_error: None,
    });
    let state = format!("{:?}", status.state);
    let (path_attr, path) = match status.path {
        PathKind::Direct => ("Direct", "Direct".to_string()),
        PathKind::Relayed => ("Relayed", "Relayed".to_string()),
        PathKind::WebRtc(Some(pair)) => (
            "WebRtc",
            format!("WebRTC {}/{}", pair.local.as_str(), pair.remote.as_str()),
        ),
        PathKind::WebRtc(None) => ("WebRtc", "WebRTC".to_string()),
        PathKind::Unknown => ("Unknown", "path unknown".to_string()),
    };
    let candidates = match status.path {
        PathKind::WebRtc(Some(pair)) => format!("{}/{}", pair.local.as_str(), pair.remote.as_str()),
        _ => String::new(),
    };
    let upgrade_error = status.upgrade_error.unwrap_or_default();
    let host = status.host.unwrap_or_else(|| "host".into());
    rsx! {
        header { class: "status", "data-state": "{state}", "data-path": "{path_attr}",
            "data-candidates": "{candidates}", "data-upgrade-error": "{upgrade_error}",
            span { class: "dot" }
            span { class: "host", "{host}" }
            span { class: "state", "{state}" }
            span { class: "path", "{path}" }
            button { class: "change-host", onclick: move |_| on_change_host.call(()), "Change host" }
            if let Some(error) = status.error {
                span { class: "error", "{error}" }
            }
        }
    }
}

#[component]
fn SessionPane(
    id: SessionId,
    session: SessionView,
    link: Signal<Option<Rc<HostLink>>>,
    notice: Signal<Option<String>>,
) -> Element {
    let composer = session.controls().composer;
    let provider = match session.provider {
        Provider::Claude => "Claude Code",
        Provider::Codex => "Codex",
    };
    rsx! {
        section { class: "session-pane",
            h2 { class: "session-title", "{provider} in {session.directory.0}" }
            div { class: "items",
                for (i, item) in session.items.into_iter().enumerate() {
                    TranscriptItem { key: "{i}", item, on_decide: move |(request, decision)| {
                        run_command(link, notice, CommandBody::Resolve { session: id, request, decision })
                    } }
                }
            }
            Composer { composer, on_send: move |text| run_command(link, notice, CommandBody::SendMessage { session: id, text }) }
        }
    }
}

#[component]
fn TranscriptItem(item: Item, on_decide: EventHandler<(RequestId, Decision)>) -> Element {
    match item {
        Item::User { text, by } => rsx! { p { class: "user", "{by.0}: {text}" } },
        Item::Reply { text, state } => rsx! {
            p { class: match state { ReplyState::Done => "reply", ReplyState::Streaming => "reply streaming" }, "{text}" }
        },
        Item::Tool { tool, state, .. } => {
            let state = match state {
                ToolState::Running => "running",
                ToolState::Finished(outcome) => match outcome {
                    ToolOutcome::Succeeded => "succeeded",
                    ToolOutcome::Failed => "failed",
                    ToolOutcome::Declined => "declined",
                },
            };
            let tool = match tool {
                ToolKind::Command { command } => command,
                ToolKind::FileChange { paths } => paths.join(", "),
                ToolKind::Other { name, detail } => format!("{name} {detail}"),
            };
            rsx! { p { class: "tool", "data-state": state, "{tool}" } }
        }
        Item::Request {
            request,
            tool,
            detail,
            state,
        } => rsx! {
            section { class: "request", "data-open": matches!(state, RequestState::Open),
                p { class: "request-title", "{tool} wants to run" }
                pre { "{detail}" }
                match &state {
                    RequestState::Open => rsx! {
                        div { class: "actions",
                            button { class: "approve", onclick: move |_| on_decide((request, Decision::Approve)), "Approve" }
                            button { class: "deny", onclick: move |_| on_decide((request, Decision::Deny)), "Deny" }
                        }
                    },
                    RequestState::Resolved(Resolution::Decided { decision, by }) => rsx! {
                        p { class: "resolved",
                            {match decision { Decision::Approve => "Approved", Decision::Deny => "Denied" }}
                            " by {by.0}"
                        }
                    },
                    RequestState::Resolved(Resolution::Withdrawn) => rsx! { p { class: "resolved", "Withdrawn" } },
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
fn Composer(composer: host_view::Composer, on_send: EventHandler<String>) -> Element {
    let mut draft = use_signal(String::new);
    rsx! {
        form { class: "composer",
            onsubmit: move |evt: FormEvent| {
                evt.prevent_default();
                let text = draft().trim().to_string();
                if !text.is_empty() {
                    on_send(text);
                    draft.set(String::new());
                }
            },
            div { class: "row",
                input {
                    r#type: "text",
                    name: "prompt",
                    placeholder: "Message the agent",
                    autocomplete: "off",
                    enterkeyhint: "send",
                    value: "{draft}",
                    oninput: move |evt| draft.set(evt.value()),
                }
                button { r#type: "submit", disabled: !matches!(composer, host_view::Composer::Open), "Send" }
            }
        }
    }
}
