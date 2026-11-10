impl std::fmt::Display for PermissionRequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for AuthToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&HEXLOWER.encode(&self.0))
    }
}

impl FromStr for AuthToken {
    type Err = TicketError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        HEXLOWER
            .decode(s.as_bytes())
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .map(Self)
            .ok_or(TicketError::Token)
    }
}

#[derive(Parser)]
struct Args {
    ticket: Ticket,
}

fn report(id: &PermissionRequestId, token: &AuthToken) {
    tracing::info!(%id, %token, "answered");
}
