//! Where the host ticket comes from: the join link, then local storage.

use proto::{Ticket, TicketError};

/// What the app opens on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opening {
    Host(Ticket),
    Unpaired,
}

/// Accepts what a person pastes: the whole join link, or the bare ticket.
pub fn parse_join(text: &str) -> Result<Ticket, TicketError> {
    let trimmed = text.trim();
    let ticket = trimmed
        .rsplit_once("#join=")
        .map_or(trimmed, |(_, ticket)| ticket);
    net::parse(ticket)
}

/// `#join=<ticket>` on first open, saved to local storage and removed from
/// the address bar so a reload reconnects from storage.
mod storage {
    const KEY: &str = "conductor.ticket";

    fn local() -> Option<web_sys::Storage> {
        web_sys::window()
            .and_then(|window| window.local_storage().ok().flatten())
    }

    pub fn get() -> Option<String> {
        local().and_then(|storage| storage.get_item(KEY).ok().flatten())
    }

    pub fn set(text: &str) {
        if let Some(storage) = local() {
            let _ = storage.set_item(KEY, text);
        }
    }

    pub fn remove() {
        if let Some(storage) = local() {
            let _ = storage.remove_item(KEY);
        }
    }
}

/// Remembers the host so the next launch reconnects. A Home Screen web app on
/// iOS has its own storage, separate from Safari's, so it pairs through here.
pub fn save(ticket: &Ticket) {
    storage::set(&ticket.to_text());
}

/// Drops the saved host, so the next launch asks for a join link. A Home
/// Screen app can't receive a new join link from the camera; iOS opens those
/// in Safari, so pairing again starts here.
pub fn forget() {
    storage::remove();
}

pub fn initial() -> Result<Opening, TicketError> {
    web_sys::window().map_or(Ok(Opening::Unpaired), |window| opened(&window))
}

/// The join link in the address bar, else the saved host.
fn opened(window: &web_sys::Window) -> Result<Opening, TicketError> {
    let hash = window.location().hash().unwrap_or_default();
    hash.strip_prefix("#join=")
        .map_or_else(|| opening(storage::get()), |text| joined(window, text))
}

fn joined(
    window: &web_sys::Window,
    text: &str,
) -> Result<Opening, TicketError> {
    let ticket = net::parse(text)?;
    save(&ticket);
    clear_hash(window);
    Ok(Opening::Host(ticket))
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

fn opening(saved: Option<String>) -> Result<Opening, TicketError> {
    saved.map_or(Ok(Opening::Unpaired), |text| {
        net::parse(&text).map(Opening::Host)
    })
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
