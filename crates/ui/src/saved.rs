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
        serde_json::from_str::<BTreeMap<String, Entry>>(text).ok()
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
                .into_values()
                .filter_map(|entry| {
                    Ticket::parse(&entry.ticket).ok().map(|ticket| Saved {
                        ticket,
                        name: entry.name,
                    })
                })
                .collect()
        },
    )
}
