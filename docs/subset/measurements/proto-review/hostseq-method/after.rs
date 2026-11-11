#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostSeq(pub u64);

pub enum Freshness {
    Behind,
    Current,
    Ahead,
}

impl HostSeq {
    pub fn freshness(self, head: HostSeq) -> Freshness {
        match self.0.cmp(&head.0) {
            Ordering::Less => Freshness::Behind,
            Ordering::Equal => Freshness::Current,
            Ordering::Greater => Freshness::Ahead,
        }
    }
}

fn ours(event: &Event, after: HostSeq) -> Ours {
    match event.seq.freshness(after) {
        Freshness::Ahead => Ours::Yes,
        Freshness::Behind | Freshness::Current => Ours::No,
    }
}
fn replay(log: &[Event], after_seq: HostSeq) -> Vec<Event> {
    log.iter()
        .filter_map(|e| match e.seq.freshness(after_seq) {
            Freshness::Ahead => Some(e.clone()),
            Freshness::Behind | Freshness::Current => None,
        })
        .collect()
}
fn caught_up(live_at: HostSeq, cursor: HostSeq) -> Progress {
    match live_at.freshness(cursor) {
        Freshness::Behind | Freshness::Current => Progress::Live,
        Freshness::Ahead => Progress::CatchingUp,
    }
}
fn seen(event: &Event, cursor: HostSeq) -> Seen {
    match event.seq.freshness(cursor) {
        Freshness::Behind | Freshness::Current => Seen::Duplicate,
        Freshness::Ahead => Seen::New,
    }
}
fn reached(event: &Event, live_at: HostSeq) -> Progress {
    match event.seq.freshness(live_at) {
        Freshness::Behind => Progress::CatchingUp,
        Freshness::Current | Freshness::Ahead => Progress::Live,
    }
}
fn forward(event: &Event, last: HostSeq) -> Seen {
    match event.seq.freshness(last) {
        Freshness::Behind | Freshness::Current => Seen::Duplicate,
        Freshness::Ahead => Seen::New,
    }
}
