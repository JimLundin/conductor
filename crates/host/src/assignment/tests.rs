use proto::{
    ClientName, Event, EventKind, HostPath, HostSeq, InstanceId, IssueId,
    Provider, SessionId,
};

use super::{Assigned, Assignment, assigned};

const ISSUE: IssueId = IssueId(3);

fn event(session: u128, kind: EventKind) -> Event {
    Event {
        seq: HostSeq(1),
        session: SessionId(session),
        kind,
    }
}

fn created(session: u128, instance: InstanceId) -> Event {
    event(
        session,
        EventKind::SessionCreated {
            instance,
            provider: Provider::Claude,
            directory: HostPath(String::from("/src")),
            by: ClientName(String::from("phone")),
            model: None,
            effort: None,
        },
    )
}

fn own() -> InstanceId {
    InstanceId::own(Provider::Claude)
}

fn fold(events: &[Event]) -> Assigned {
    events.iter().fold(Assigned::default(), |so_far, event| {
        so_far.after(&event.kind)
    })
}

#[test]
fn a_session_is_assigned_its_issue_and_the_instance_it_was_created_in() {
    let events = [
        created(1, own()),
        event(1, EventKind::SessionReady),
        event(1, EventKind::OnIssue { issue: ISSUE }),
    ];
    assert_eq!(
        fold(&events).assignment(),
        Some(Assignment {
            issue: ISSUE,
            instance: own(),
        })
    );
}

#[test]
fn a_session_without_an_issue_or_an_instance_has_no_assignment() {
    let unassigned = fold(&[created(1, own())]);
    assert_eq!(unassigned.issue, None);
    assert_eq!(unassigned.instance, Some(own()));
    assert_eq!(unassigned.assignment(), None);
    let unknown = fold(&[event(1, EventKind::OnIssue { issue: ISSUE })]);
    assert_eq!(unknown.issue, Some(ISSUE));
    assert_eq!(unknown.assignment(), None);
}

#[test]
fn removing_a_session_takes_it_off_its_issue() {
    let removed = fold(&[
        created(1, own()),
        event(1, EventKind::OnIssue { issue: ISSUE }),
        event(1, EventKind::SessionRemoved { kept: None }),
    ]);
    assert_eq!(removed.issue, None);
    assert_eq!(removed.instance, Some(own()));
}

#[test]
fn each_session_folds_only_its_own_events() {
    let other = InstanceId::named(Provider::Codex, "https://gw");
    let sessions = assigned(&[
        created(1, own()),
        created(2, other.clone()),
        event(2, EventKind::OnIssue { issue: ISSUE }),
    ]);
    assert_eq!(sessions[&SessionId(1)].clone().assignment(), None);
    assert_eq!(
        sessions[&SessionId(2)].clone().assignment(),
        Some(Assignment {
            issue: ISSUE,
            instance: other,
        })
    );
}

#[test]
fn an_assignment_is_signed_by_its_instance() {
    let signed = |instance| Assignment {
        issue: ISSUE,
        instance,
    };
    assert_eq!(signed(own()).signer(), "claude");
    assert_eq!(
        signed(InstanceId::named(Provider::Codex, "https://gw")).signer(),
        "codex · https://gw"
    );
}
