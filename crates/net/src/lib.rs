//! The client link and the fleet protocol on iroh: frame I/O on streams,
//! a ticket as an address to dial, and where a host is.

pub mod platform;
mod relay;
pub mod wire;

use iroh::{EndpointAddr, EndpointId, RelayUrl};
use link::{AuthToken, Ticket, TicketError};

pub use relay::without_root_dot;

/// Parses ticket text and checks that it names a host that can be dialled.
///
/// # Errors
///
/// The [`TicketError`] of [`Ticket::parse`], or of [`address`].
pub fn parse(text: &str) -> Result<Ticket, TicketError> {
    let ticket = Ticket::parse(text)?;
    address(&ticket).map(|_| ticket)
}

/// The host's iroh address, by its relay named without the root dot.
///
/// # Errors
///
/// [`TicketError::HostId`] when the id is not a valid key, and
/// [`TicketError::RelayUrl`] when the relay is not a valid URL.
pub fn address(ticket: &Ticket) -> Result<EndpointAddr, TicketError> {
    let id = EndpointId::from_bytes(&ticket.host)
        .map_err(|_| TicketError::HostId)?;
    let relay = ticket
        .relay
        .parse::<RelayUrl>()
        .map_err(|_| TicketError::RelayUrl)?;
    Ok(EndpointAddr::new(id).with_relay_url(without_root_dot(&relay)))
}

/// Where a host is and the secret it expects in `Hello`.
#[derive(Debug, Clone)]
pub struct Target {
    pub addr: EndpointAddr,
    pub token: AuthToken,
}

impl Target {
    /// # Errors
    ///
    /// The [`TicketError`] of [`address`].
    pub fn of_ticket(ticket: &Ticket) -> Result<Self, TicketError> {
        Ok(Self {
            addr: address(ticket)?,
            token: ticket.token,
        })
    }
}

#[cfg(test)]
mod tests {
    use link::{AuthToken, Ticket, TicketError};

    use super::{address, parse};

    fn ticket(host: [u8; 32], relay: &str) -> Ticket {
        Ticket {
            host,
            relay: String::from(relay),
            token: AuthToken::from_bytes([1; 16]),
        }
    }

    /// The ed25519 base point: a fixed, valid public key.
    fn base_point() -> [u8; 32] {
        core::array::from_fn(|i| if i == 0 { 0x58 } else { 0x66 })
    }

    #[test]
    fn a_valid_ticket_dials_its_relay_without_the_root_dot()
    -> Result<(), TicketError> {
        let valid = ticket(base_point(), "https://r.example./");
        let addr = address(&parse(&valid.to_text())?)?;
        assert_eq!(
            addr.relay_urls()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["https://r.example/"]
        );
        Ok(())
    }

    #[test]
    fn an_invalid_key_or_relay_is_refused() {
        let point = base_point();
        assert_eq!(
            parse(&ticket(point, "not a url").to_text()),
            Err(TicketError::RelayUrl)
        );
        let off_curve = core::array::from_fn(|i| if i == 0 { 2 } else { 0 });
        assert_eq!(
            parse(&ticket(off_curve, "https://r.example/").to_text()),
            Err(TicketError::HostId)
        );
    }
}
