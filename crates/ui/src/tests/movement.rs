use client::link::{LinkState, Update};
use proto::movement::{MoveId, Phase};
use proto::{
    CommandBody, Epoch, Event, EventKind, Feed, HostPath, HostSeq, Instances,
    Rejection, Workspace,
};

use super::{
    HOST, SESSION, answer, connection, created, event, pane, run, said, screen,
    sent, text, ticket, welcomed,
};
use crate::{
    Footer, HostId, Intent, Listing, MoveIntent, MovePanel, Opening, Place,
    Saved, SessionIntent, State, Stored, encode,
};

const TARGET: HostId = HostId([2; 32]);
const MOVE: MoveId = MoveId(4);

fn linked(host: HostId, seq: u64, kind: EventKind) -> Intent {
    Intent::Link(
        host,
        Update::Feed(Feed::Event(Event {
            seq: HostSeq(seq),
            session: SESSION,
            kind,
        })),
    )
}

fn owned() -> State {
    let kept = [
        Saved {
            ticket: ticket(),
            name: None,
        },
        Saved {
            ticket: proto::Ticket {
                host: TARGET.0,
                ..ticket()
            },
            name: None,
        },
    ];
    let empty = State::new(Opening {
        stored: Stored {
            hosts: Some(encode(&kept)),
            ticket: None,
        },
        joined: None,
    });
    run(
        empty,
        Vec::from([
            welcomed(HOST, "desk", Instances::default(), None),
            welcomed(TARGET, "laptop", Instances::default(), None),
            connection(HOST, LinkState::Live, None),
            connection(TARGET, LinkState::Live, None),
            Intent::Link(
                HOST,
                Update::Feed(Feed::MoveTargets(Vec::from([TARGET.0]))),
            ),
            event(1, created()),
            event(2, EventKind::SessionReady),
            event(
                3,
                EventKind::WorkspaceCreated {
                    workspace: Workspace {
                        repository: HostPath(text("/repo")),
                        original: HostPath(text("/repo")),
                        worktree: HostPath(text("/work")),
                        directory: HostPath(text("/work")),
                        branch: text("session-7"),
                    },
                },
            ),
            Intent::Show(Place::Session(HOST, SESSION)),
        ]),
    )
    .0
}

const fn moving() -> EventKind {
    EventKind::SessionMoving {
        to: TARGET.0,
        move_id: MOVE,
        phase: Phase::Settling,
    }
}

#[test]
fn a_failed_move_keeps_the_source_locked_and_retries_the_same_destination_and_id()
 {
    let initial = owned();
    assert!(
        matches!(pane(&initial).map(|shown| shown.movement), Some(MovePanel::Offer(hosts)) if matches!(hosts.as_slice(), [target] if target.id == TARGET))
    );
    let start = Intent::Move(MoveIntent::To(TARGET));
    let (pending, requested) = run(initial, Vec::from([start.clone(), start]));
    assert_eq!(
        sent(&requested),
        [CommandBody::MoveSession {
            session: SESSION,
            to: TARGET.0,
            retry: None
        }]
    );
    assert_eq!(
        pane(&pending).map(|shown| shown.movement),
        Some(MovePanel::Starting)
    );
    let (failed, _) = run(
        pending,
        Vec::from([
            event(4, moving()),
            event(
                5,
                EventKind::SessionMoveFailed {
                    move_id: MOVE,
                    reason: text("connection lost"),
                    resumable: false,
                },
            ),
            answer(&requested, Ok(proto::CommandResult::Accepted)),
        ]),
    );
    assert!(matches!(pane(&failed), Some(shown)
        if matches!(shown.footer, Footer::Moving)
        && matches!(shown.movement, MovePanel::Failed { can_retry: true, .. })));
    let (_, retried) = run(
        failed,
        Vec::from([
            Intent::Session(SessionIntent::Continue),
            Intent::Move(MoveIntent::Retry),
        ]),
    );
    assert_eq!(
        sent(&retried),
        [CommandBody::MoveSession {
            session: SESSION,
            to: TARGET.0,
            retry: Some(MOVE)
        }]
    );
}

fn shown_hosts(state: &State) -> Vec<HostId> {
    screen(state)
        .into_iter()
        .flat_map(|screen| screen.list.groups)
        .filter_map(|group| match group.listing {
            Listing::Rows(_) => Some(group.host),
            Listing::Empty(_) => None,
        })
        .collect::<Vec<_>>()
}

#[test]
fn the_app_shows_one_session_and_follows_it_only_after_the_target_history_is_complete()
 {
    let (incomplete, _) = run(
        owned(),
        Vec::from([
            event(4, moving()),
            linked(
                TARGET,
                1,
                EventKind::SessionArrived {
                    from: HOST.0,
                    move_id: MOVE,
                    source_epoch: Epoch(1),
                    source_last_seq: HostSeq(4),
                },
            ),
            linked(TARGET, 2, created()),
            linked(TARGET, 3, said("copied conversation")),
            event(
                5,
                EventKind::SessionMoved {
                    to: TARGET.0,
                    move_id: MOVE,
                },
            ),
        ]),
    );
    assert_eq!(shown_hosts(&incomplete), [HOST]);
    assert!(matches!(
        pane(&incomplete).map(|shown| shown.movement),
        Some(MovePanel::Moved(_))
    ));
    let (arrived, _) = run(
        incomplete,
        Vec::from([
            linked(TARGET, 4, EventKind::SessionArrivalReady { move_id: MOVE }),
            Intent::Show(Place::Session(HOST, SESSION)),
        ]),
    );
    assert_eq!(shown_hosts(&arrived), [TARGET]);
    assert!(
        matches!(pane(&arrived), Some(shown) if shown.subtitle.ends_with("on laptop") && !shown.entries.is_empty())
    );
}

#[test]
fn a_host_refusal_unlocks_the_source_without_claiming_a_transfer() {
    let (pending, requested) =
        run(owned(), Vec::from([Intent::Move(MoveIntent::To(TARGET))]));
    let (refused, _) = run(
        pending,
        Vec::from([answer(
            &requested,
            Ok(proto::CommandResult::Rejected(Rejection::TurnInProgress)),
        )]),
    );
    assert!(matches!(pane(&refused), Some(shown)
        if shown.notice.is_some()
        && matches!(shown.movement, MovePanel::Offer(_))
        && !matches!(shown.footer, Footer::Moving)));
}

#[test]
fn a_lost_command_reply_can_retry_a_known_in_progress_move() {
    let (pending, requested) =
        run(owned(), Vec::from([Intent::Move(MoveIntent::To(TARGET))]));
    let (uncertain, _) = run(
        pending,
        Vec::from([
            event(4, moving()),
            answer(&requested, Err(text("connection lost"))),
        ]),
    );
    assert!(matches!(
        pane(&uncertain).map(|shown| shown.movement),
        Some(MovePanel::Moving {
            can_retry: true,
            ..
        })
    ));
    let (_, retry) =
        run(uncertain, Vec::from([Intent::Move(MoveIntent::Retry)]));
    assert_eq!(
        sent(&retry),
        [CommandBody::MoveSession {
            session: SESSION,
            to: TARGET.0,
            retry: Some(MOVE)
        }]
    );
}

#[test]
fn a_definite_pre_transfer_failure_leaves_normal_source_actions_available() {
    let (failed, _) = run(
        owned(),
        Vec::from([
            event(4, moving()),
            event(
                5,
                EventKind::SessionMoveFailed {
                    move_id: MOVE,
                    reason: text("CLI pins differ"),
                    resumable: true,
                },
            ),
        ]),
    );
    assert!(matches!(pane(&failed), Some(shown)
        if matches!(shown.movement, MovePanel::Failed { can_retry: true, .. })
        && matches!(&shown.footer, Footer::Composer(form) if !form.disabled)));
}

#[test]
fn moving_back_replaces_the_old_copy_after_the_new_arrival_is_ready() {
    let back = MoveId(5);
    let (returning, _) = run(
        owned(),
        Vec::from([
            event(4, said("older local copy")),
            event(5, moving()),
            event(
                6,
                EventKind::SessionMoved {
                    to: TARGET.0,
                    move_id: MOVE,
                },
            ),
            linked(
                TARGET,
                1,
                EventKind::SessionArrived {
                    from: HOST.0,
                    move_id: MOVE,
                    source_epoch: Epoch(1),
                    source_last_seq: HostSeq(5),
                },
            ),
            linked(TARGET, 2, created()),
            linked(TARGET, 3, EventKind::SessionArrivalReady { move_id: MOVE }),
            linked(
                TARGET,
                4,
                EventKind::SessionMoving {
                    to: HOST.0,
                    move_id: back,
                    phase: Phase::Committing,
                },
            ),
            event(
                7,
                EventKind::SessionArrived {
                    from: TARGET.0,
                    move_id: back,
                    source_epoch: Epoch(1),
                    source_last_seq: HostSeq(4),
                },
            ),
            event(8, created()),
            event(9, said("new imported copy")),
            event(10, EventKind::SessionArrivalReady { move_id: MOVE }),
        ]),
    );
    assert_eq!(shown_hosts(&returning), [TARGET]);
    let (returned, _) = run(
        returning,
        Vec::from([
            event(11, EventKind::SessionArrivalReady { move_id: back }),
            linked(
                TARGET,
                5,
                EventKind::SessionMoved {
                    to: HOST.0,
                    move_id: back,
                },
            ),
        ]),
    );
    assert_eq!(shown_hosts(&returned), [HOST]);
    assert!(matches!(pane(&returned), Some(shown)
        if shown.subtitle.ends_with("on desk") && shown.entries.len() == 1));
}
