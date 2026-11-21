//! Where the hosts' tickets come from: a join link, then local storage.

use proto::{Ticket, TicketError};
use ui::{HOST_KEY, HostId, NAME_KEY, Opening, Saved, State, Stored};

/// The paired hosts, as `ui::encode` writes them.
const HOSTS: &str = "conductor.hosts";
/// The one ticket an app from before several hosts saved.
const TICKET: &str = "conductor.ticket";

/// Accepts what a person pastes: the whole join link, or the bare ticket.
pub fn parse_join(text: &str) -> Result<Ticket, TicketError> {
    let trimmed = text.trim();
    let ticket = trimmed
        .rsplit_once("#join=")
        .map_or(trimmed, |(_, ticket)| ticket);
    net::parse(ticket)
}

const MIGRATED: &str = "conductor.migrated-ticket";

fn local() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|window| window.local_storage().ok().flatten())
}

fn get(storage: &web_sys::Storage, key: &str) -> Option<String> {
    storage.get_item(key).ok().flatten()
}

pub fn pair(saved: Saved) -> bool {
    let key = format!("{HOST_KEY}{}", HostId::of(&saved.ticket).key());
    local().is_some_and(|storage| {
        storage.set_item(&key, &ui::encode(&[saved])).is_ok()
    })
}

pub fn forget(host: HostId) -> bool {
    let key = host.key();
    local().is_some_and(|storage| {
        let hidden = storage.set_item(&format!("{HOST_KEY}{key}"), "").is_ok();
        if hidden {
            let _ = storage.remove_item(&format!("{NAME_KEY}{key}"));
        }
        hidden
    })
}

pub fn name(saved: Saved) {
    let key = format!("{NAME_KEY}{}", HostId::of(&saved.ticket).key());
    if let (Some(storage), Some(name)) = (local(), saved.name) {
        let _ = storage.set_item(&key, &name);
    }
}

fn saved_hosts(
    storage: &web_sys::Storage,
    legacy: Option<String>,
) -> Vec<Saved> {
    let stored = Stored {
        hosts: get(storage, HOSTS),
        ticket: legacy,
    };
    let entries = (0..storage.length().unwrap_or_default())
        .filter_map(|index| storage.key(index).ok().flatten())
        .filter(|key| ui::host_storage_key(key))
        .filter_map(|key| get(storage, &key).map(|value| (key, value)));
    ui::restored(&stored, entries.collect())
}

fn load_hosts(storage: &web_sys::Storage) -> (Vec<Saved>, bool) {
    let legacy = get(storage, TICKET);
    let failed = ui::legacy_to_migrate(legacy.as_deref())
        .is_some_and(|ticket| !migrate(storage, ticket));
    let hosts = saved_hosts(storage, legacy.or_else(|| get(storage, MIGRATED)));
    (hosts, failed)
}

fn migrate(storage: &web_sys::Storage, ticket: &str) -> bool {
    let saved = storage.set_item(MIGRATED, ticket).is_ok();
    if saved {
        let _ = storage.remove_item(TICKET);
    }
    saved
}

fn pair_from_hash(window: Option<&web_sys::Window>, saved: Saved) -> bool {
    let remembered = pair(saved);
    if let (true, Some(shown)) = (remembered, window) {
        clear_hash(shown);
    }
    remembered
}

pub fn open() -> (State, bool) {
    let window = web_sys::window();
    let hash = window
        .as_ref()
        .and_then(|shown| shown.location().hash().ok())
        .unwrap_or_default();
    let joined = hash.strip_prefix("#join=").map(net::parse);
    let (hosts, migration_failed) = local()
        .as_ref()
        .map_or_else(|| (Vec::new(), false), load_hosts);
    let pairing_failed = joined
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .is_some_and(|ticket| {
            !pair_from_hash(window.as_ref(), ui::joined_host(&hosts, ticket))
        });
    let state = State::new(Opening {
        stored: Stored {
            hosts: Some(ui::encode(&hosts)),
            ticket: None,
        },
        joined,
    });
    (state, migration_failed || pairing_failed)
}

fn clear_hash(window: &web_sys::Window) {
    let location = window.location();
    if let (Ok(path), Ok(search), Ok(history)) =
        (location.pathname(), location.search(), window.history())
    {
        let _ = history.replace_state_with_url(
            &web_sys::wasm_bindgen::JsValue::NULL,
            "",
            Some(&format!("{path}{search}")),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::parse_join;

    const TICKET: &str = "AOr1251Bb2p0aTNfacOjGWA_ymOblEoyognrqD1bXfA_j1NbKmHUzEVqXP2oBgk2pGh0dHBzOi8vZXVjMS0xLnJlbGF5Lm4wLmlyb2gubGluay4v";

    #[test]
    fn accepts_a_whole_join_link_or_a_bare_ticket()
    -> Result<(), Box<dyn std::error::Error>> {
        let link =
            format!("  https://jimlundin.github.io/conductor/#join={TICKET}\n");
        assert_eq!(parse_join(&link)?.to_text(), TICKET);
        assert_eq!(parse_join(TICKET)?.to_text(), TICKET);
        assert_eq!(
            parse_join("https://jimlundin.github.io/conductor/")
                .ok()
                .map(|ticket| ticket.to_text()),
            None
        );
        Ok(())
    }
}
