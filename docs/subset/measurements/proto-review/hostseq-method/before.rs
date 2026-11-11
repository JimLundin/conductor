#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostSeq(pub u64);

fn ours(event: &Event, after: HostSeq) -> Ours {
    match event.seq.0.cmp(&after.0) {
        Ordering::Greater => Ours::Yes,
        Ordering::Less | Ordering::Equal => Ours::No,
    }
}
fn replay(log: &[Event], after_seq: HostSeq) -> Vec<Event> {
    log.iter()
        .filter_map(|e| match e.seq.0.cmp(&after_seq.0) {
            Ordering::Greater => Some(e.clone()),
            Ordering::Less | Ordering::Equal => None,
        })
        .collect()
}
fn caught_up(live_at: HostSeq, cursor: HostSeq) -> Progress {
    match live_at.0.cmp(&cursor.0) {
        Ordering::Less | Ordering::Equal => Progress::Live,
        Ordering::Greater => Progress::CatchingUp,
    }
}
fn seen(event: &Event, cursor: HostSeq) -> Seen {
    match event.seq.0.cmp(&cursor.0) {
        Ordering::Less | Ordering::Equal => Seen::Duplicate,
        Ordering::Greater => Seen::New,
    }
}
fn reached(event: &Event, live_at: HostSeq) -> Progress {
    match event.seq.0.cmp(&live_at.0) {
        Ordering::Less => Progress::CatchingUp,
        Ordering::Equal | Ordering::Greater => Progress::Live,
    }
}
fn forward(event: &Event, last: HostSeq) -> Seen {
    match event.seq.0.cmp(&last.0) {
        Ordering::Less | Ordering::Equal => Seen::Duplicate,
        Ordering::Greater => Seen::New,
    }
}
