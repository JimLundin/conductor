//! Each paired host: its ticket, the name it last gave, its link's status
//! and what its events fold into.

use alloc::collections::BTreeMap;

use crate::prelude::*;
use client::host_fold::HostFold;
use client::host_view::SessionView;
use client::link::{LinkState, Update};
use data_encoding::HEXLOWER;
use proto::{Project, Ticket};

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
    pub fold: HostFold,
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
    pub fn new(ticket: Ticket, saved: Option<String>) -> Self {
        Self {
            ticket,
            saved,
            fold: HostFold::default(),
        }
    }

    /// The host after `update`, and whether it gave a name the app hasn't
    /// saved.
    pub fn linked(mut self, update: Update) -> (Self, bool) {
        self.fold.apply(update);
        let renamed = self
            .fold
            .name
            .as_ref()
            .is_some_and(|name| self.saved.as_ref() != Some(name));
        if renamed {
            self.saved.clone_from(&self.fold.name);
        }
        (self, renamed)
    }

    /// What the app calls the host: the name it gave, else a short form of
    /// its id.
    pub fn name(&self) -> String {
        self.saved.clone().unwrap_or_else(|| {
            let key = HostId::of(&self.ticket).key();
            format!("host {}", key.get(..8).unwrap_or(&key))
        })
    }

    /// The project a session runs in: the one with a checkout at its
    /// worktree's repository.
    pub fn project_of(&self, session: &SessionView) -> Option<&Project> {
        let repository = &session.workspace.as_ref()?.repository;
        self.fold.home.as_ref()?.projects.iter().find(|project| {
            project
                .checkouts
                .iter()
                .any(|checkout| &checkout.path == repository)
        })
    }

    pub fn status_line(&self) -> StatusLine {
        let shown = self.fold.status.clone().unwrap_or_default();
        let state = match shown.state {
            LinkState::Disconnected => "Disconnected",
            LinkState::Connecting => "Connecting",
            LinkState::Authenticating => "Authenticating",
            LinkState::CatchingUp => "CatchingUp",
            LinkState::Live => "Live",
        };
        StatusLine {
            state: String::from(state),
            host: self.name(),
            error: shown.error,
        }
    }
}
