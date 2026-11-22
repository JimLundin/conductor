use proto::{
    ClientName, Event, EventKind, HostPath, HostSeq, InstanceId, IssueId,
    Provider, SessionId,
};

use super::{Assigned, Assignment, assigned};

const ISSUE: IssueId = IssueId::new(0, 3);

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
        fold(&events),
        Assigned {
            issue: Some(ISSUE),
            instance: Some(own()),
        }
    );
}

#[test]
fn a_session_without_an_issue_or_an_instance_has_no_assignment() {
    let unassigned = fold(&[created(1, own())]);
    assert_eq!(unassigned.issue, None);
    assert_eq!(unassigned.instance, Some(own()));
    let unknown = fold(&[event(1, EventKind::OnIssue { issue: ISSUE })]);
    assert_eq!(unknown.issue, Some(ISSUE));
    assert_eq!(unknown.instance, None);
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
fn a_session_is_on_an_issue_in_its_instance_only() {
    let on = fold(&[
        created(1, own()),
        event(1, EventKind::OnIssue { issue: ISSUE }),
    ]);
    assert!(on.is(ISSUE, &own()));
    assert!(!on.is(IssueId::new(0, 4), &own()));
    assert!(!on.is(ISSUE, &InstanceId::named(Provider::Claude, "https://gw")));
}

#[test]
fn each_session_folds_only_its_own_events() {
    let other = InstanceId::named(Provider::Codex, "https://gw");
    let sessions = assigned(&[
        created(1, own()),
        created(2, other.clone()),
        event(2, EventKind::OnIssue { issue: ISSUE }),
    ]);
    assert_eq!(sessions[&SessionId(1)].issue, None);
    assert_eq!(
        sessions[&SessionId(2)],
        Assigned {
            issue: Some(ISSUE),
            instance: Some(other),
        }
    );
}

#[test]
fn an_assignment_is_signed_by_its_instance() {
    let signed = |instance| Assignment {
        issue: ISSUE,
        instance,
    };
    assert_eq!(signed(own()).signer().0, "claude");
    assert_eq!(
        signed(InstanceId::named(Provider::Codex, "https://gw"))
            .signer()
            .0,
        "codex · https://gw"
    );
}
