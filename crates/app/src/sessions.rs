//! The session list and the form that starts a session.

use client::host_view::{Attention, HostView, SessionView};
use client::new_session::{
    Attempt, PROVIDERS, ProviderNote, Refusal, effort_picks, kept_effort, label, model_picks,
    models, note,
};
use dioxus::prelude::*;
use proto::{
    Availability, CommandBody, CommandResult, Effort, HostPath, ModelId, Provider, Providers,
    SessionId,
};

use crate::{Link, PaneHeader, Screen, send};

/// The last segment of a host path, which names the project. The host's OS
/// may separate with either slash.
pub(crate) fn short(path: &HostPath) -> &str {
    path.0
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(&path.0)
}

#[component]
pub(crate) fn SessionList(
    view: Signal<HostView>,
    screen: Screen,
    on_select: EventHandler<Screen>,
) -> Element {
    let host = view.read();
    let rows = host
        .recent()
        .into_iter()
        .filter_map(|id| host.sessions.get(&id).cloned().map(|session| (id, session)))
        .collect::<Vec<(SessionId, SessionView)>>();
    rsx! {
        nav { class: "sidebar",
            header { class: "list-head",
                h1 { "Sessions" }
                button {
                    class: "new",
                    "aria-current": screen == Screen::NewSession,
                    onclick: move |_| on_select(Screen::NewSession),
                    "New session"
                }
            }
            match rows.first() {
                Some(_) => rsx! {
                    ul { class: "session-list",
                        for (id, session) in rows {
                            SessionRow { key: "{id.0}", id, session, screen, on_select }
                        }
                    }
                },
                None => rsx! { p { class: "list-empty", "No sessions yet. Start one in a directory on the host." } },
            }
        }
    }
}

#[component]
fn SessionRow(
    id: SessionId,
    session: SessionView,
    screen: Screen,
    on_select: EventHandler<Screen>,
) -> Element {
    let (status, text, badge) = match session.attention() {
        Attention::Quiet => (session.status.word(), session.status.word(), String::new()),
        Attention::Requests(1) => (
            String::from("approval"),
            String::from("needs you"),
            String::from("1"),
        ),
        Attention::Requests(open) => (
            String::from("approval"),
            format!("{open} requests need you"),
            format!("{open}"),
        ),
    };
    rsx! {
        li {
            button {
                class: "session-row",
                "data-status": "{status}",
                "aria-current": screen == Screen::Session(id),
                onclick: move |_| on_select(Screen::Session(id)),
                span { class: "dot", "aria-hidden": "true" }
                span { class: "row-text",
                    span { class: "row-title", "{short(&session.directory)}" }
                    span { class: "row-sub", "{label(session.provider)}, {text}" }
                }
                span { class: "badge", "{badge}" }
            }
        }
    }
}

#[component]
pub(crate) fn NewSession(
    view: Signal<HostView>,
    providers: Option<Providers>,
    link: Signal<Link>,
    on_back: EventHandler<()>,
    on_started: EventHandler<SessionId>,
) -> Element {
    let mut provider = use_signal(|| Provider::Claude);
    let mut model = use_signal(|| None::<ModelId>);
    let mut effort = use_signal(|| None::<Effort>);
    let mut directory = use_signal(String::new);
    let attempt = use_signal(|| Attempt::Editing);
    let host = view.read();
    let suggestions = host.directories();
    let (directory_error, provider_error, form_error) = match &attempt() {
        Attempt::Refused(Refusal::Directory) => (
            Some(String::from(
                "There is no directory at that path on the host.",
            )),
            None,
            None,
        ),
        Attempt::Refused(Refusal::Provider(reason)) => (None, Some(reason.clone()), None),
        Attempt::Refused(Refusal::Other(message)) => (None, None, Some(message.clone())),
        Attempt::Editing | Attempt::Starting => (None, None, None),
    };
    let provider_note = note(
        provider(),
        providers.as_ref().map(|providers| providers.of(provider())),
    );
    let offered = models(providers.as_ref().map(|providers| providers.of(provider())));
    let model_options = model_picks(&offered);
    let effort_options = effort_picks(&offered, model().as_ref());
    let start = match (attempt(), directory().trim()) {
        (Attempt::Starting, _) => Start::Busy,
        (_, "") => Start::Empty,
        (_, _) => Start::Ready,
    };
    rsx! {
        section { class: "pane new-pane",
            PaneHeader { title: "New session", subtitle: "Runs on the host, in a directory you choose.", on_back }
            form {
                class: "new-session",
                onsubmit: move |event: FormEvent| {
                    event.prevent_default();
                    let directory = HostPath(directory().trim().to_string());
                    begin(link, attempt, (provider(), model(), effort()), directory, on_started);
                },
                fieldset { class: "providers",
                    legend { "Agent" }
                    div { class: "segments",
                        for (choice, state) in PROVIDERS.iter().map(|choice| (*choice, word(providers.as_ref().map(|providers| providers.of(*choice))))) {
                            label { class: "segment", "data-available": "{state}",
                                input {
                                    r#type: "radio",
                                    name: "provider",
                                    checked: provider() == choice,
                                    // Another provider offers other models.
                                    onchange: move |_| {
                                        provider.set(choice);
                                        model.set(None);
                                        effort.set(None);
                                    },
                                }
                                span { class: "segment-name", "{label(choice)}" }
                                span { class: "segment-state", "{state}" }
                            }
                        }
                    }
                    match (provider_error, provider_note) {
                        (Some(reason), _) | (None, ProviderNote::Unavailable(reason)) => rsx! {
                            Hint { error: format!("{} can't run on this host: {reason}", label(provider())) }
                        },
                        (None, ProviderNote::WritesUnasked) => rsx! {
                            p { class: "hint muted",
                                "Codex runs commands and edits files in this folder and in /tmp without asking. It asks before writing anywhere else, changing .git, or using the network."
                            }
                        },
                        (None, ProviderNote::Nothing) => rsx! {},
                    }
                }
                div { class: "choices",
                    label { class: "field",
                        span { class: "field-label", "Model" }
                        select {
                            name: "model",
                            // An effort the new model doesn't take goes back to its default.
                            onchange: move |event| {
                                let picked = offered.find(&ModelId(event.value())).map(|choice| choice.id.clone());
                                effort.set(kept_effort(&offered, picked.as_ref(), effort()));
                                model.set(picked);
                            },
                            for pick in model_options {
                                option {
                                    value: "{pick.value.as_ref().map(|id| id.0.clone()).unwrap_or_default()}",
                                    selected: pick.value == model(),
                                    "{pick.label}"
                                }
                            }
                        }
                    }
                    label { class: "field",
                        span { class: "field-label", "Effort" }
                        select {
                            name: "effort",
                            onchange: move |event| effort.set(Effort::parse(&event.value())),
                            for pick in effort_options {
                                option {
                                    value: "{pick.value.map(|effort| String::from(effort.wire())).unwrap_or_default()}",
                                    selected: pick.value == effort(),
                                    "{pick.label}"
                                }
                            }
                        }
                    }
                }
                label { class: "field",
                    span { class: "field-label", "Directory" }
                    input {
                        r#type: "text",
                        name: "directory",
                        placeholder: "/home/you/src/project",
                        autocomplete: "off",
                        autocapitalize: "off",
                        autocorrect: "off",
                        spellcheck: "false",
                        enterkeyhint: "go",
                        "aria-invalid": directory_error.is_some(),
                        value: "{directory}",
                        oninput: move |event| directory.set(event.value()),
                    }
                    Hint { error: directory_error.clone() }
                }
                match suggestions.first() {
                    Some(_) => rsx! {
                        div { class: "suggestions",
                            p { class: "field-label", "Recent" }
                            ul {
                                for path in suggestions {
                                    li {
                                        button {
                                            r#type: "button",
                                            class: "suggestion",
                                            onclick: move |_| directory.set(path.0.clone()),
                                            span { class: "suggestion-name", "{short(&path)}" }
                                            span { class: "suggestion-path", "{path.0}" }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    None => rsx! {},
                }
                Hint { error: form_error }
                button {
                    r#type: "submit",
                    class: "start",
                    disabled: !matches!(start, Start::Ready),
                    match start {
                        Start::Busy => "Starting…",
                        Start::Empty | Start::Ready => "Start session",
                    }
                }
            }
        }
    }
}

/// Whether the form can be sent.
enum Start {
    Empty,
    Ready,
    Busy,
}

#[component]
fn Hint(error: Option<String>) -> Element {
    match error {
        Some(error) => rsx! { p { class: "hint error", role: "alert", "{error}" } },
        None => rsx! {},
    }
}

fn begin(
    link: Signal<Link>,
    mut attempt: Signal<Attempt>,
    choice: (Provider, Option<ModelId>, Option<Effort>),
    directory: HostPath,
    on_started: EventHandler<SessionId>,
) {
    let (provider, model, effort) = choice;
    attempt.set(Attempt::Starting);
    spawn(async move {
        let answer = send(
            link(),
            CommandBody::StartSession {
                provider,
                directory,
                model,
                effort,
            },
        )
        .await;
        if let Ok(CommandResult::Started { session }) = &answer {
            on_started(SessionId(session.0));
        }
        attempt.set(Attempt::answered(answer));
    });
}

/// The provider's state in one word, for the form's text and `data-available`.
fn word(availability: Option<&Availability>) -> String {
    String::from(match availability {
        Some(Availability::Available { .. }) => "available",
        Some(Availability::Unavailable { .. }) => "unavailable",
        None => "unknown",
    })
}
