//! The paired hosts as local storage keeps them: a JSON object keyed by
//! each host's endpoint id in hex, holding its ticket and the name from its
//! last `Welcome`. An app from before several hosts kept one bare ticket;
//! the first launch since takes it into the list.

use alloc::collections::BTreeMap;

use crate::prelude::*;
use proto::Ticket;
use serde::{Deserialize, Serialize};

use crate::host::HostId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub ticket: Ticket,
    pub name: Option<String>,
}

/// What local storage holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stored {
    /// The list, as [`encode`] wrote it.
    pub hosts: Option<String>,
    /// The single ticket an older app saved.
    pub ticket: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    ticket: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[must_use]
pub fn encode(hosts: &[Saved]) -> String {
    let entries = hosts
        .iter()
        .map(|saved| {
            (
                HostId::of(&saved.ticket).key(),
                Entry {
                    ticket: saved.ticket.to_text(),
                    name: saved.name.clone(),
                },
            )
        })
        .collect::<BTreeMap<String, Entry>>();
    serde_json::to_string(&entries).unwrap_or_default()
}

/// The saved hosts: the list once there is one, else the older app's
/// ticket. A ticket that no longer parses is dropped.
#[must_use]
pub fn decode(stored: &Stored) -> Vec<Saved> {
    let listed = stored.hosts.as_deref().and_then(|text| {
        serde_json::from_str::<BTreeMap<String, serde_json::Value>>(text).ok()
    });
    listed.map_or_else(
        || {
            stored
                .ticket
                .as_deref()
                .and_then(|text| Ticket::parse(text).ok())
                .map(|ticket| Saved { ticket, name: None })
                .into_iter()
                .collect()
        },
        |entries| {
            entries
                .into_iter()
                .filter_map(|(key, value)| {
                    let entry = serde_json::from_value::<Entry>(value).ok()?;
                    let ticket = Ticket::parse(&entry.ticket).ok()?;
                    (HostId::of(&ticket).key() == key).then_some(Saved {
                        ticket,
                        name: entry.name,
                    })
                })
                .collect()
        },
    )
}

// Per-host overrides cover removals from the old shared map. Only explicit
// pairing can replace a removal marker.
pub const HOST_KEY: &str = "conductor.host.";
pub const NAME_KEY: &str = "conductor.name.";

#[must_use]
pub fn host_storage_key(key: &str) -> bool {
    key.starts_with(HOST_KEY) || key.starts_with(NAME_KEY)
}

#[must_use]
pub fn restored(stored: &Stored, entries: Vec<(String, String)>) -> Vec<Saved> {
    let mut hosts = decode(stored)
        .into_iter()
        .map(|saved| (HostId::of(&saved.ticket).key(), saved))
        .collect::<BTreeMap<_, _>>();
    let mut names = BTreeMap::new();
    for (key, value) in entries {
        if let Some(id) = key.strip_prefix(HOST_KEY) {
            if value.is_empty() {
                hosts.remove(id);
            } else if let Some(saved) = decode(&Stored {
                hosts: Some(value),
                ticket: None,
            })
            .into_iter()
            .find(|saved| HostId::of(&saved.ticket).key() == id)
            {
                hosts.insert(id.to_owned(), saved);
            }
        } else if let Some(id) = key.strip_prefix(NAME_KEY) {
            names.insert(id.to_owned(), value);
        }
    }
    hosts
        .into_iter()
        .map(|(id, saved)| Saved {
            name: names.remove(&id).or(saved.name),
            ..saved
        })
        .collect()
}

#[must_use]
pub fn legacy_to_migrate(text: Option<&str>) -> Option<&str> {
    text.filter(|ticket| Ticket::parse(ticket).is_ok())
}

#[must_use]
pub fn joined_host(hosts: &[Saved], ticket: &Ticket) -> Saved {
    let name = hosts
        .iter()
        .find(|saved| saved.ticket.host == ticket.host)
        .and_then(|saved| saved.name.clone());
    Saved {
        ticket: ticket.clone(),
        name,
    }
}
