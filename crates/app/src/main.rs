//! The Conductor UI over every paired host: the session list by host, a
//! form to start a session on one, and each session's transcript. The `ui`
//! crate describes every screen; these components only draw it. On a phone
//! one screen shows at a time; on a wide window the list is a sidebar.
//! `dx serve --platform web` runs it in a browser.

mod checkpoints;
mod credentials;
mod fleet;
mod issue;
mod projects;
mod push;
mod session;
mod sessions;
mod shell;
mod ticket;

use shell::{PaneHeader, Shell};

fn main() {
    dioxus::launch(shell::App);
}
