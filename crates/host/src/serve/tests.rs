use proto::{
    ClientName, Effort, HostSeq, InstanceId, Models, Provider, SessionCommand,
    SessionId,
};

use crate::session::Order;

use super::{Relaunch, SessionRoute, session};

fn phone() -> ClientName {
    ClientName(String::from("phone"))
}

#[test]
fn a_diff_and_a_message_are_orders_to_the_running_session() {
    assert_eq!(
        session(
            SessionCommand::GetDiff {
                session: SessionId(1),
                snapshot: HostSeq(2),
            },
            phone()
        ),
        SessionRoute::Order(SessionId(1), Order::Diff(HostSeq(2)))
    );
    assert_eq!(
        session(
            SessionCommand::SendMessage {
                session: SessionId(1),
                text: String::from("hi"),
            },
            phone()
        ),
        SessionRoute::Order(
            SessionId(1),
            Order::Send {
                text: String::from("hi"),
                by: phone(),
            }
        )
    );
}

#[test]
fn a_session_may_start_in_a_project() {
    let project = || proto::ProjectId::new(1);
    assert_eq!(
        session(
            SessionCommand::Start {
                instance: InstanceId::own(Provider::new("claude")),
                at: proto::StartAt::Project(project()),
                model: None,
                effort: Some(Effort::High),
            },
            phone()
        ),
        SessionRoute::Start {
            instance: InstanceId::own(Provider::new("claude")),
            at: proto::StartAt::Project(project()),
            choice: (None, Some(Effort::High)),
            by: phone(),
        }
    );
}

#[test]
fn relaunching_orders_take_the_offered_models() {
    assert_eq!(
        session(
            SessionCommand::SetEffort {
                session: SessionId(1),
                effort: Effort::High,
            },
            phone(),
        ),
        SessionRoute::Relaunch(SessionId(1), Relaunch::Effort(Effort::High))
    );
    assert_eq!(
        Relaunch::Continue.order(Models::none()),
        Order::Start {
            offered: Models::none()
        }
    );
}

#[test]
fn finishing_runs_on_the_sessions_issue_by_its_client() {
    assert_eq!(
        session(
            SessionCommand::Finish {
                session: SessionId(3),
            },
            phone()
        ),
        SessionRoute::Finish(SessionId(3), phone())
    );
}
