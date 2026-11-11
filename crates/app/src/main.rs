//! The Conductor UI. One host for S3: status line, live transcript, approval
//! cards and a prompt box. `dx serve --platform web` runs it in a browser.

// rsx! expands each "{text}" node to format!, which clippy flags.
#![allow(clippy::useless_format)]

mod ticket;

use std::rc::Rc;

use client::transcript::{Item, Outcome, ReplyState, Transcript};
use client::{HostLink, LinkOptions, LinkState, PathKind, Status, Target, Update};
use dioxus::prelude::*;
use proto::{CommandBody, CommandResult, Decision, RequestId, Ticket};

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
    let mut status = use_signal(|| None::<Status>);
    let mut transcript = use_signal(Transcript::empty);
    let mut link = use_signal(|| None::<Rc<HostLink>>);
    let notice = use_signal(|| None::<String>);

    use_future(move || {
        let ticket = ticket.clone();
        async move {
            let spawned = HostLink::spawn(
                Target::of_ticket(&ticket),
                LinkOptions::new(String::from(ticket::CLIENT_NAME)),
            )
            .await;
            let (handle, mut updates) = match spawned {
                Ok(spawned) => spawned,
                Err(err) => {
                    tracing::error!(%err, "could not start iroh");
                    return;
                }
            };
            link.set(Some(Rc::new(handle)));
            while let Some(update) = updates.recv().await {
                match update {
                    Update::Status(next) => {
                        tracing::info!(state = ?next.state, path = ?next.path, error = ?next.error, "link status");
                        status.set(Some(next));
                    }
                    Update::Reset(_) => transcript.set(Transcript::empty()),
                    Update::Event(event) => transcript.with_mut(|t| {
                        *t = std::mem::replace(t, Transcript::empty()).apply(event.kind)
                    }),
                }
            }
        }
    });

    let send = move |body: CommandBody| run_command(link, notice, body);

    rsx! {
        div { class: "app",
            StatusLine { status: status(), on_change_host }
            main { class: "transcript",
                div { class: "items",
                    for (i, item) in transcript.read().items.iter().enumerate() {
                        TranscriptItem { key: "{i}", item: item.clone(), on_decide: move |(id, decision)| {
                            send(CommandBody::Resolve { approval: id, decision })
                        } }
                    }
                }
            }
            Composer { live: status().is_some_and(|s| s.state == LinkState::Live), notice: notice(),
                on_send: move |text| send(CommandBody::Prompt { text }) }
        }
    }
}

fn run_command(
    link: Signal<Option<Rc<HostLink>>>,
    mut notice: Signal<Option<String>>,
    body: CommandBody,
) {
    let Some(link) = link() else {
        notice.set(Some("Not connected yet.".into()));
        return;
    };
    spawn(async move {
        let result = link.send(body).await;
        notice.set(match result {
            Ok(CommandResult::Accepted) => None,
            Ok(CommandResult::Rejected(rejection)) => Some(rejection.label()),
            Err(err) => Some(err.to_string()),
        });
    });
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
fn TranscriptItem(item: Item, on_decide: EventHandler<(RequestId, Decision)>) -> Element {
    match item {
        Item::Reply { text, state } => rsx! {
            p { class: match state { ReplyState::Done => "reply", ReplyState::Streaming => "reply streaming" }, "{text}" }
        },
        Item::Message(text) => rsx! { p { class: "message", "{text}" } },
        Item::TurnFinished => rsx! { p { class: "turn-end", "Turn finished" } },
        Item::Approval {
            id,
            tool,
            detail,
            outcome,
        } => {
            let open = outcome.is_none();
            rsx! {
            section { class: "approval", "data-open": open,
                p { class: "approval-title", "{tool} wants to run" }
                pre { "{detail}" }
                match outcome {
                    None => rsx! {
                        div { class: "actions",
                            button { class: "approve", onclick: move |_| on_decide((id, Decision::Approve)), "Approve" }
                            button { class: "deny", onclick: move |_| on_decide((id, Decision::Deny)), "Deny" }
                        }
                    },
                    Some(Outcome { decision, by }) => rsx! {
                        p { class: "resolved",
                            {match decision { Decision::Approve => "Approved", Decision::Deny => "Denied" }}
                            " by {by}"
                        }
                    },
                }
            }
            }
        }
    }
}

#[component]
fn Composer(live: bool, notice: Option<String>, on_send: EventHandler<String>) -> Element {
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
            if let Some(notice) = notice {
                p { class: "notice", "{notice}" }
            }
            div { class: "row",
                input {
                    r#type: "text",
                    name: "prompt",
                    placeholder: "Ask the agent",
                    autocomplete: "off",
                    enterkeyhint: "send",
                    value: "{draft}",
                    oninput: move |evt| draft.set(evt.value()),
                }
                button { r#type: "submit", disabled: !live, "Send" }
            }
        }
    }
}
