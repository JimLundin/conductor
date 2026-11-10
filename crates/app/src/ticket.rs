//! Where the host ticket comes from on each target.

use proto::{Ticket, TicketError};

/// In the browser: `#join=<ticket>` on first open, saved to local storage and
/// removed from the address bar so a reload reconnects from storage.
#[cfg(target_family = "wasm")]
const KEY: &str = "conductor.ticket";

/// Accepts what a person pastes: the whole join link, or the bare ticket.
pub fn parse_join(text: &str) -> Result<Ticket, TicketError> {
    let text = text.trim();
    let ticket = text
        .rsplit_once("#join=")
        .map_or(text, |(_, ticket)| ticket);
    ticket.parse()
}

/// Remembers the host so the next launch reconnects. A Home Screen web app on
/// iOS has its own storage, separate from Safari's, so it pairs through here.
#[cfg(target_family = "wasm")]
pub fn save(ticket: &Ticket) {
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        storage.set_item(KEY, &ticket.to_string()).ok();
    }
}

#[cfg(not(target_family = "wasm"))]
pub fn save(_ticket: &Ticket) {}

#[cfg(target_family = "wasm")]
pub fn initial() -> Result<Option<Ticket>, TicketError> {
    let window = web_sys::window().expect("running in a browser window");
    let storage = window.local_storage().ok().flatten();
    let hash = window.location().hash().unwrap_or_default();
    if let Some(text) = hash.strip_prefix("#join=") {
        let ticket: Ticket = text.parse()?;
        save(&ticket);
        let path = window.location().pathname().unwrap_or_default();
        if let Ok(history) = window.history() {
            history
                .replace_state_with_url(&web_sys::wasm_bindgen::JsValue::NULL, "", Some(&path))
                .ok();
        }
        return Ok(Some(ticket));
    }
    match storage.and_then(|s| s.get_item(KEY).ok().flatten()) {
        Some(text) => text.parse().map(Some),
        None => Ok(None),
    }
}

/// Natively: the `CONDUCTOR_TICKET` environment variable.
#[cfg(not(target_family = "wasm"))]
pub fn initial() -> Result<Option<Ticket>, TicketError> {
    std::env::var("CONDUCTOR_TICKET")
        .ok()
        .map(|text| text.parse())
        .transpose()
}

#[cfg(target_family = "wasm")]
pub const CLIENT_NAME: &str = "browser";
#[cfg(not(target_family = "wasm"))]
pub const CLIENT_NAME: &str = "desktop";

#[cfg(test)]
mod tests {
    use super::parse_join;

    const TICKET: &str = "AOr1251Bb2p0aTNfacOjGWA_ymOblEoyognrqD1bXfA_j1NbKmHUzEVqXP2oBgk2pGh0dHBzOi8vZXVjMS0xLnJlbGF5Lm4wLmlyb2gubGluay4v";

    #[test]
    fn accepts_a_whole_join_link_or_a_bare_ticket() {
        let link = format!("  https://jimlundin.github.io/conductor/#join={TICKET}\n");
        assert_eq!(parse_join(&link).unwrap().to_string(), TICKET);
        assert_eq!(parse_join(TICKET).unwrap().to_string(), TICKET);
        assert!(parse_join("https://jimlundin.github.io/conductor/").is_err());
    }
}
