//! Where the host ticket comes from on each target.

use proto::{ClientName, Ticket, TicketError};

/// Accepts what a person pastes: the whole join link, or the bare ticket.
pub fn parse_join(text: &str) -> Result<Ticket, TicketError> {
    let text = text.trim();
    let ticket = text
        .rsplit_once("#join=")
        .map_or(text, |(_, ticket)| ticket);
    Ticket::parse(ticket)
}

/// In the browser: `#join=<ticket>` on first open, saved to local storage and
/// removed from the address bar so a reload reconnects from storage.
#[cfg(target_family = "wasm")]
mod storage {
    const KEY: &str = "conductor.ticket";

    fn local() -> Option<web_sys::Storage> {
        web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    }

    pub(crate) fn get() -> Option<String> {
        local().and_then(|storage| storage.get_item(KEY).ok().flatten())
    }

    pub(crate) fn set(text: &str) {
        if let Some(storage) = local() {
            let _ = storage.set_item(KEY, text);
        }
    }

    pub(crate) fn remove() {
        if let Some(storage) = local() {
            let _ = storage.remove_item(KEY);
        }
    }
}

/// Remembers the host so the next launch reconnects. A Home Screen web app on
/// iOS has its own storage, separate from Safari's, so it pairs through here.
#[cfg(target_family = "wasm")]
pub fn save(ticket: &Ticket) {
    storage::set(&ticket.to_text());
}

#[cfg(not(target_family = "wasm"))]
pub fn save(_ticket: &Ticket) {}

/// Drops the saved host, so the next launch asks for a join link. A Home
/// Screen app can't receive a new join link from the camera; iOS opens those
/// in Safari, so pairing again starts here.
#[cfg(target_family = "wasm")]
pub fn forget() {
    storage::remove();
}

#[cfg(not(target_family = "wasm"))]
pub fn forget() {}

#[cfg(target_family = "wasm")]
pub fn initial() -> Result<Option<Ticket>, TicketError> {
    let window = web_sys::window().expect("running in a browser window");
    let hash = window.location().hash().unwrap_or_default();
    match hash.strip_prefix("#join=") {
        Some(text) => {
            let ticket = Ticket::parse(text)?;
            save(&ticket);
            let path = window.location().pathname().unwrap_or_default();
            if let Ok(history) = window.history() {
                let _ = history.replace_state_with_url(
                    &web_sys::wasm_bindgen::JsValue::NULL,
                    "",
                    Some(&path),
                );
            }
            Ok(Some(ticket))
        }
        None => storage::get().map(|text| Ticket::parse(&text)).transpose(),
    }
}

/// Natively: the `CONDUCTOR_TICKET` environment variable.
#[cfg(not(target_family = "wasm"))]
pub fn initial() -> Result<Option<Ticket>, TicketError> {
    std::env::var("CONDUCTOR_TICKET")
        .ok()
        .map(|text| Ticket::parse(&text))
        .transpose()
}

/// Shown as "by" on what this client does.
#[cfg(target_family = "wasm")]
pub fn client_name() -> ClientName {
    ClientName(String::from("browser"))
}

#[cfg(not(target_family = "wasm"))]
pub fn client_name() -> ClientName {
    ClientName(String::from("desktop"))
}

#[cfg(test)]
mod tests {
    use super::parse_join;

    const TICKET: &str = "AOr1251Bb2p0aTNfacOjGWA_ymOblEoyognrqD1bXfA_j1NbKmHUzEVqXP2oBgk2pGh0dHBzOi8vZXVjMS0xLnJlbGF5Lm4wLmlyb2gubGluay4v";

    #[test]
    fn accepts_a_whole_join_link_or_a_bare_ticket() {
        let link = format!("  https://jimlundin.github.io/conductor/#join={TICKET}\n");
        assert_eq!(parse_join(&link).unwrap().to_text(), TICKET);
        assert_eq!(parse_join(TICKET).unwrap().to_text(), TICKET);
        assert_eq!(
            parse_join("https://jimlundin.github.io/conductor/")
                .ok()
                .map(|ticket| ticket.to_text()),
            None
        );
    }
}
