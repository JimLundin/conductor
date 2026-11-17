//! The app shell: it holds the state, feeds it every intent and link update
//! through `ui::update`, and performs the effects that come back.

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

use crate::sessions::SessionList;
use crate::ticket;
use dioxus::prelude::*;
use net::{HostLink, LinkOptions, Update};
use proto::{CommandBody, CommandResult, Ticket};
use tokio::sync::mpsc;
use ui::{Detail, Effect, Focus, Intent, Place, Screen, State};

#[expect(
    clippy::volatile_composites,
    reason = "asset! reads its link section with read_volatile"
)]
const CSS: Asset = asset!("/assets/main.css");

/// Where commands go.
#[derive(Clone)]
enum Link {
    Waiting,
    Live(Rc<HostLink>),
}

/// The one place that holds the state.
#[derive(Clone, Copy)]
pub struct Shell {
    state: Signal<State>,
    link: Signal<Link>,
}

impl Shell {
    pub fn dispatch(self, intent: Intent) {
        let mut state = self.state;
        let (next, effects) =
            ui::update(state.replace(State::default()), intent);
        state.set(next);
        for effect in effects {
            self.perform(effect);
        }
    }

    fn perform(self, effect: Effect) {
        match effect {
            Effect::Send { body, reply } => {
                spawn(async move {
                    let answer = send(self.link.cloned(), body).await;
                    self.dispatch(Intent::Replied(reply, answer));
                });
            }
            Effect::SaveTicket(ticket) => ticket::save(&ticket),
            Effect::ForgetTicket => ticket::forget(),
            Effect::Focus(focus) => {
                document::eval(&script(&focus));
            }
            Effect::ShowEffort(effort) => {
                document::eval(&format!(
                    "document.querySelector('select.effort-pick').value = '{effort}'"
                ));
            }
            Effect::Log(status) => tracing::info!(
                state = ?status.state,
                error = ?status.error,
                "link status"
            ),
        }
    }
}

fn script(focus: &Focus) -> String {
    match focus {
        Focus::Element(id) => {
            format!("document.getElementById('{id}').focus()")
        }
        Focus::NextFrame(id) => format!(
            "requestAnimationFrame(() => document.getElementById('{id}')?.focus())"
        ),
        Focus::AfterUndo => String::from(
            "requestAnimationFrame(() => (document.querySelector('.composer textarea:not(:disabled)') || document.querySelector('.undo-status'))?.focus())",
        ),
        Focus::Composer => {
            String::from("document.querySelector('.composer textarea').focus()")
        }
    }
}

#[component]
pub fn App() -> Element {
    let state = use_signal(|| State::new(ticket::initial()));
    let link = use_signal(|| Link::Waiting);
    use_context_provider(|| Shell { state, link });
    let screen = ui::view(&state.read());
    rsx! {
        document::Stylesheet { href: CSS }
        match screen {
            Screen::Pair(pair) => rsx! { Pair { pair } },
            Screen::Host(host) => rsx! { Host { host: *host } },
        }
    }
}

/// Shown until a host is known. The host prints its join link when it starts;
/// opening that link pairs Safari, and pasting it here pairs a Home Screen app.
#[component]
fn Pair(pair: ui::Pair) -> Element {
    let shell = use_context::<Shell>();
    let text = pair.text.clone();
    rsx! {
        main { class: "empty",
            h1 { "Connect to a host" }
            p { "Paste the join link your host printed when it started." }
            form {
                class: "pair",
                onsubmit: move |event: FormEvent| {
                    event.prevent_default();
                    shell.dispatch(Intent::Join(ticket::parse_join(&text)));
                },
                input {
                    r#type: "url",
                    placeholder: "https://…#join=…",
                    autocomplete: "off",
                    value: "{pair.text}",
                    oninput: move |event| shell.dispatch(Intent::TypeJoin(event.value())),
                }
                button { r#type: "submit", disabled: pair.connect_disabled, "Connect" }
            }
            if let Some(message) = pair.error {
                p { class: "error", "{message}" }
            }
        }
    }
}

/// The screens over one host, and the link that feeds them while they show.
#[component]
fn Host(host: ui::HostScreen) -> Element {
    let shell = use_context::<Shell>();
    let ticket = host.ticket.clone();
    use_future(move || run_link(ticket.clone(), shell));
    use_drop(move || {
        let mut link = shell.link;
        link.set(Link::Waiting);
    });
    rsx! {
        div { class: "app", "data-screen": host.shown,
            StatusLine { status: host.status }
            div { class: "panes",
                SessionList { list: host.list }
                main { class: "detail",
                    match host.detail {
                        Detail::Placeholder(text) => rsx! {
                            div { class: "placeholder",
                                p { "{text}" }
                            }
                        },
                        Detail::NewSession(form) => rsx! {
                            crate::sessions::NewSession { form: *form }
                        },
                        Detail::Credentials(pane) => rsx! {
                            crate::credentials::CredentialsPane { pane: *pane }
                        },
                        Detail::Session(pane) => rsx! {
                            crate::session::SessionPane { key: "{pane.key}", pane: *pane }
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
    children: Element,
) -> Element {
    let shell = use_context::<Shell>();
    rsx! {
        header { class: "pane-head",
            button {
                class: "back",
                "aria-label": "Sessions",
                onclick: move |_| shell.dispatch(Intent::Show(Place::Sessions)),
            }
            div { class: "pane-title",
                h2 { "{title}" }
                p { "{subtitle}" }
            }
            {children}
        }
    }
}

#[component]
fn StatusLine(status: ui::StatusLine) -> Element {
    let shell = use_context::<Shell>();
    rsx! {
        header { class: "status", "data-state": "{status.state}",
            span { class: "dot" }
            span { class: "host", "{status.host}" }
            span { class: "state", "{status.state}" }
            button {
                class: "credentials",
                onclick: move |_| shell.dispatch(Intent::Show(Place::Credentials)),
                "Credentials"
            }
            button {
                class: "change-host",
                onclick: move |_| shell.dispatch(Intent::ChangeHost),
                "Change host"
            }
            if let Some(error) = status.error {
                span { class: "error", "{error}" }
            }
        }
    }
}

#[expect(
    clippy::future_not_send,
    reason = "runs on the single-threaded UI executor and holds signals and an Rc'd link"
)]
async fn run_link(ticket: Ticket, shell: Shell) {
    let options = LinkOptions::new(net::platform::client_name());
    let started = HostLink::start(&ticket, options).await;
    follow(started.ok(), shell).await;
}

#[expect(
    clippy::future_not_send,
    reason = "runs on the single-threaded UI executor and holds signals"
)]
async fn follow(
    started: Option<(HostLink, mpsc::Receiver<Update>)>,
    shell: Shell,
) {
    let Some((handle, mut updates)) = started else {
        return;
    };
    let mut link = shell.link;
    link.set(Link::Live(Rc::new(handle)));
    while let Some(update) = updates.recv().await {
        shell.dispatch(Intent::Link(update));
    }
}

#[expect(
    clippy::future_not_send,
    reason = "runs on the single-threaded UI executor and holds the Rc'd link"
)]
async fn send(link: Link, body: CommandBody) -> Result<CommandResult, String> {
    match link {
        Link::Live(host) => {
            host.send(body).await.map_err(|err| err.to_string())
        }
        Link::Waiting => Err(String::from("Not connected to the host yet.")),
    }
}
