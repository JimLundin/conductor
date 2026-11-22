use proto::{
    Availability, CommandId, CommandResult, Epoch, Event, EventKind, HostSeq,
    InstanceId, InstanceStatus, Instances, Models, Provider, SessionId,
    Subscribe, Welcome,
};

use super::{Begun, Effect, FORMAT, Kind, Log, Ticket, Write, opens};

fn log() -> Log {
    Log::new(String::from("test"), Epoch(5), Vec::new())
}

fn delta(text: &str) -> EventKind {
    EventKind::AssistantDelta {
        text: String::from(text),
    }
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

/// Begins `key` on an idle log whose store has no receipt for it.
fn execute(log: &mut Log, key: u128, kind: Kind) -> Ticket {
    let (ticket, effects) = log.begin(CommandId(key), kind);
    assert_eq!(effects, [Effect::Lookup(CommandId(key))]);
    let begun = Effect::Begun(ticket, Begun::Execute(CommandId(key)));
    let expected = match kind {
        Kind::Session => Vec::from([Effect::Open, begun]),
        Kind::Receipt => Vec::from([begun]),
    };
    assert_eq!(log.receipt(None), expected);
    ticket
}

#[test]
fn stored_events_take_the_next_seq_and_deltas_repeat_the_last() {
    let mut log = log();
    let (ready, effects) = log.append(SessionId(1), EventKind::SessionReady);
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
}

#[test]
fn session_admission_sees_staged_events_while_replay_waits_for_the_receipt() {
    let mut log = log();
    let _ = log.append(SessionId(1), EventKind::SessionReady);
    let _ = execute(&mut log, 1, Kind::Session);
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
fn the_welcome_names_the_head_and_a_subscription_needs_the_epoch() {
    let mut log = log();
    let _ = log.append(SessionId(1), EventKind::SessionReady);
    let _ = log.append(SessionId(1), delta("a"));
    let listed = instances(Availability::Unavailable {
        reason: String::from("no claude"),
    });
    assert_eq!(
        log.welcome(listed.clone(), proto::Catalog::default(), None),
        Welcome {
            host: String::from("test"),
            epoch: Epoch(5),
            head_seq: HostSeq(1),
            instances: listed,
            catalog: proto::Catalog::default(),
            home: None,
        }
    );
    let from = |epoch| Subscribe {
        epoch: Epoch(epoch),
        after_seq: HostSeq(0),
    };
    assert_eq!(
        log.resume(&from(5)),
        Some(Vec::from([event(1, EventKind::SessionReady)]))
    );
    assert_eq!(log.resume(&from(4)), None);
}

#[test]
fn a_commands_events_stage_until_its_receipt_commits_them() {
    let mut log = log();
    let ticket = execute(&mut log, 1, Kind::Session);
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
    let (finished, effects) = log.finish(CommandId(1), CommandResult::Accepted);
    assert_ne!(finished, ticket);
    assert_eq!(
        effects,
        [
            Effect::Store(Write::Receipt(
                CommandId(1),
                CommandResult::Accepted
            )),
            Effect::Commit,
            Effect::Publish(event(1, EventKind::SessionReady)),
            Effect::Publish(event(1, delta("a"))),
            Effect::Finished(finished),
        ]
    );
    assert_eq!(log.history(), [event(1, EventKind::SessionReady)]);
}

#[test]
fn a_barrier_commits_the_staged_events_and_keeps_the_command_open() {
    let mut log = log();
    let _ = execute(&mut log, 1, Kind::Session);
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
    assert_eq!(log.abort(CommandId(1)), [Effect::Stop]);
}

#[test]
fn an_idle_barrier_only_commits() {
    assert_eq!(log().barrier(), [Effect::Commit]);
}

#[test]
fn dropping_a_running_command_stops_the_log_and_a_finished_one_does_not() {
    let mut log = log();
    let _ = execute(&mut log, 1, Kind::Receipt);
    assert_eq!(log.abort(CommandId(1)), [Effect::Stop]);
    assert_eq!(log.abort(CommandId(2)), Vec::<Effect>::new());
    let _ = log.finish(CommandId(1), CommandResult::Accepted);
    assert_eq!(log.abort(CommandId(1)), Vec::<Effect>::new());
}

#[test]
fn concurrent_session_retries_share_a_retryable_result() {
    let mut log = log();
    let _ = execute(&mut log, 1, Kind::Session);
    let (retry, effects) = log.begin(CommandId(1), Kind::Session);
    assert_eq!(effects, Vec::<Effect>::new());
    let result = CommandResult::Rejected(proto::Rejection::NoSuchDirectory);
    let (finished, effects) =
        log.finish(CommandId(1), super::Completion::Retryable(result.clone()));
    assert_eq!(
        effects,
        [
            Effect::Commit,
            Effect::Begun(retry, Begun::Cached(result)),
            Effect::Finished(finished),
        ],
    );
    let _ = execute(&mut log, 1, Kind::Session);
}

#[test]
fn a_receipt_finish_waits_for_the_active_command_to_commit() {
    let mut log = log();
    let _ = execute(&mut log, 23, Kind::Receipt);
    let _ = execute(&mut log, 24, Kind::Session);
    let (held, effects) = log.finish(CommandId(23), CommandResult::Accepted);
    assert_eq!(effects, Vec::<Effect>::new());
    let (retry, effects) = log.begin(CommandId(23), Kind::Receipt);
    assert_eq!(effects, Vec::<Effect>::new());
    let (finished, effects) =
        log.finish(CommandId(24), CommandResult::Accepted);
    assert_eq!(
        effects,
        [
            Effect::Store(Write::Receipt(
                CommandId(24),
                CommandResult::Accepted
            )),
            Effect::Commit,
            Effect::Finished(finished),
            Effect::Open,
            Effect::Store(Write::Receipt(
                CommandId(23),
                CommandResult::Accepted
            )),
            Effect::Commit,
            Effect::Begun(retry, Begun::Cached(CommandResult::Accepted)),
            Effect::Finished(held),
        ]
    );
}

#[test]
fn a_receipt_retry_waits_for_the_first_run_and_takes_its_result() {
    let mut log = log();
    let _ = execute(&mut log, 23, Kind::Receipt);
    let (retry, effects) = log.begin(CommandId(23), Kind::Receipt);
    assert_eq!(effects, [Effect::Lookup(CommandId(23))]);
    assert_eq!(log.receipt(None), Vec::<Effect>::new());
    let (finished, effects) =
        log.finish(CommandId(23), CommandResult::Accepted);
    assert_eq!(
        effects,
        [
            Effect::Open,
            Effect::Store(Write::Receipt(
                CommandId(23),
                CommandResult::Accepted
            )),
            Effect::Commit,
            Effect::Begun(retry, Begun::Cached(CommandResult::Accepted)),
            Effect::Finished(finished),
        ]
    );
}

#[test]
fn a_conversation_id_joins_the_active_transaction() {
    let mut log = log();
    let write = Write::Conversation(SessionId(1), String::from("c"));
    assert_eq!(
        log.conversation(SessionId(1), String::from("c")),
        [Effect::Open, Effect::Store(write.clone()), Effect::Commit]
    );
    let _ = execute(&mut log, 1, Kind::Session);
    assert_eq!(
        log.conversation(SessionId(1), String::from("c")),
        [Effect::Store(write)]
    );
}

#[test]
fn a_receipt_nobody_asked_for_does_nothing() {
    assert_eq!(
        log().receipt(Some(CommandResult::Accepted)),
        Vec::<Effect>::new()
    );
}

#[test]
fn a_restored_log_continues_from_its_head() {
    let mut log = Log::new(
        String::from("test"),
        Epoch(1),
        Vec::from([event(7, EventKind::SessionReady)]),
    );
    assert_eq!(log.append(SessionId(1), delta("a")).0.seq, HostSeq(7));
    assert_eq!(
        log.append(SessionId(1), EventKind::SessionReady).0.seq,
        HostSeq(8)
    );
    assert_eq!(
        log.welcome(
            instances(Availability::Available {
                auth: proto::Auth::SignedIn,
                models: Models::none(),
            }),
            proto::Catalog::default(),
            None,
        )
        .head_seq,
        HostSeq(8)
    );
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
        ],
        [Ok(true), Ok(false), refused(1), refused(2), refused(3)]
    );
}

/// A Claude instance with `availability`, and one Codex instance that
/// can't run.
fn instances(availability: Availability) -> Instances {
    let status = |availability| InstanceStatus { availability };
    Instances(
        [
            (InstanceId::own(Provider::Claude), status(availability)),
            (
                InstanceId::own(Provider::Codex),
                status(Availability::Unavailable {
                    reason: String::from("no codex"),
                }),
            ),
        ]
        .into(),
    )
}
