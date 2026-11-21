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

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::Duration;

use crate::fleet::Fleet;
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
    fleet: Signal<Option<Fleet>>,
    fleet_versions: Signal<BTreeMap<HostId, u64>>,
}

impl Shell {
    pub(crate) fn is_paired(self, host: HostId) -> bool {
        self.state
            .read()
            .saved()
            .iter()
            .any(|saved| saved.ticket.host == host.0)
    }

    pub(crate) fn host_link(self, host: HostId) -> Option<Rc<HostLink>> {
        self.links
            .read()
            .get(&host)
            .map(|(_, link)| Rc::clone(link))
    }

    fn set_fleet(self, next: Fleet) {
        let mut removed = self.retired_members(&next);
        let stale = self
            .state
            .read()
            .saved()
            .iter()
            .map(|saved| HostId::of(&saved.ticket))
            .filter(|&host| next.revoked(host))
            .collect::<Vec<_>>();
        let mut saved = next.save();
        for host in stale {
            removed.insert(host);
            saved &= ticket::forget(host);
        }
        let hosts = next.saved();
        if !saved {
            let mut failed = self.storage_failed;
            failed.set(true);
        }
        let mut fleet = self.fleet;
        fleet.set(Some(next));
        self.dispatch(Intent::FleetHosts {
            hosts,
            removed: removed.into_iter().collect(),
        });
    }

    fn retired_members(self, next: &Fleet) -> BTreeSet<HostId> {
        self.fleet
            .read()
            .iter()
            .flat_map(Fleet::hosts)
            .map(|host| HostId(host.node))
            .filter(|&id| next.host(id).is_none())
            .collect()
    }

    pub(crate) fn outdated_fleet_version(self, host_key: &str) -> Option<u64> {
        let newest = self.fleet.read().as_ref()?.snapshot.as_ref()?.version;
        self.fleet_versions
            .read()
            .iter()
            .find_map(|(host, version)| {
                (*version < newest && host.key() == host_key)
                    .then_some(*version)
            })
    }

    fn refresh_fleet(
        self,
        host: HostId,
        snapshot: proto::fleet::SignedSnapshot,
    ) {
        if net::fleet::verify_snapshot(&snapshot).is_err() {
            return;
        }
        self.note_fleet_version(host, snapshot.snapshot.version);
        let next = self
            .fleet
            .read()
            .as_ref()
            .and_then(|fleet| fleet.updated(snapshot));
        if let Some(next) = next {
            self.set_fleet(next);
        }
    }

    fn note_fleet_version(self, host: HostId, version: u64) {
        let mut versions = self.fleet_versions;
        if versions.read().get(&host) != Some(&version) {
            versions.write().insert(host, version);
        }
    }

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
                let link = self.host_link(host);
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
    let fleet = use_signal(Fleet::load);
    let fleet_versions = use_signal(BTreeMap::new);
    let shell = Shell {
        state,
        links,
        storage_failed,
        fleet,
        fleet_versions,
    };
    use_context_provider(|| shell);
    crate::push::follow_activations(shell);
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
                    FleetPair {}
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

#[component]
fn FleetPair() -> Element {
    let shell = use_context::<Shell>();
    let mut request = use_signal(String::new);
    let mut image = use_signal(String::new);
    let mut signed = use_signal(String::new);
    let mut note = use_signal(String::new);
    rsx! {
        section { class: "fleet-pair",
            button {
                onclick: move |_| {
                    spawn(async move {
                        match owner_request().await {
                            Ok(text) => {
                                image.set(crate::fleet::pairing_image(&text));
                                request.set(format!("conductord pair {text}"));
                            }
                            Err(error) => note.set(error),
                        }
                    });
                },
                "Pair with a fleet"
            }
            if !request.read().is_empty() {
                p { "Run this command on the fleet root host and paste its reply." }
                code { "{request}" }
                if !image.read().is_empty() {
                    div { role: "img", "aria-label": "Public fleet pairing request", dangerous_inner_html: "{image}" }
                }
                input {
                    r#type: "text",
                    "aria-label": "Signed fleet pairing reply",
                    autocomplete: "off",
                    value: "{signed}",
                    oninput: move |event| signed.set(event.value()),
                }
                button {
                    onclick: move |_| {
                        let text = signed.read().clone();
                        spawn(async move {
                            match owner_pairing(&text).await {
                                Ok(pairing) => shell.set_fleet(Fleet::new(pairing)),
                                Err(error) => note.set(error),
                            }
                        });
                    },
                    "Finish fleet pairing"
                }
            }
            if !note.read().is_empty() {
                p { role: "status", "{note}" }
            }
        }
    }
}

async fn owner_request() -> Result<String, String> {
    let device = net::fleet::owner_public_key()
        .await
        .map_err(|error| error.to_string())?;
    let name = net::platform::client_name().0;
    (proto::fleet::PairRequest { device, name })
        .to_text()
        .map_err(|error| error.to_string())
}

async fn owner_pairing(text: &str) -> Result<proto::fleet::Pairing, String> {
    let key = net::fleet::owner_public_key()
        .await
        .map_err(|error| error.to_string())?;
    crate::fleet::pairing(text, key).ok_or_else(|| {
        String::from("This pairing reply does not match this device.")
    })
}

#[component]
fn FleetAdd() -> Element {
    let shell = use_context::<Shell>();
    let mut invite = use_signal(String::new);
    let mut failure = use_signal(String::new);
    let root = shell
        .fleet
        .read()
        .as_ref()
        .map(|fleet| HostId(fleet.pairing.root.node));
    root.map_or_else(|| rsx! { FleetPair {} }, |host| rsx! {
            section { class: "fleet-pair",
                button {
                    onclick: move |_| {
                        let link = shell.host_link(host);
                        spawn(async move {
                            match fleet_invite(send(link, CommandBody::FleetInvite).await) {
                                Ok(text) => {
                                    failure.set(String::new());
                                    invite.set(format!("conductord join {text}"));
                                }
                                Err(error) => failure.set(error),
                            }
                        });
                    },
                    "Create fleet host invitation"
                }
                if !invite.read().is_empty() {
                    p { "Run this command on the host to join. The invitation expires after one hour." }
                    code { "{invite}" }
                }
                FleetEnroll { host }
                if !failure.read().is_empty() {
                    p { role: "status", "{failure}" }
                }
            }
        })
}

fn fleet_invite(
    response: Result<CommandResult, String>,
) -> Result<String, String> {
    let Ok(CommandResult::FleetInvite(ticket)) = response else {
        return Err(String::from("Fleet changes need the root host online."));
    };
    ticket.to_text().map_err(|error| error.to_string())
}

#[component]
fn FleetEnroll(host: HostId) -> Element {
    rsx! {
        FleetAdmit { host }
        FleetApprove { host }
    }
}

#[component]
fn FleetAdmit(host: HostId) -> Element {
    let shell = use_context::<Shell>();
    let mut standalone = use_signal(String::new);
    let mut name = use_signal(String::new);
    let mut result = use_signal(String::new);
    rsx! {
        section { class: "fleet-pair",
            label {
                "Standalone host join link"
                input { value: "{standalone}", autocomplete: "off", oninput: move |event| standalone.set(event.value()) }
            }
            label {
                "Host name"
                input { value: "{name}", oninput: move |event| name.set(event.value()) }
            }
            button {
                onclick: move |_| {
                    let Ok(ticket) = ticket::parse_join(&standalone.read()) else {
                        result.set(String::from("Enter the standalone host's join link."));
                        return;
                    };
                    let host_name = name.read().trim().to_owned();
                    let link = shell.host_link(host);
                    spawn(async move {
                        match send(link, CommandBody::FleetAdmit { ticket, name: host_name }).await {
                            Ok(CommandResult::Accepted) => {
                                standalone.set(String::new());
                                result.set(String::from("The host joined the fleet."));
                            }
                            _ => result.set(String::from("The root could not admit the host. Check that both hosts are online and the join link is current.")),
                        }
                    });
                },
                "Admit standalone host"
            }
            FleetNote { text: result() }
        }
    }
}

#[component]
fn FleetApprove(host: HostId) -> Element {
    let shell = use_context::<Shell>();
    let mut device = use_signal(String::new);
    let mut result = use_signal(String::new);
    let mut reply = use_signal(String::new);
    rsx! {
        section { class: "fleet-pair",
            label {
                "New browser pairing request"
                input { value: "{device}", autocomplete: "off", oninput: move |event| device.set(event.value()) }
            }
            button {
                onclick: move |_| {
                    let text = device.read().clone();
                    let raw = text.trim().strip_prefix("conductord pair ").unwrap_or_else(|| text.trim());
                    let Ok(request) = proto::fleet::PairRequest::parse_text(raw) else {
                        result.set(String::from("Enter the pairing request shown by the new browser."));
                        return;
                    };
                    let link = shell.host_link(host);
                    spawn(async move {
                        match approve_browser(send(link, CommandBody::FleetApprove(request)).await) {
                            Ok(text) => {
                                result.set(String::from("Paste the signed reply into the new browser."));
                                reply.set(text);
                            }
                            Err(error) => result.set(error),
                        }
                    });
                },
                "Approve new browser"
            }
            if !reply.read().is_empty() {
                code { "{reply}" }
            }
            FleetNote { text: result() }
        }
    }
}

#[component]
fn FleetNote(text: String) -> Element {
    rsx! {
        if !text.is_empty() {
            p { role: "status", "{text}" }
        }
    }
}

fn approve_browser(
    response: Result<CommandResult, String>,
) -> Result<String, String> {
    let Ok(CommandResult::FleetPairing(pairing)) = response else {
        return Err(String::from(
            "Approving a browser requires the root host online.",
        ));
    };
    pairing.to_text().map_err(|error| error.to_string())
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
                                div { class: "new-session",
                                    FleetAdd {}
                                    Pair { pair }
                                }
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
            let identity = shell.fleet.read().as_ref().and_then(|fleet| {
                fleet.host(host).map(|entry| {
                    (entry.clone(), fleet.pairing.certificate.clone())
                })
            });
            let started = match identity {
                Some((entry, certificate)) => {
                    HostLink::spawn_fleet(&entry, certificate, options).await
                }
                None => HostLink::start(ticket, options).await,
            };
            started.map_err(|error| {
                (
                    !matches!(
                        error,
                        StartError::Ticket(_) | StartError::Fleet(_)
                    ),
                    error.to_string(),
                )
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
    let handle = Rc::new(handle);
    links
        .write()
        .insert(dial.host, (dial.key, Rc::clone(&handle)));
    let fleet = shell
        .fleet
        .read()
        .as_ref()
        .is_some_and(|fleet| fleet.host(dial.host).is_some());
    if fleet {
        watch_fleet(dial.host, Rc::clone(&handle), shell);
    }
    while let Some(update) = updates.recv().await {
        shell.dispatch(Intent::Link(dial.host, update));
    }
}

fn watch_fleet(host: HostId, member: Rc<HostLink>, shell: Shell) {
    spawn(async move {
        loop {
            if member.status().state == LinkState::Live {
                let snapshot = member.send(CommandBody::FleetStatus).await.ok();
                report_fleet_status(host, snapshot, shell);
            }
            n0_future::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

fn report_fleet_status(
    host: HostId,
    reply: Option<CommandResult>,
    shell: Shell,
) {
    if let Some(CommandResult::FleetStatus(snapshot)) = reply {
        shell.refresh_fleet(host, snapshot);
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
