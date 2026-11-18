//! The projects view: each host's home, then each project with where it is
//! checked out on each host.

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

use dioxus::prelude::*;
use ui::Intent;

use crate::{PaneHeader, Shell};

#[component]
pub fn ProjectsScreen(screen: ui::ProjectsScreen) -> Element {
    let shell = use_context::<Shell>();
    rsx! {
        section { class: "pane new-pane projects-pane",
            PaneHeader { title: "Projects", subtitle: "{screen.subtitle}" }
            div { class: "notices", role: "alert",
                if let Some(text) = screen.notice {
                    button { class: "notice", onclick: move |_| shell.dispatch(Intent::DismissNotice), "{text}" }
                }
            }
            div { class: "new-session",
                ul { class: "homes",
                    for line in screen.homes {
                        HomeLine { key: "{line.key}", line }
                    }
                }
                if let Some(text) = screen.empty {
                    p { class: "hint muted list-empty", "{text}" }
                }
                for project in screen.projects {
                    Project { key: "{project.key}", project }
                }
            }
        }
    }
}

#[component]
fn HomeLine(line: ui::HomeLine) -> Element {
    let shell = use_context::<Shell>();
    rsx! {
        li { class: "home-line", "data-sync": line.state,
            div {
                span { class: "home-host", "{line.host}" }
                p { class: "home-state", "{line.text}" }
            }
            if let Some(intent) = line.sync {
                button {
                    r#type: "button",
                    class: "clear sync",
                    "aria-label": "Sync {line.host}'s home",
                    onclick: move |_| shell.dispatch(intent.clone()),
                    "Sync"
                }
            }
        }
    }
}

#[component]
fn Project(project: ui::ProjectGroup) -> Element {
    rsx! {
        section { class: "project", "data-project": "{project.key}",
            h2 { "{project.name}" }
            p { class: "hint muted", "{project.detail}" }
            ul { class: "checkouts",
                for row in project.hosts {
                    Checkout { key: "{row.key}", row }
                }
            }
        }
    }
}

#[component]
fn Checkout(row: ui::CheckoutRow) -> Element {
    let shell = use_context::<Shell>();
    rsx! {
        li { class: "checkout", "data-checkout": row.state,
            span { class: "checkout-host", "{row.host}" }
            for path in row.paths {
                code { class: "checkout-path", "{path}" }
            }
            if let Some(text) = row.text {
                span { class: "checkout-state", "{text}" }
            }
            if let Some(button) = row.clone {
                button {
                    r#type: "button",
                    class: "clear clone",
                    "aria-label": "{button.label}",
                    onclick: move |_| shell.dispatch(button.intent.clone()),
                    "Clone here"
                }
            }
        }
    }
}
