#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostSeq(pub u64);

fn ours(event: &Event, after: HostSeq) -> Ours {
    match event.seq.cmp(&after) {
        Ordering::Greater => Ours::Yes,
        Ordering::Less | Ordering::Equal => Ours::No,
    }
}
fn replay(log: &[Event], after_seq: HostSeq) -> Vec<Event> {
    log.iter()
        .filter_map(|e| match e.seq.cmp(&after_seq) {
            Ordering::Greater => Some(e.clone()),
            Ordering::Less | Ordering::Equal => None,
        })
        .collect()
}
fn caught_up(live_at: HostSeq, cursor: HostSeq) -> Progress {
    match live_at.cmp(&cursor) {
        Ordering::Less | Ordering::Equal => Progress::Live,
        Ordering::Greater => Progress::CatchingUp,
    }
}
fn seen(event: &Event, cursor: HostSeq) -> Seen {
    match event.seq.cmp(&cursor) {
        Ordering::Less | Ordering::Equal => Seen::Duplicate,
        Ordering::Greater => Seen::New,
    }
}
fn reached(event: &Event, live_at: HostSeq) -> Progress {
    match event.seq.cmp(&live_at) {
        Ordering::Less => Progress::CatchingUp,
        Ordering::Equal | Ordering::Greater => Progress::Live,
    }
}
fn forward(event: &Event, last: HostSeq) -> Seen {
    match event.seq.cmp(&last) {
        Ordering::Less | Ordering::Equal => Seen::Duplicate,
        Ordering::Greater => Seen::New,
    }
}
