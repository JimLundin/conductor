use client::link::Update;
use proto::{
    AuthToken, Change, Checkpoint, CheckpointDiff, CheckpointFile,
    CheckpointId, ClientName, CommandResult, DiffExtent, Event, EventKind,
    Feed, FilePatch, HistoryAnchor, HostPath, HostSeq, InstanceId, Provider,
    SessionId, Ticket, ToolCallId, ToolKind, ToolOutcome, ToolPhase,
    TurnOutcome, UndoAvailability,
};
use ui::{
    Detail, DiffPanel, Effect, HostId, Intent, Opening, Place, SessionIntent,
    State, Stored, update,
};

const HOST: HostId = HostId([9; 32]);
const SESSION: SessionId = SessionId(7);
const TURNS: u64 = 12;
const OPEN: [u64; 3] = [3, 7, 12];
const PATCHES_PER_CHECKPOINT: usize = 4;
const PATCH_BYTES: usize = 64 * 1024;

fn event(state: State, seq: u64, kind: EventKind) -> State {
    update(
        state,
        Intent::Link(
            HOST,
            Update::Feed(Feed::Event(Event {
                seq: HostSeq(seq),
                session: SESSION,
                kind,
            })),
        ),
    )
    .0
}

fn transcript() -> State {
    let paired = State::new(Opening {
        stored: Stored::default(),
        joined: Some(Ok(Ticket {
            host: HOST.0,
            relay: "https://relay.example/".to_owned(),
            token: AuthToken::from_bytes([1; 16]),
        })),
    });
    let created = event(
        paired,
        1,
        EventKind::SessionCreated {
            instance: InstanceId::own(Provider::Claude),
            provider: Provider::Claude,
            directory: HostPath("/home/me/conductor".to_owned()),
            by: ClientName("desk".to_owned()),
            model: None,
            effort: None,
        },
    );
    let idle = event(created, 2, EventKind::SessionReady);
    let shown = update(idle, Intent::Show(Place::Session(HOST, SESSION))).0;
    (1..=TURNS).fold(shown, |state, turn| {
        let first = 3 + (turn - 1) * 7;
        let call = ToolCallId(format!("tool-{turn}"));
        let events = [
            EventKind::UserMessage {
                text: format!("Review the changed source files (turn {turn})."),
                by: ClientName("desk".to_owned()),
            },
            EventKind::Reasoning {
                text: "Check the code around the changed files before making edits.".to_owned(),
            },
            EventKind::ToolCall {
                call: call.clone(),
                phase: ToolPhase::Started {
                    tool: ToolKind::Command { command: "git diff --stat".to_owned() },
                },
            },
            EventKind::ToolCall {
                call,
                phase: ToolPhase::Finished { outcome: ToolOutcome::Succeeded },
            },
            EventKind::AssistantMessage {
                text: format!("Updated the source files and verified the checks for turn {turn}."),
            },
            EventKind::TurnCompleted { outcome: TurnOutcome::Completed },
            EventKind::CheckpointCreated {
                checkpoint: Checkpoint {
                    id: CheckpointId(turn),
                    parent: Some(CheckpointId(turn - 1)),
                    turn_start: Some(HostSeq(first)),
                    commit: format!("commit-{turn}"),
                    history: HistoryAnchor::Fresh,
                    undo: UndoAvailability::Available,
                },
            },
        ];
        events.into_iter().enumerate().fold(state, |current, (offset, kind)| {
            event(current, first + offset as u64, kind)
        })
    })
}

fn open_preview(state: State, turn: u64, patch: &str) -> State {
    let (loading, effects) = update(
        state,
        Intent::Session(SessionIntent::ShowChanges(CheckpointId(turn))),
    );
    let reply = effects
        .into_iter()
        .find_map(|effect| match effect {
            Effect::Send { reply, .. } => Some(reply),
            _ => None,
        })
        .expect("a request for each checkpoint");
    let diff = CheckpointDiff {
        checkpoint: CheckpointId(turn),
        parent: CheckpointId(turn - 1),
        files: (0..PATCHES_PER_CHECKPOINT)
            .map(|index| CheckpointFile {
                path: format!("file-{index}.rs"),
                change: Change::Edit,
                patch: FilePatch::Text(patch.to_owned()),
            })
            .collect(),
        extent: DiffExtent::Complete,
    };
    update(
        loading,
        Intent::Replied(reply, Ok(CommandResult::Diff(diff))),
    )
    .0
}

fn measure(name: &str, patch: &str) {
    let loaded = OPEN
        .into_iter()
        .fold(transcript(), |state, turn| open_preview(state, turn, patch));
    let (edited, _) = update(
        loaded,
        Intent::Session(SessionIntent::TypeMessage(
            "An unrelated draft update.".to_owned(),
        )),
    );
    let mut detail = None;
    let allocations = allocation_counter::measure(|| {
        detail = Some(edited.detail());
    });
    let Some(Detail::Session(pane)) = detail else {
        panic!("expected a session detail")
    };
    assert_eq!(pane.entries.len(), 60);
    assert_eq!(
        pane.entries
            .iter()
            .filter(|entry| {
                entry.changes.as_ref().is_some_and(|changes| {
                    matches!(changes.panel, DiffPanel::Loaded(_))
                })
            })
            .count(),
        OPEN.len()
    );
    println!(
        "{name}: rows={}, previews={}, calls={}, bytes={} peak_calls={} peak_bytes={}",
        pane.entries.len(),
        OPEN.len(),
        allocations.count_total,
        allocations.bytes_total,
        allocations.count_max,
        allocations.bytes_max
    );
}

#[test]
fn preview_allocations() {
    measure("empty-text control", "");
    let patch =
        "+ let configuration = read();\n".repeat(PATCH_BYTES.div_ceil(30));
    measure("64 KiB patches", &patch[..PATCH_BYTES]);
}
