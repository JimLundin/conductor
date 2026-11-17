//! The app shell: pairing, one host's screens and the link that feeds them.

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

use std::rc::Rc;

use crate::ticket::{self, Opening};
use client::host_view::HostView;
use dioxus::prelude::*;
use net::{HostLink, LinkOptions, LinkState, Status, Update};
use proto::{CommandBody, CommandResult, SessionId, Ticket};
use tokio::sync::mpsc;

#[expect(
    clippy::volatile_composites,
    reason = "asset! reads its link section with read_volatile"
)]
const CSS: Asset = asset!("/assets/main.css");

/// What fills the detail pane, and on a phone the whole window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Sessions,
    NewSession,
    Credentials,
    Session(SessionId),
}

/// Where commands go.
#[derive(Clone)]
pub enum Link {
    Waiting,
    Live(Rc<HostLink>),
}

#[component]
pub fn App() -> Element {
    let initial = use_hook(ticket::initial);
    let mut opening = use_signal(|| {
        initial.as_ref().ok().cloned().unwrap_or(Opening::Unpaired)
    });
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
    let mut shown_error = use_signal(move || error);
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
                        Err(err) => shown_error.set(Some(format!("That join link is not valid: {err}"))),
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
            {shown_error().map_or_else(|| rsx! {}, |message| rsx! { p { class: "error", "{message}" } })}
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
    let selected = match screen() {
        Screen::Session(id) => view.read().sessions.get(&id).cloned(),
        Screen::Sessions | Screen::NewSession | Screen::Credentials => None,
    };
    // A session the host no longer reports (after a reset) falls back to the
    // list, which on a phone is the only way out of the detail pane.
    let shown = match (screen(), &selected) {
        (Screen::NewSession | Screen::Credentials, _)
        | (Screen::Session(_), Some(_)) => "detail",
        (Screen::Sessions, _) | (Screen::Session(_), None) => "list",
    };
    rsx! {
        div { class: "app", "data-screen": shown,
            StatusLine {
                status: status(),
                on_credentials: move |()| screen.set(Screen::Credentials),
                on_change_host,
            }
            div { class: "panes",
                crate::sessions::SessionList { view, screen: screen(), on_select: move |next| screen.set(next) }
                main { class: "detail",
                    match (screen(), selected) {
                        (Screen::NewSession, _) => rsx! {
                            crate::sessions::NewSession {
                                view,
                                providers: status().and_then(|current| current.providers),
                                link,
                                on_back: move |()| screen.set(Screen::Sessions),
                                on_started: move |id| screen.set(Screen::Session(id)),
                            }
                        },
                        (Screen::Credentials, _) => rsx! {
                            crate::credentials::CredentialsPane {
                                host: status().and_then(|current| current.host).unwrap_or_else(|| String::from("the host")),
                                providers: status().and_then(|current| current.providers),
                                link,
                                on_back: move |()| screen.set(Screen::Sessions),
                            }
                        },
                        (Screen::Session(id), Some(session)) => rsx! {
                            crate::session::SessionPane {
                                key: "{id.0}",
                                id,
                                session,
                                providers: status().and_then(|current| current.providers),
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
pub fn PaneHeader(
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

#[expect(
    clippy::future_not_send,
    reason = "runs on the single-threaded UI executor and holds signals and an Rc'd link"
)]
async fn run_link(
    ticket: Ticket,
    status: Signal<Option<Status>>,
    view: Signal<HostView>,
    link: Signal<Link>,
) {
    let options = LinkOptions::new(net::platform::client_name());
    let started = HostLink::start(&ticket, options).await;
    follow(started.ok(), link, status, view).await;
}

#[expect(
    clippy::future_not_send,
    reason = "runs on the single-threaded UI executor and holds signals"
)]
async fn follow(
    started: Option<(HostLink, mpsc::Receiver<Update>)>,
    mut link: Signal<Link>,
    mut status: Signal<Option<Status>>,
    mut view: Signal<HostView>,
) {
    let Some((handle, mut updates)) = started else {
        return;
    };
    link.set(Link::Live(Rc::new(handle)));
    while let Some(update) = updates.recv().await {
        let (next_status, next_view) =
            net::shown_after(status(), view.replace(HostView::empty()), update);
        status.set(next_status);
        view.set(next_view);
    }
}

#[expect(
    clippy::future_not_send,
    reason = "runs on the single-threaded UI executor and holds the Rc'd link"
)]
pub async fn send(
    link: Link,
    body: CommandBody,
) -> Result<CommandResult, String> {
    match link {
        Link::Live(host) => {
            host.send(body).await.map_err(|err| err.to_string())
        }
        Link::Waiting => Err(String::from("Not connected to the host yet.")),
    }
}

/// Sends a command whose only answer worth showing is a refusal.
pub fn run_command(
    link: Signal<Link>,
    mut notice: Signal<Option<String>>,
    body: CommandBody,
) {
    spawn(async move {
        notice.set(match send(link(), body).await {
            Ok(
                CommandResult::Accepted
                | CommandResult::Started { .. }
                | CommandResult::Providers(_)
                | CommandResult::Diff(_),
            ) => None,
            Ok(CommandResult::Rejected(rejection)) => Some(rejection.label()),
            Err(err) => Some(err),
        });
    });
}

#[component]
fn StatusLine(
    status: Option<Status>,
    on_credentials: EventHandler<()>,
    on_change_host: EventHandler<()>,
) -> Element {
    let shown = status.unwrap_or(Status {
        state: LinkState::Disconnected,
        host: None,
        providers: None,
        error: None,
    });
    let state = format!("{:?}", shown.state);
    let host = shown.host.unwrap_or_else(|| "host".into());
    rsx! {
        header { class: "status", "data-state": "{state}",
            span { class: "dot" }
            span { class: "host", "{host}" }
            span { class: "state", "{state}" }
            button { class: "credentials", onclick: move |_| on_credentials.call(()), "Credentials" }
            button { class: "change-host", onclick: move |_| on_change_host.call(()), "Change host" }
            {shown.error.map_or_else(|| rsx! {}, |error| rsx! { span { class: "error", "{error}" } })}
        }
    }
}
