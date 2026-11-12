//! The Conductor UI for one host: the session list, a form to start a
//! session, and each session's transcript. On a phone one screen shows at a
//! time; on a wide window the list is a sidebar. `dx serve --platform web`
//! runs it in a browser.

// rsx! expands each "{text}" node to format!, which clippy flags.
#![allow(clippy::useless_format)]

mod session;
mod sessions;
mod ticket;

use std::convert::Infallible;
use std::rc::Rc;

use client::host_view::HostView;
use client::{HostLink, LinkOptions, LinkState, SendError, Status, Target, Update};
use dioxus::prelude::*;
use proto::{CommandBody, CommandResult, SessionId, Ticket};
use ticket::Opening;
use tokio::sync::mpsc;

const CSS: Asset = asset!("/assets/main.css");

fn main() {
    dioxus::launch(App);
}

/// What fills the detail pane, and on a phone the whole window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    Sessions,
    NewSession,
    Session(SessionId),
}

/// Where commands go.
#[derive(Clone)]
pub(crate) enum Link {
    Waiting,
    Live(Rc<HostLink>),
}

#[component]
fn App() -> Element {
    let initial = use_hook(ticket::initial);
    let mut opening = use_signal(|| initial.as_ref().ok().cloned().unwrap_or(Opening::Unpaired));
    let first_error = initial
        .err()
        .map(|error| format!("That join link is not valid: {error}"));
    rsx! {
        document::Stylesheet { href: CSS }
        match opening() {
            Opening::Host(ticket) => rsx! {
                Host {
                    ticket,
                    on_change_host: move |()| {
                        ticket::forget();
                        opening.set(Opening::Unpaired);
                    },
                }
            },
            Opening::Unpaired => rsx! {
                Pair {
                    error: first_error,
                    on_pair: move |ticket: Ticket| {
                        ticket::save(&ticket);
                        opening.set(Opening::Host(ticket));
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
            match error() {
                Some(message) => rsx! { p { class: "error", "{message}" } },
                None => rsx! {},
            }
        }
    }
}

/// The screens over one host's view.
#[component]
fn Host(ticket: Ticket, on_change_host: EventHandler<()>) -> Element {
    let status = use_signal(|| None::<Status>);
    let view = use_signal(HostView::empty);
    let link = use_signal(|| Link::Waiting);
    use_future(move || run_link(ticket.clone(), status, view, link));
    let mut screen = use_signal(|| Screen::Sessions);
    let notice = use_signal(|| None::<String>);
    let session = match screen() {
        Screen::Session(id) => view.read().sessions.get(&id).cloned(),
        Screen::Sessions | Screen::NewSession => None,
    };
    // A session the host no longer reports (after a reset) falls back to the
    // list, which on a phone is the only way out of the detail pane.
    let shown = match (screen(), &session) {
        (Screen::NewSession, _) | (Screen::Session(_), Some(_)) => "detail",
        (Screen::Sessions, _) | (Screen::Session(_), None) => "list",
    };
    rsx! {
        div { class: "app", "data-screen": shown,
            StatusLine { status: status(), on_change_host }
            div { class: "panes",
                sessions::SessionList { view, screen: screen(), on_select: move |next| screen.set(next) }
                main { class: "detail",
                    match (screen(), session) {
                        (Screen::NewSession, _) => rsx! {
                            sessions::NewSession {
                                view,
                                providers: status().and_then(|status| status.providers),
                                link,
                                on_back: move |()| screen.set(Screen::Sessions),
                                on_started: move |id| screen.set(Screen::Session(id)),
                            }
                        },
                        (Screen::Session(id), Some(session)) => rsx! {
                            session::SessionPane {
                                key: "{id.0}",
                                id,
                                session,
                                link,
                                notice,
                                on_back: move |()| screen.set(Screen::Sessions),
                            }
                        },
                        (Screen::Sessions | Screen::Session(_), _) => rsx! {
                            div { class: "placeholder",
                                p { "Choose a session, or start a new one." }
                            }
                        },
                    }
                }
            }
        }
    }
}

/// A screen's title bar. The back button shows only on a phone, where the
/// list is a screen of its own.
#[component]
pub(crate) fn PaneHeader(
    title: String,
    subtitle: String,
    on_back: EventHandler<()>,
    children: Element,
) -> Element {
    rsx! {
        header { class: "pane-head",
            button { class: "back", "aria-label": "Sessions", onclick: move |_| on_back(()),
            }
            div { class: "pane-title",
                h2 { "{title}" }
                p { "{subtitle}" }
            }
            {children}
        }
    }
}

async fn run_link(
    ticket: Ticket,
    status: Signal<Option<Status>>,
    view: Signal<HostView>,
    mut link: Signal<Link>,
) {
    let options = LinkOptions::new(client::platform::client_name());
    match HostLink::spawn(Target::of_ticket(&ticket), options).await {
        Ok((handle, updates)) => {
            link.set(Link::Live(Rc::new(handle)));
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
                tracing::info!(state = ?next.state, error = ?next.error, "link status");
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

pub(crate) async fn send(link: Link, body: CommandBody) -> Result<CommandResult, String> {
    match link {
        Link::Live(link) => link.send(body).await.map_err(|err| err.to_string()),
        Link::Waiting => Err(String::from("Not connected to the host yet.")),
    }
}

/// Sends a command whose only answer worth showing is a refusal.
pub(crate) fn run_command(
    link: Signal<Link>,
    mut notice: Signal<Option<String>>,
    body: CommandBody,
) {
    spawn(async move {
        notice.set(match send(link(), body).await {
            Ok(CommandResult::Accepted | CommandResult::Started { .. }) => None,
            Ok(CommandResult::Rejected(rejection)) => Some(rejection.label()),
            Err(err) => Some(err),
        });
    });
}

#[component]
fn StatusLine(status: Option<Status>, on_change_host: EventHandler<()>) -> Element {
    let status = status.unwrap_or(Status {
        state: LinkState::Disconnected,
        host: None,
        providers: None,
        error: None,
    });
    let state = format!("{:?}", status.state);
    let host = status.host.unwrap_or_else(|| "host".into());
    rsx! {
        header { class: "status", "data-state": "{state}",
            span { class: "dot" }
            span { class: "host", "{host}" }
            span { class: "state", "{state}" }
            button { class: "change-host", onclick: move |_| on_change_host.call(()), "Change host" }
            match status.error {
                Some(error) => rsx! { span { class: "error", "{error}" } },
                None => rsx! {},
            }
        }
    }
}
