struct Args {
    ticket: Ticket,
    prompt: String,
    decision: Decision,
    /// Drop the connection after this many transcript deltas of our turn,
    /// to show the resubscribe with `after_seq`.
    reconnect_after: Option<usize>,
    timeout_secs: u64,
}

const USAGE: &str = "usage: headless <ticket> [--prompt TEXT] [--decision approve|deny] [--reconnect-after N] [--timeout-secs N]";

impl Args {
    fn new(ticket: Ticket) -> Args {
        Args {
            ticket,
            prompt: String::from("what is the answer?"),
            decision: Decision::Approve,
            reconnect_after: None,
            timeout_secs: 60,
        }
    }

    fn parse(words: &[String]) -> Result<Args, String> {
        let (ticket, flags) = words.split_first().ok_or(String::from(USAGE))?;
        let ticket = Ticket::parse(ticket).map_err(|e| format!("ticket: {e}"))?;
        flags.chunks(2).try_fold(Args::new(ticket), |args, pair| {
            match (pair.first().map(String::as_str), pair.get(1)) {
                (Some("--prompt"), Some(value)) => Ok(Args { prompt: value.clone(), ..args }),
                (Some("--decision"), Some(value)) => {
                    decision(value).map(|decision| Args { decision, ..args })
                }
                (Some("--reconnect-after"), Some(value)) => value
                    .parse()
                    .map(|n| Args { reconnect_after: Some(n), ..args })
                    .map_err(|e| format!("--reconnect-after: {e}")),
                (Some("--timeout-secs"), Some(value)) => value
                    .parse()
                    .map(|n| Args { timeout_secs: n, ..args })
                    .map_err(|e| format!("--timeout-secs: {e}")),
                (flag, _) => Err(format!("unknown or incomplete flag {flag:?}\n{USAGE}")),
            }
        })
    }
}

fn decision(value: &str) -> Result<Decision, String> {
    match value {
        "approve" => Ok(Decision::Approve),
        "deny" => Ok(Decision::Deny),
        other => Err(format!("--decision must be approve or deny, not {other}")),
    }
}
