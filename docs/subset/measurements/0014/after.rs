pub const ALPN: &[u8] = b"conductor/0";
pub const DEFAULT_STUN_SERVERS: &[&str] = &["stun:stun.l.google.com:19302", "stun:stun.cloudflare.com:3478"];

fn seqs(events: &[Event]) -> Vec<u64> {
    events.iter().map(|e| e.seq.0).collect()
}

fn transcript(events: &[Event]) -> String {
    events.iter().filter_map(|e| match &e.kind {
        EventKind::TranscriptDelta { text } => Some(text.as_str()),
        _ => None,
    }).collect()
}

fn decode(body: &[u8]) -> Result<Open, FrameError> {
    serde_json::from_slice(body).map_err(FrameError::Json)
}
