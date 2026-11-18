//! Where the hosts' tickets come from: a join link, then local storage.

use proto::{Ticket, TicketError};
use ui::{Opening, Saved, State, Stored};

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

mod storage {
    fn local() -> Option<web_sys::Storage> {
        web_sys::window()
            .and_then(|window| window.local_storage().ok().flatten())
    }

    pub fn get(key: &str) -> Option<String> {
        local().and_then(|storage| storage.get_item(key).ok().flatten())
    }

    pub fn set(key: &str, text: &str) {
        if let Some(storage) = local() {
            let _ = storage.set_item(key, text);
        }
    }

    pub fn remove(key: &str) {
        if let Some(storage) = local() {
            let _ = storage.remove_item(key);
        }
    }
}

/// Remembers the hosts so the next launch reconnects to each. A Home Screen
/// web app on iOS has its own storage, separate from Safari's, so it pairs
/// through the app's own form. The older single ticket goes once the list
/// holds it.
pub fn save(hosts: &[Saved]) {
    storage::set(HOSTS, &ui::encode(hosts));
    storage::remove(TICKET);
}

/// Opens on the saved hosts and on `#join=<ticket>` in the address bar,
/// saves them as one list, and takes the join link out of the address bar
/// so a reload reconnects from storage. A Home Screen app can't receive a
/// new join link from the camera; iOS opens those in Safari, so pairing
/// again, or with one more host, starts in the app.
pub fn open() -> State {
    let window = web_sys::window();
    let hash = window
        .as_ref()
        .and_then(|shown| shown.location().hash().ok())
        .unwrap_or_default();
    let joined = hash.strip_prefix("#join=").map(net::parse);
    let state = State::new(Opening {
        stored: Stored {
            hosts: storage::get(HOSTS),
            ticket: storage::get(TICKET),
        },
        joined,
    });
    save(&state.saved());
    if let Some(shown) = window.filter(|_| hash.starts_with("#join=")) {
        clear_hash(&shown);
    }
    state
}

fn clear_hash(window: &web_sys::Window) {
    let path = window.location().pathname().unwrap_or_default();
    if let Ok(history) = window.history() {
        let _ = history.replace_state_with_url(
            &web_sys::wasm_bindgen::JsValue::NULL,
            "",
            Some(&path),
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
