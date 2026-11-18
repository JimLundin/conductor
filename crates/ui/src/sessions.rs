//! The session list: a group per host, headed by its name and link
//! state, each with its sessions, the most recently active first, and what
//! each waits on.

use alloc::collections::BTreeMap;

use crate::prelude::*;
use client::host_view::{Attention, SessionView, UndoState};
use proto::{Catalog, HostPath, Instances, Project, SessionId};

use crate::Place;
use crate::host::{Host, HostId, StatusLine, by_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionList {
    /// The new-session form is shown.
    pub new_current: bool,
    pub groups: Vec<HostGroup>,
}

/// One host's sessions, under its status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostGroup {
    pub host: HostId,
    pub key: String,
    pub status: StatusLine,
    pub listing: Listing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    Empty(String),
    Rows(Vec<SessionRow>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub host: HostId,
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

pub fn list(hosts: &BTreeMap<HostId, Host>, place: Place) -> SessionList {
    SessionList {
        new_current: place == Place::NewSession,
        groups: by_name(hosts)
            .into_iter()
            .map(|(id, host)| group(id, host, place))
            .collect(),
    }
}

fn group(id: HostId, host: &Host, place: Place) -> HostGroup {
    let catalog = host.catalog();
    let known = (host.instances(), &catalog);
    let rows = host
        .view
        .recent()
        .into_iter()
        .filter_map(|session| {
            host.view.sessions.get(&session).map(|shown| {
                row(
                    (id, session),
                    shown,
                    place,
                    (known, host.project_of(shown)),
                )
            })
        })
        .collect::<Vec<SessionRow>>();
    HostGroup {
        host: id,
        key: id.key(),
        status: host.status_line(),
        listing: match rows.first() {
            Some(_) => Listing::Rows(rows),
            None => {
                Listing::Empty(format!("No sessions on {} yet.", host.name()))
            }
        },
    }
}

/// A session's row, titled by its project, else its directory.
fn row(
    (host, id): (HostId, SessionId),
    session: &SessionView,
    place: Place,
    (known, project): ((Option<&Instances>, &Catalog), Option<&Project>),
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
        host,
        id,
        key: format!("{}", id.0),
        status,
        current: place == Place::Session(host, id),
        title: project.map_or_else(
            || short(&session.directory).to_owned(),
            |known| known.name.clone(),
        ),
        sub: format!("{}, {text}", agent(session, known)),
        branch: session
            .workspace
            .as_ref()
            .map(|workspace| workspace.branch.clone()),
        badge,
    }
}

/// What the app calls the session's agent: its instance's label, or its
/// provider's once the host no longer has the instance.
pub fn agent(
    session: &SessionView,
    known: (Option<&Instances>, &Catalog),
) -> String {
    let (instances, catalog) = known;
    instances
        .and_then(|listed| listed.of(&session.instance))
        .map_or_else(
            || catalog.label(session.provider),
            |status| status.label.clone(),
        )
}

/// The last segment of a host path, which names the project. The host's OS
/// may separate with either slash.
pub fn short(path: &HostPath) -> &str {
    path.0
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(&path.0)
}
