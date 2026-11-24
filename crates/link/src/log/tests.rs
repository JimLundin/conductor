use crate::CommandId;
use agents::{Models, Output, Provider};
use host::log::{Effect as Logged, Write};
use host::log::{Epoch, Event, HostSeq};
use host::registry::{Availability, InstanceId, InstanceStatus, Instances};
use host::session::{EventKind, SessionId, SessionRefusal, SessionResult};

use crate::{CommandResult, Subscribe, Welcome};

use super::{Begun, Completion, Effect, Kind, Lease, Log};

fn log() -> Log {
    Log::new(String::from("test"), Epoch(5), Vec::new())
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

fn accepted() -> CommandResult {
    CommandResult::Session(SessionResult::Accepted)
}

const OPEN: Effect = Effect::Log(Logged::Open);
const COMMIT: Effect = Effect::Log(Logged::Commit);

fn published(event: Event) -> Effect {
    Effect::Log(Logged::Publish(event))
}

/// Begins `key` on an idle log whose store has no receipt for it.
fn execute(log: &mut Log, key: u128, kind: Kind) -> Lease {
    let (lease, effects) = log.begin(CommandId(key), kind);
    assert_eq!(effects, [Effect::Lookup(CommandId(key))]);
    let begun = Effect::Begun(lease, Begun::Execute(CommandId(key)));
    let expected = match kind {
        Kind::Session => Vec::from([OPEN, begun]),
        Kind::Receipt => Vec::from([begun]),
    };
    assert_eq!(log.receipt(None), expected);
    lease
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
        log.welcome(listed.clone(), host::registry::Catalog::default(), None),
        Welcome {
            host: String::from("test"),
            epoch: Epoch(5),
            head_seq: HostSeq(1),
            instances: listed,
            catalog: host::registry::Catalog::default(),
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
    let lease = execute(&mut log, 1, Kind::Session);
    let (_, effects) = log.append(SessionId(1), EventKind::SessionReady);
    assert_eq!(
        effects,
        [Effect::Log(Logged::Store(Write::Event(event(
            1,
            EventKind::SessionReady
        ))))]
    );
    let (streamed, effects) = log.append(SessionId(1), delta("a"));
    assert_eq!(streamed.seq, HostSeq(1));
    assert_eq!(effects, Vec::<Effect>::new());
    assert_eq!(log.history(), Vec::<Event>::new());
    let (finished, effects) = log.finish(CommandId(1), accepted());
    assert_ne!(finished, lease);
    assert_eq!(
        effects,
        [
            Effect::Receipt(CommandId(1), accepted()),
            COMMIT,
            published(event(1, EventKind::SessionReady)),
            published(event(1, delta("a"))),
            Effect::Finished(finished),
        ]
    );
    assert_eq!(log.history(), [event(1, EventKind::SessionReady)]);
}

#[test]
fn a_barrier_keeps_the_command_running() {
    let mut log = log();
    let _ = execute(&mut log, 1, Kind::Session);
    let _ = log.append(SessionId(1), EventKind::SessionReady);
    assert_eq!(
        log.barrier(),
        [COMMIT, published(event(1, EventKind::SessionReady)), OPEN,]
    );
    assert_eq!(log.abort(CommandId(1)), [Effect::Stop]);
}

#[test]
fn dropping_a_running_command_stops_the_log_and_a_finished_one_does_not() {
    let mut log = log();
    let _ = execute(&mut log, 1, Kind::Receipt);
    assert_eq!(log.abort(CommandId(1)), [Effect::Stop]);
    assert_eq!(log.abort(CommandId(2)), Vec::<Effect>::new());
    let _ = log.finish(CommandId(1), accepted());
    assert_eq!(log.abort(CommandId(1)), Vec::<Effect>::new());
}

#[test]
fn concurrent_session_retries_share_a_retryable_result() {
    let mut log = log();
    let _ = execute(&mut log, 1, Kind::Session);
    let (retry, effects) = log.begin(CommandId(1), Kind::Session);
    assert_eq!(effects, Vec::<Effect>::new());
    let result = CommandResult::Session(SessionResult::Refused(
        SessionRefusal::NoSuchDirectory,
    ));
    let (finished, effects) =
        log.finish(CommandId(1), Completion::Retryable(result.clone()));
    assert_eq!(
        effects,
        [
            COMMIT,
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
    let (held, effects) = log.finish(CommandId(23), accepted());
    assert_eq!(effects, Vec::<Effect>::new());
    let (retry, effects) = log.begin(CommandId(23), Kind::Receipt);
    assert_eq!(effects, Vec::<Effect>::new());
    let (finished, effects) = log.finish(CommandId(24), accepted());
    assert_eq!(
        effects,
        [
            Effect::Receipt(CommandId(24), accepted()),
            COMMIT,
            Effect::Finished(finished),
            OPEN,
            Effect::Receipt(CommandId(23), accepted()),
            COMMIT,
            Effect::Begun(retry, Begun::Cached(accepted())),
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
    let (finished, effects) = log.finish(CommandId(23), accepted());
    assert_eq!(
        effects,
        [
            OPEN,
            Effect::Receipt(CommandId(23), accepted()),
            COMMIT,
            Effect::Begun(retry, Begun::Cached(accepted())),
            Effect::Finished(finished),
        ]
    );
}

#[test]
fn a_conversation_id_joins_the_active_commands_transaction() {
    let mut log = log();
    let _ = execute(&mut log, 1, Kind::Session);
    assert_eq!(
        log.conversation(SessionId(1), String::from("c")),
        [Effect::Log(Logged::Store(Write::Conversation(
            SessionId(1),
            String::from("c")
        )))]
    );
}

#[test]
fn a_receipt_nobody_asked_for_does_nothing() {
    assert_eq!(log().receipt(Some(accepted())), Vec::<Effect>::new());
}

#[test]
fn a_restored_log_welcomes_with_its_head() {
    let mut log = Log::new(
        String::from("test"),
        Epoch(1),
        Vec::from([event(7, EventKind::SessionReady)]),
    );
    let _ = log.append(SessionId(1), EventKind::SessionReady);
    assert_eq!(
        log.welcome(
            instances(Availability::Available {
                auth: agents::Auth::SignedIn,
                models: Models::none(),
            }),
            host::registry::Catalog::default(),
            None,
        )
        .head_seq,
        HostSeq(8)
    );
}

/// A Claude instance with `availability`, and one Codex instance that
/// can't run.
fn instances(availability: Availability) -> Instances {
    let status = |availability| InstanceStatus { availability };
    Instances(
        [
            (
                InstanceId::own(Provider::new("claude")),
                status(availability),
            ),
            (
                InstanceId::own(Provider::new("codex")),
                status(Availability::Unavailable {
                    reason: String::from("no codex"),
                }),
            ),
        ]
        .into(),
    )
}
