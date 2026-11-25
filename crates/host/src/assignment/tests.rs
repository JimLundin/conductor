use crate::log::{Event, HostSeq};
use crate::registry::InstanceId;
use crate::session::SessionDirectory;
use crate::session::{EventKind, SessionId};
use agents::{ClientName, Provider};
use home::IssueId;

use super::{Assigned, Assignment, assigned};

fn issue_id() -> Result<IssueId, &'static str> {
    IssueId::new("local", "00000003").ok_or("invalid fixture identity")
}

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
            directory: SessionDirectory(String::from("/src")),
            by: ClientName(String::from("phone")),
            model: None,
            effort: None,
        },
    )
}

fn own() -> InstanceId {
    InstanceId::own(Provider::new("claude"))
}

fn fold(events: &[Event]) -> Assigned {
    events.iter().fold(Assigned::default(), |so_far, event| {
        so_far.after(&event.kind)
    })
}

#[test]
fn a_session_is_assigned_its_issue_and_the_instance_it_was_created_in()
-> Result<(), &'static str> {
    let events = [
        created(1, own()),
        event(1, EventKind::SessionReady),
        event(1, EventKind::OnIssue { issue: issue_id()? }),
    ];
    assert_eq!(
        fold(&events),
        Assigned {
            issue: Some(issue_id()?),
            instance: Some(own()),
        }
    );
    Ok(())
}

#[test]
fn a_session_without_an_issue_or_an_instance_has_no_assignment()
-> Result<(), &'static str> {
    let unassigned = fold(&[created(1, own())]);
    assert_eq!(unassigned.issue, None);
    assert_eq!(unassigned.instance, Some(own()));
    let unknown = fold(&[event(1, EventKind::OnIssue { issue: issue_id()? })]);
    assert_eq!(unknown.issue, Some(issue_id()?));
    assert_eq!(unknown.instance, None);
    Ok(())
}

#[test]
fn removing_a_session_takes_it_off_its_issue() -> Result<(), &'static str> {
    let removed = fold(&[
        created(1, own()),
        event(1, EventKind::OnIssue { issue: issue_id()? }),
        event(1, EventKind::SessionRemoved { kept: None }),
    ]);
    assert_eq!(removed.issue, None);
    assert_eq!(removed.instance, Some(own()));
    Ok(())
}

#[test]
fn a_session_is_on_an_issue_in_its_instance_only() -> Result<(), &'static str> {
    let on = fold(&[
        created(1, own()),
        event(1, EventKind::OnIssue { issue: issue_id()? }),
    ]);
    assert!(on.is(issue_id()?, &own()));
    assert!(!on.is(
        IssueId::new("local", "00000004").ok_or("invalid fixture identity")?,
        &own()
    ));
    assert!(!on.is(
        issue_id()?,
        &InstanceId::named(Provider::new("claude"), "https://gw")
    ));
    Ok(())
}

#[test]
fn each_session_folds_only_its_own_events() -> Result<(), &'static str> {
    let other = InstanceId::named(Provider::new("codex"), "https://gw");
    let sessions = assigned(&[
        created(1, own()),
        created(2, other.clone()),
        event(2, EventKind::OnIssue { issue: issue_id()? }),
    ]);
    assert_eq!(sessions[&SessionId(1)].issue, None);
    assert_eq!(
        sessions[&SessionId(2)],
        Assigned {
            issue: Some(issue_id()?),
            instance: Some(other),
        }
    );
    Ok(())
}

#[test]
fn an_assignment_is_signed_by_its_instance() -> Result<(), &'static str> {
    let issue = issue_id()?;
    let signed = |instance| Assignment {
        issue: issue.clone(),
        instance,
    };
    assert_eq!(signed(own()).signer().0, "claude");
    assert_eq!(
        signed(InstanceId::named(Provider::new("codex"), "https://gw"))
            .signer()
            .0,
        "codex · https://gw"
    );
    Ok(())
}
