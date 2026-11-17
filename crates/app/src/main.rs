//! The Conductor UI for one host: the session list, a form to start a
//! session, and each session's transcript. On a phone one screen shows at a
//! time; on a wide window the list is a sidebar. `dx serve --platform web`
//! runs it in a browser.

mod checkpoints;
mod credentials;
mod session;
mod sessions;
mod ticket;

mod ui;

pub(crate) use ui::{Link, PaneHeader, Screen, run_command, send};

fn main() {
    dioxus::launch(ui::App);
}
