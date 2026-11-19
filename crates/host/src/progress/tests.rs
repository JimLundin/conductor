use proto::{
    ClientName, Event, EventKind, HostPath, HostSeq, InstanceId, IssueId,
    Provider, SessionId, Task, TaskId, TaskStatus, TurnOutcome,
};

use super::{Progress, ends_turn, latest, progress};

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
            issue: ISSUE,
            instance: instance(),
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
fn the_watch_starts_after_the_log_and_looks_only_at_turn_ends() {
    let log = history(Vec::from([created(SESSION), ended(SESSION)]));
    assert_eq!(latest(&log), HostSeq(2));
    assert_eq!(latest(&[]), HostSeq(0));
    assert!(ends_turn(&log[1]));
    assert!(!ends_turn(&log[0]));
}
