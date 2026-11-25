use crate::log::{Event, HostSeq};
use crate::session::{EventKind, SessionId};
use agents::Output;

use super::{Effect, FORMAT, Log, Write, opens};

fn log() -> Log {
    Log::new(Vec::new())
}

fn delta(text: &str) -> EventKind {
    EventKind::Agent(Output::AssistantDelta {
        text: String::from(text),
    })
}

fn event(seq: u64, kind: EventKind) -> Event {
    Event {
        seq: HostSeq(seq),
        session: SessionId(1),
        kind,
    }
}

fn stored(seq: u64, kind: EventKind) -> Vec<Effect> {
    let event = event(seq, kind);
    Vec::from([
        Effect::Open,
        Effect::Store(Write::Event(event.clone())),
        Effect::Commit,
        Effect::Publish(event),
    ])
}

#[test]
fn stored_events_take_the_next_seq_and_deltas_repeat_the_last() {
    let mut log = log();
    assert_eq!(log.head(), HostSeq::BEFORE_FIRST);
    let (ready, effects) = log.append(SessionId(1), EventKind::SessionReady);
    assert_eq!(ready.seq, HostSeq::BEFORE_FIRST.next());
    assert_eq!(ready, event(1, EventKind::SessionReady));
    assert_eq!(effects, stored(1, EventKind::SessionReady));
    let (streamed, effects) = log.append(SessionId(1), delta("a"));
    assert_eq!(streamed, event(1, delta("a")));
    assert_eq!(effects, [Effect::Publish(event(1, delta("a")))]);
    let (_, effects) = log.append(SessionId(1), EventKind::SessionReady);
    assert_eq!(effects, stored(2, EventKind::SessionReady));
    assert_eq!(
        log.history(),
        [
            event(1, EventKind::SessionReady),
            event(2, EventKind::SessionReady)
        ]
    );
    assert_eq!(log.replay(HostSeq(1)), [event(2, EventKind::SessionReady)]);
    assert_eq!(log.head(), HostSeq(2));
}

#[test]
fn session_admission_sees_staged_events_while_replay_waits_for_the_commit() {
    let mut log = log();
    let _ = log.append(SessionId(1), EventKind::SessionReady);
    assert_eq!(log.open(), Effect::Open);
    let (started, _) = log.append(SessionId(1), EventKind::FinishStarted);
    let _ = log.append(SessionId(1), delta("partial"));
    let _ = log.append(SessionId(2), EventKind::SessionReady);
    assert_eq!(
        log.current_history(SessionId(1)),
        [event(1, EventKind::SessionReady), started]
    );
    assert_eq!(log.replay(HostSeq(1)), Vec::<Event>::new());
}

#[test]
fn an_open_commands_events_stage_until_it_closes() {
    let mut log = log();
    let _ = log.open();
    let (_, effects) = log.append(SessionId(1), EventKind::SessionReady);
    assert_eq!(
        effects,
        [Effect::Store(Write::Event(event(
            1,
            EventKind::SessionReady
        )))]
    );
    let (streamed, effects) = log.append(SessionId(1), delta("a"));
    assert_eq!(streamed.seq, HostSeq(1));
    assert_eq!(effects, Vec::<Effect>::new());
    assert_eq!(log.history(), Vec::<Event>::new());
    assert_eq!(
        log.close(),
        [
            Effect::Commit,
            Effect::Publish(event(1, EventKind::SessionReady)),
            Effect::Publish(event(1, delta("a"))),
        ]
    );
    assert_eq!(log.history(), [event(1, EventKind::SessionReady)]);
    assert_eq!(
        log.append(SessionId(1), EventKind::SessionReady).1,
        stored(2, EventKind::SessionReady)
    );
}

#[test]
fn a_barrier_commits_the_staged_events_and_keeps_the_command_open() {
    let mut log = log();
    let _ = log.open();
    let _ = log.append(SessionId(1), EventKind::SessionReady);
    assert_eq!(
        log.barrier(),
        [
            Effect::Commit,
            Effect::Publish(event(1, EventKind::SessionReady)),
            Effect::Open,
        ]
    );
    assert_eq!(log.barrier(), [Effect::Commit, Effect::Open]);
}

#[test]
fn an_idle_barrier_only_commits() {
    assert_eq!(log().barrier(), [Effect::Commit]);
}

#[test]
fn a_conversation_id_joins_the_open_transaction() {
    let mut log = log();
    let write = Write::Conversation(SessionId(1), String::from("c"));
    assert_eq!(
        log.conversation(SessionId(1), String::from("c")),
        [Effect::Open, Effect::Store(write.clone()), Effect::Commit]
    );
    let _ = log.open();
    assert_eq!(
        log.conversation(SessionId(1), String::from("c")),
        [Effect::Store(write)]
    );
}

#[test]
fn a_restored_log_continues_from_its_head() {
    let mut log = Log::new(Vec::from([event(7, EventKind::SessionReady)]));
    assert_eq!(log.append(SessionId(1), delta("a")).0.seq, HostSeq(7));
    assert_eq!(
        log.append(SessionId(1), EventKind::SessionReady).0.seq,
        HostSeq(8)
    );
    assert_eq!(log.head(), HostSeq(8));
}

#[test]
fn a_new_database_takes_the_format_and_another_format_does_not_open() {
    let refused = |found: u32| {
        Err(format!(
            "the log is format {found}, and this host reads format {FORMAT}; move the database aside to start over"
        ))
    };
    assert_eq!(
        [
            opens(true, None),
            opens(false, Some(FORMAT)),
            opens(false, None),
            opens(false, Some(2)),
            opens(false, Some(3)),
            opens(false, Some(7)),
        ],
        [
            Ok(true),
            Ok(false),
            refused(1),
            refused(2),
            refused(3),
            refused(7)
        ]
    );
}

#[test]
fn constant_cursor_saturates_without_wrapping() {
    const LAST: HostSeq = HostSeq(u64::MAX).next();
    assert_eq!(LAST, HostSeq(u64::MAX));
}

#[test]
fn stored_events_use_the_current_format()
-> Result<(), Box<dyn std::error::Error>> {
    let ready = Event {
        seq: HostSeq(1),
        session: SessionId(1),
        kind: EventKind::SessionReady,
    };
    assert_eq!(postcard::from_bytes::<Event>(&[1, 1, 1])?, ready);
    assert_eq!(postcard::to_allocvec(&ready)?, Vec::from([1, 1, 1]));
    Ok(())
}
