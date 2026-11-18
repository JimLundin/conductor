//! Each paired host: its ticket, the name it last gave, its link's status
//! and what its events fold into.

use alloc::collections::BTreeMap;

use crate::prelude::*;
use client::host_view::{HostView, SessionView};
use client::link::{LinkState, Status, Update, shown_after};
use data_encoding::HEXLOWER;
use proto::{
    Catalog, Home, InstanceId, Instances, Project, SignInStep, Ticket,
};

/// A host's endpoint id, which keys it everywhere the app keeps hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct HostId(pub [u8; 32]);

impl HostId {
    #[must_use]
    pub const fn of(ticket: &Ticket) -> Self {
        Self(ticket.host)
    }

    /// The id in hex, which keys the host in storage and in the page.
    #[must_use]
    pub fn key(&self) -> String {
        HEXLOWER.encode(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub ticket: Ticket,
    /// The name from its last `Welcome`, kept across launches.
    pub saved: Option<String>,
    pub status: Option<Status>,
    pub view: HostView,
}

/// The hosts in the order the app lists them: by name, then by id.
pub fn by_name(hosts: &BTreeMap<HostId, Host>) -> Vec<(HostId, &Host)> {
    let mut listed = hosts
        .iter()
        .map(|(id, host)| (*id, host))
        .collect::<Vec<(HostId, &Host)>>();
    listed.sort_by_cached_key(|(id, host)| (host.name(), *id));
    listed
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusLine {
    /// The link's state, also its `data-state`.
    pub state: String,
    pub host: String,
    pub error: Option<String>,
}

impl Host {
    pub const fn new(ticket: Ticket, saved: Option<String>) -> Self {
        Self {
            ticket,
            saved,
            status: None,
            view: HostView::empty(),
        }
    }

    /// The host after `update`, and whether it gave a name the app hasn't
    /// saved.
    pub fn linked(self, update: Update) -> (Self, bool) {
        let (status, view) = shown_after(self.status, self.view, update);
        let named = status.as_ref().and_then(|known| known.host.clone());
        let saved = named.or_else(|| self.saved.clone());
        let renamed = saved != self.saved;
        (
            Self {
                ticket: self.ticket,
                saved,
                status,
                view,
            },
            renamed,
        )
    }

    /// What the app calls the host: the name it gave, else a short form of
    /// its id.
    pub fn name(&self) -> String {
        self.saved.clone().unwrap_or_else(|| {
            let key = HostId::of(&self.ticket).key();
            format!("host {}", key.get(..8).unwrap_or(&key))
        })
    }

    pub fn live(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|known| known.state == LinkState::Live)
    }

    pub fn instances(&self) -> Option<&Instances> {
        self.status
            .as_ref()
            .and_then(|known| known.instances.as_ref())
    }

    /// How each provider presents itself, once the host has said.
    pub fn catalog(&self) -> Catalog {
        self.status
            .as_ref()
            .map_or_default(|known| known.catalog.clone())
    }

    /// Its home and projects, once it has said; none on a host with no
    /// home.
    pub fn home(&self) -> Option<&Home> {
        self.status.as_ref().and_then(|known| known.home.as_ref())
    }

    /// The project a session runs in: the one with a checkout at its
    /// worktree's repository.
    pub fn project_of(&self, session: &SessionView) -> Option<&Project> {
        let repository = &session.workspace.as_ref()?.repository;
        self.home()?.projects.iter().find(|project| {
            project
                .checkouts
                .iter()
                .any(|checkout| &checkout.path == repository)
        })
    }

    pub fn sign_ins(&self) -> BTreeMap<InstanceId, SignInStep> {
        self.status
            .as_ref()
            .map_or_default(|known| known.sign_ins.clone())
    }

    pub fn status_line(&self) -> StatusLine {
        let shown = self.status.clone().unwrap_or_default();
        StatusLine {
            state: format!("{:?}", shown.state),
            host: self.name(),
            error: shown.error,
        }
    }
}
