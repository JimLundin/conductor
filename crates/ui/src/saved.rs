//! The paired hosts as local storage keeps them: a JSON object keyed by
//! each host's endpoint id in hex, holding its ticket and the name from its
//! last `Welcome`.

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

/// Invalid entries do not discard healthy neighbors. A ticket must match
/// the host key it was saved under.
#[must_use]
pub fn decode(text: &str) -> Vec<Saved> {
    serde_json::from_str::<BTreeMap<String, serde_json::Value>>(text)
        .unwrap_or_default()
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
}

// Separate host keys let concurrent tabs pair independently. Names can
// refresh without overwriting a pairing or resurrecting a forgotten host.
pub const HOST_KEY: &str = "conductor.host.";
pub const NAME_KEY: &str = "conductor.name.";

#[must_use]
pub fn host_storage_key(key: &str) -> bool {
    key.starts_with(HOST_KEY) || key.starts_with(NAME_KEY)
}

#[must_use]
pub fn restored(entries: Vec<(String, String)>) -> Vec<Saved> {
    let mut hosts = BTreeMap::new();
    let mut names = BTreeMap::new();
    for (key, value) in entries {
        if let Some(id) = key.strip_prefix(HOST_KEY) {
            if let Some(saved) = decode(&value)
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
