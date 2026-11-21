use proto::{
    ClientName, Event, EventKind, HostPath, HostSeq, InstanceId, IssueId,
    Provider, SessionId, Task, TaskId, TaskStatus, TurnOutcome,
};

use super::{Progress, edit_id, ends_turn, progress};
use crate::assignment::Assignment;

const SESSION: SessionId = SessionId(1);
const ISSUE: IssueId = IssueId(7);

fn instance() -> InstanceId {
    InstanceId::own(Provider::Claude)
}

fn history(kinds: Vec<(SessionId, EventKind)>) -> Vec<Event> {
    kinds
        .into_iter()
        .zip(1..)
        .map(|((session, kind), seq)| Event {
            seq: HostSeq(seq),
            session,
            kind,
        })
        .collect()
}

fn created(session: SessionId) -> (SessionId, EventKind) {
    (
        session,
        EventKind::SessionCreated {
            instance: instance(),
            provider: Provider::Claude,
            directory: HostPath(String::from("/work")),
            by: ClientName(String::from("phone")),
            model: None,
            effort: None,
        },
    )
}

fn tasks(
    session: SessionId,
    of: &[(u128, &str, TaskStatus)],
) -> (SessionId, EventKind) {
    (
        session,
        EventKind::TasksChanged {
            tasks: of
                .iter()
                .map(|(id, text, status)| Task {
                    id: TaskId(*id),
                    text: String::from(*text),
                    status: *status,
                    holder: session,
                })
                .collect(),
        },
    )
}

fn ended(session: SessionId) -> (SessionId, EventKind) {
    (
        session,
        EventKind::TurnCompleted {
            outcome: TurnOutcome::Completed,
        },
    )
}

fn at(history: &[Event], seq: u64) -> Option<Progress> {
    let event = history.iter().find(|event| event.seq == HostSeq(seq))?;
    progress(history, event)
}

#[test]
fn a_turn_notes_the_checklist_items_its_agent_finished_in_it() {
    let done = TaskStatus::Completed;
    let doing = TaskStatus::InProgress;
    let log = history(Vec::from([
        created(SESSION),
        (SESSION, EventKind::OnIssue { issue: ISSUE }),
        tasks(SESSION, &[(1, "plan", done), (2, "build", doing)]),
        ended(SESSION),
        tasks(
            SESSION,
            &[(1, "plan", done), (2, "build", done), (3, "test", done)],
        ),
        created(SessionId(2)),
        tasks(SessionId(2), &[(9, "elsewhere", done)]),
        ended(SESSION),
        ended(SESSION),
    ]));
    let noted = |text: &str| {
        Some(Progress {
            assignment: Assignment {
                issue: ISSUE,
                instance: instance(),
            },
            text: String::from(text),
        })
    };
    assert_eq!(at(&log, 4), noted("Done: plan."));
    assert_eq!(at(&log, 8), noted("Done: build; test."));
    assert_eq!(
        at(&log, 9),
        None,
        "a turn that finished nothing notes nothing"
    );
    assert_eq!(at(&log, 5), None, "only a turn's end notes");
}

#[test]
fn a_session_on_no_issue_or_taken_off_it_notes_nothing() {
    let done = TaskStatus::Completed;
    let loose = history(Vec::from([
        created(SESSION),
        tasks(SESSION, &[(1, "plan", done)]),
        ended(SESSION),
    ]));
    assert_eq!(at(&loose, 3), None);
    let removed = history(Vec::from([
        created(SESSION),
        (SESSION, EventKind::OnIssue { issue: ISSUE }),
        tasks(SESSION, &[(1, "plan", done)]),
        (SESSION, EventKind::SessionRemoved { kept: None }),
        ended(SESSION),
    ]));
    assert_eq!(at(&removed, 5), None);
}

#[test]
fn only_a_completed_turn_is_eligible_for_a_progress_note() {
    let log = history(Vec::from([created(SESSION), ended(SESSION)]));
    assert!(ends_turn(&log[1]));
    assert!(!ends_turn(&log[0]));
}

#[test]
fn a_progress_edit_is_named_by_its_epoch_and_turn() {
    let epoch = proto::Epoch(1);
    let turn = HostSeq(2);
    let first = edit_id(epoch, turn);
    assert_eq!(edit_id(epoch, turn), first);
    assert_ne!(edit_id(proto::Epoch(2), turn), first);
    assert_ne!(edit_id(epoch, HostSeq(3)), first);
}

#[test]
fn an_unfinished_turn_carries_its_items_on_and_a_summary_turn_notes_none() {
    let done = TaskStatus::Completed;
    let log = history(Vec::from([
        created(SESSION),
        (SESSION, EventKind::OnIssue { issue: ISSUE }),
        tasks(SESSION, &[(1, "plan", done)]),
        (
            SESSION,
            EventKind::TurnCompleted {
                outcome: TurnOutcome::Interrupted,
            },
        ),
        ended(SESSION),
        (SESSION, EventKind::FinishStarted),
        tasks(SESSION, &[(1, "plan", done), (2, "wrap up", done)]),
        ended(SESSION),
    ]));
    assert_eq!(at(&log, 4), None, "an interrupted turn notes nothing");
    assert_eq!(
        at(&log, 5),
        Some(Progress {
            assignment: Assignment {
                issue: ISSUE,
                instance: instance(),
            },
            text: String::from("Done: plan."),
        })
    );
    assert_eq!(at(&log, 8), None, "the summary stays the last note");
}
