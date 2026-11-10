//! Where the host ticket comes from on each target.

use proto::{Ticket, TicketError};

/// In the browser: `#join=<ticket>` on first open, saved to local storage and
/// removed from the address bar so a reload, or a Home Screen launch,
/// reconnects from storage.
#[cfg(target_family = "wasm")]
pub fn initial() -> Result<Option<Ticket>, TicketError> {
    const KEY: &str = "conductor.ticket";
    let window = web_sys::window().expect("running in a browser window");
    let storage = window.local_storage().ok().flatten();
    let hash = window.location().hash().unwrap_or_default();
    if let Some(text) = hash.strip_prefix("#join=") {
        let ticket: Ticket = text.parse()?;
        if let Some(storage) = &storage {
            storage.set_item(KEY, &ticket.to_string()).ok();
        }
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
