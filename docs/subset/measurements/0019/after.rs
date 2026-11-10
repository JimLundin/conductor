#[derive(Parser)]
struct Args {
    ticket: Ticket,
    #[arg(long, default_value = "what is the answer?")]
    prompt: String,
    #[arg(long, value_enum, default_value = "approve")]
    decision: DecisionArg,
    /// Drop the connection after this many transcript deltas of our turn,
    /// to show the resubscribe with `after_seq`.
    #[arg(long)]
    reconnect_after: Option<usize>,
    #[arg(long, default_value_t = 60)]
    timeout_secs: u64,
}

#[derive(Clone, Copy, ValueEnum)]
enum DecisionArg {
    Approve,
    Deny,
}
