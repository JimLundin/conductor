pub fn alpn() -> Vec<u8> {
    Vec::from(b"conductor/0")
}
pub fn default_stun_servers() -> Vec<String> {
    Vec::from([String::from("stun:stun.l.google.com:19302"), String::from("stun:stun.cloudflare.com:3478")])
}

fn seqs(events: &Vec<Event>) -> Vec<u64> {
    events.iter().map(|e| e.seq.0).collect()
}

fn transcript(events: &Vec<Event>) -> String {
    events.iter().filter_map(|e| match &e.kind {
        EventKind::TranscriptDelta { text } => Some(text.as_str()),
        _ => None,
    }).collect()
}

fn decode(body: &Vec<u8>) -> Result<Open, FrameError> {
    serde_json::from_slice(body).map_err(FrameError::Json)
}
