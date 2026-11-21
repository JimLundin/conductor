//! The app shell: it holds the state, feeds it every intent and each
//! host's link updates through `ui::update`, and performs the effects that
//! come back. It keeps one live link per paired host.

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

use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::Duration;

use crate::sessions::SessionList;
use crate::ticket;
use dioxus::prelude::*;
use net::{HostLink, LinkOptions, LinkState, StartError, Status, Update};
use proto::{CommandBody, CommandResult};
use tokio::sync::mpsc;
use ui::{Detail, Dial, Effect, Focus, HostId, Intent, Place, Screen, State};

#[expect(
    clippy::volatile_composites,
    reason = "asset! reads its link section with read_volatile"
)]
const CSS: Asset = asset!("/assets/main.css");

/// Each host's live link, with the ticket it dialled.
type Links = BTreeMap<HostId, (String, Rc<HostLink>)>;

/// The one place that holds the state.
#[derive(Clone, Copy)]
pub struct Shell {
    state: Signal<State>,
    links: Signal<Links>,
    storage_failed: Signal<bool>,
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
            Effect::Send { host, body, reply } => {
                let link = self
                    .links
                    .read()
                    .get(&host)
                    .map(|(_, link)| Rc::clone(link));
                // Not tied to the component that sent it: a host update can
                // take that component away before the answer comes (a failed
                // sign-in drops its code form), and the answer still has to
                // land.
                dioxus_core::spawn_forever(async move {
                    let answer = send(link, body).await;
                    self.dispatch(Intent::Replied(reply, answer));
                });
            }
            Effect::PairHost(saved) => {
                let mut storage_failed = self.storage_failed;
                storage_failed.set(!ticket::pair(saved));
            }
            Effect::ForgetHost(host) => {
                let mut storage_failed = self.storage_failed;
                storage_failed.set(!ticket::forget(host));
            }
            Effect::NameHost(saved) => ticket::name(saved),
            Effect::Focus(focus) => {
                document::eval(&script(&focus));
            }
            Effect::ShowEffort(effort) => {
                document::eval(&format!(
                    "document.querySelector('select.effort-pick').value = '{effort}'"
                ));
            }
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
        // The composer can mount disabled while the agent starts, and is
        // enabled in place once it takes messages.
        Focus::Composer => String::from(
            "(() => { \
               const box = document.querySelector('.composer textarea'); \
               if (!box) return; \
               if (!box.disabled) { box.focus(); return; } \
               const watch = new MutationObserver(() => { \
                 if (!box.isConnected) watch.disconnect(); \
                 else if (!box.disabled) { watch.disconnect(); box.focus(); } \
               }); \
               watch.observe(box, { attributes: true, attributeFilter: ['disabled'] }); \
             })()",
        ),
    }
}

#[component]
pub fn App() -> Element {
    let (state, storage_failed) = use_hook(|| {
        let (opened, failed) = ticket::open();
        (Signal::new(opened), Signal::new(failed))
    });
    let links = use_signal(Links::new);
    use_context_provider(|| Shell {
        state,
        links,
        storage_failed,
    });
    let screen = ui::view(&state.read());
    rsx! {
        document::Stylesheet { href: CSS }
        if storage_failed() {
            p { class: "error storage-error", role: "alert", "Browser storage could not save the pairing change. Keep your join link to reconnect." }
        }
        match screen {
            Screen::Pair(pair) => rsx! {
                main { class: "empty",
                    h1 { "Connect to a host" }
                    p { "Paste the join link or ticket your host printed when it started." }
                    Pair { pair }
                }
            },
            Screen::Hosts(hosts) => rsx! { Hosts { hosts: *hosts } },
        }
    }
}

/// The join link form. The host prints its join link when it starts;
/// opening that link pairs Safari, and pasting it here pairs a Home Screen
/// app.
#[component]
fn Pair(pair: ui::Pair) -> Element {
    let shell = use_context::<Shell>();
    let text = pair.text.clone();
    rsx! {
        form {
            class: "pair",
            onsubmit: move |event: FormEvent| {
                event.prevent_default();
                shell.dispatch(Intent::Join(ticket::parse_join(&text)));
            },
            input {
                r#type: "text",
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

/// The screens over every paired host, and a link to each while they show.
#[component]
fn Hosts(hosts: ui::HostsScreen) -> Element {
    rsx! {
        for dial in hosts.links {
            Link { key: "{dial.key}", dial }
        }
        div { class: "app", "data-screen": hosts.shown,
            Band { add_current: hosts.add_current, credentials_current: hosts.credentials_current, projects_current: hosts.projects_current }
            div { class: "panes",
                SessionList { list: hosts.list }
                main { class: "detail",
                    match hosts.detail {
                        Detail::Placeholder(text) => rsx! {
                            div { class: "placeholder",
                                p { "{text}" }
                            }
                        },
                        Detail::Pair(pair) => rsx! {
                            section { class: "pane new-pane",
                                PaneHeader { title: "Add a host", subtitle: "Paste the join link or ticket it printed when it started." }
                                div { class: "new-session", Pair { pair } }
                            }
                        },
                        Detail::NewSession(form) => rsx! {
                            crate::sessions::NewSession { form: *form }
                        },
                        Detail::Credentials(screen) => rsx! {
                            crate::credentials::CredentialsScreen { screen: *screen }
                        },
                        Detail::Projects(screen) => rsx! {
                            crate::projects::ProjectsScreen { screen: *screen }
                        },
                        Detail::Session(pane) => rsx! {
                            crate::session::SessionPane { key: "{pane.key}", pane: *pane }
                        },
                        Detail::Issue(screen) => rsx! {
                            crate::issue::IssueScreen { key: "{screen.key}", screen: *screen }
                        },
                    }
                }
            }
        }
    }
}

/// One host's link, for as long as the host stays paired with this ticket.
#[component]
fn Link(dial: Dial) -> Element {
    let shell = use_context::<Shell>();
    let key = dial.key.clone();
    use_future(move || run_link(dial.clone(), shell));
    use_drop(move || {
        let mut links = shell.links;
        links.write().retain(|_, (dialled, _)| *dialled != key);
    });
    rsx! {}
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
fn Band(
    add_current: bool,
    credentials_current: bool,
    projects_current: bool,
) -> Element {
    let shell = use_context::<Shell>();
    rsx! {
        header { class: "band",
            span { class: "band-title", "Conductor" }
            button {
                class: "projects",
                "aria-current": projects_current,
                onclick: move |_| shell.dispatch(Intent::Show(Place::Projects)),
                "Projects"
            }
            button {
                class: "credentials",
                "aria-current": credentials_current,
                onclick: move |_| shell.dispatch(Intent::Show(Place::Credentials)),
                "Credentials"
            }
            button {
                class: "add-host",
                "aria-current": add_current,
                onclick: move |_| shell.dispatch(Intent::Show(Place::AddHost)),
                "Add host"
            }
        }
    }
}

#[expect(
    clippy::future_not_send,
    reason = "runs on the single-threaded UI executor and holds signals and an Rc'd link"
)]
async fn run_link(dial: Dial, shell: Shell) {
    let ticket = &dial.ticket;
    let host = dial.host;
    let started = retry_start(
        async || {
            let options = LinkOptions::new(net::platform::client_name());
            HostLink::start(ticket, options).await.map_err(|error| {
                (!matches!(error, StartError::Ticket(_)), error.to_string())
            })
        },
        |update| shell.dispatch(Intent::Link(host, update)),
        async || n0_future::time::sleep(Duration::from_secs(2)).await,
    )
    .await;
    if let Some((handle, updates)) = started {
        follow(dial, handle, updates, shell).await;
    }
}

async fn retry_start<T>(
    mut start: impl AsyncFnMut() -> Result<T, (bool, String)>,
    mut report: impl FnMut(Update),
    mut wait: impl AsyncFnMut(),
) -> Option<T> {
    loop {
        match start().await {
            Ok(link) => return Some(link),
            Err((retry, failure)) => {
                report(startup_failure(&failure));
                if !retry {
                    return None;
                }
                wait().await;
            }
        }
    }
}

fn startup_failure(error: &str) -> Update {
    Update::Status(Status {
        state: LinkState::Disconnected,
        error: Some(format!("Could not start the host link: {error}")),
    })
}

#[expect(
    clippy::future_not_send,
    reason = "runs on the single-threaded UI executor and holds signals"
)]
async fn follow(
    dial: Dial,
    handle: HostLink,
    mut updates: mpsc::Receiver<Update>,
    shell: Shell,
) {
    let mut links = shell.links;
    links.write().insert(dial.host, (dial.key, Rc::new(handle)));
    while let Some(update) = updates.recv().await {
        shell.dispatch(Intent::Link(dial.host, update));
    }
}

#[expect(
    clippy::future_not_send,
    reason = "runs on the single-threaded UI executor and holds the Rc'd link"
)]
async fn send(
    link: Option<Rc<HostLink>>,
    body: CommandBody,
) -> Result<CommandResult, String> {
    match link {
        Some(host) => host.send(body).await.map_err(|err| err.to_string()),
        None => Err(String::from("Not connected to this host yet.")),
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    use super::{LinkState, Status, Update, retry_start};

    #[test]
    fn a_transient_startup_failure_is_reported_before_the_next_attempt() {
        let mut attempts = 0;
        let (report, mut updates) = tokio::sync::mpsc::unbounded_channel();
        let (finish_waiting, mut waiting) = tokio::sync::oneshot::channel();
        let ready = {
            let mut started = std::pin::pin!(retry_start(
                async || {
                    attempts += 1;
                    if attempts == 1 {
                        Err((true, String::from("could not bind")))
                    } else {
                        Ok("link")
                    }
                },
                |update| {
                    let _ = report.send(update);
                },
                async || {
                    let _ = (&mut waiting).await;
                },
            ));
            let mut context = Context::from_waker(Waker::noop());
            assert_eq!(started.as_mut().poll(&mut context), Poll::Pending);
            assert_eq!(
                updates.try_recv(),
                Ok(Update::Status(Status {
                    state: LinkState::Disconnected,
                    error: Some(String::from(
                        "Could not start the host link: could not bind"
                    )),
                }))
            );
            assert_eq!(finish_waiting.send(()), Ok(()));
            started.as_mut().poll(&mut context)
        };
        assert_eq!(ready, Poll::Ready(Some("link")));
        assert_eq!(attempts, 2);
    }
}
