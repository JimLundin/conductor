impl PermissionRequestId {
    pub fn text(&self) -> &str {
        &self.0
    }
}

impl AuthToken {
    pub fn hex(&self) -> String {
        HEXLOWER.encode(&self.0)
    }

    pub fn parse(s: &str) -> Result<AuthToken, TicketError> {
        HEXLOWER
            .decode(s.as_bytes())
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .map(AuthToken)
            .ok_or(TicketError::Token)
    }
}

#[derive(Parser)]
struct Args {
    #[arg(value_parser = Ticket::parse)]
    ticket: Ticket,
}

fn report(id: &PermissionRequestId, token: &AuthToken) {
    tracing::info!(id = id.text(), token = token.hex(), "answered");
}
