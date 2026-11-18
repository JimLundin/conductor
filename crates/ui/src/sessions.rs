//! The session list, the most recently active first, with what each
//! session waits on.

use crate::prelude::*;
use client::host_view::{Attention, HostView, SessionView, UndoState};
use proto::{Catalog, HostPath, SessionId};

use crate::Place;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionList {
    /// The new-session form is shown.
    pub new_current: bool,
    pub listing: Listing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    Empty(&'static str),
    Rows(Vec<SessionRow>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub id: SessionId,
    pub key: String,
    /// The row's `data-status`, which colours its dot.
    pub status: String,
    pub current: bool,
    pub title: String,
    pub sub: String,
    pub branch: Option<String>,
    /// How many requests wait on a person, empty when none do.
    pub badge: String,
}

pub fn list(view: &HostView, place: Place, catalog: &Catalog) -> SessionList {
    let rows = view
        .recent()
        .into_iter()
        .filter_map(|id| {
            view.sessions
                .get(&id)
                .map(|session| row(id, session, place, catalog))
        })
        .collect::<Vec<SessionRow>>();
    SessionList {
        new_current: place == Place::NewSession,
        listing: match rows.first() {
            Some(_) => Listing::Rows(rows),
            None => Listing::Empty(
                "No sessions yet. Start one in a directory on the host.",
            ),
        },
    }
}

fn row(
    id: SessionId,
    session: &SessionView,
    place: Place,
    catalog: &Catalog,
) -> SessionRow {
    let (status, text, badge) = match (&session.undo, session.attention()) {
        (UndoState::Undoing(_), _) => (
            String::from("undoing"),
            String::from("undoing"),
            String::new(),
        ),
        (UndoState::Failed { .. }, _) => (
            String::from("recovering"),
            String::from("recovery needed"),
            String::new(),
        ),
        (_, Attention::Quiet) => {
            (session.status.word(), session.status.word(), String::new())
        }
        (_, Attention::Requests(1)) => (
            String::from("approval"),
            String::from("needs you"),
            String::from("1"),
        ),
        (_, Attention::Requests(open)) => (
            String::from("approval"),
            format!("{open} requests need you"),
            format!("{open}"),
        ),
    };
    SessionRow {
        id,
        key: format!("{}", id.0),
        status,
        current: place == Place::Session(id),
        title: short(&session.directory).to_owned(),
        sub: format!("{}, {text}", catalog.label(session.provider)),
        branch: session
            .workspace
            .as_ref()
            .map(|workspace| workspace.branch.clone()),
        badge,
    }
}

/// The last segment of a host path, which names the project. The host's OS
/// may separate with either slash.
pub fn short(path: &HostPath) -> &str {
    path.0
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(&path.0)
}
