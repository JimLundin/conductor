use proto::{
    AuthToken, ClientName, Command, CommandId, Effort, Epoch, Hello,
    HomeCommand, HostSeq, InstanceCommand, InstanceId, IssueChange, IssueId,
    Models, NewIssue, Open, Provider, SessionCommand, SessionId, SignInCode,
    SignInCommand, Subscribe,
};

use crate::log::Kind;
use crate::session::Order;

use super::{
    Admission, Relaunch, SessionRoute, Stream, admit, ledger, session, stream,
};

const TOKEN: AuthToken = AuthToken::from_bytes([7; 16]);

fn phone() -> ClientName {
    ClientName(String::from("phone"))
}

fn hello(token: AuthToken) -> Open {
    Open::Hello(Hello {
        client: phone(),
        token,
    })
}

fn subscribe() -> Subscribe {
    Subscribe {
        epoch: Epoch(1),
        after_seq: HostSeq(0),
    }
}

fn interrupt() -> Command {
    Command::Session(SessionCommand::Interrupt {
        session: SessionId(1),
    })
}

fn refused(reason: &str) -> Admission {
    Admission::Refused {
        reason: String::from(reason),
    }
}

#[test]
fn a_hello_with_the_hosts_token_is_admitted() {
    assert_eq!(
        admit(Some(hello(TOKEN)), &TOKEN),
        Admission::Admitted(phone())
    );
}

#[test]
fn a_hello_with_another_token_is_refused() {
    assert_eq!(
        admit(Some(hello(AuthToken::from_bytes([8; 16]))), &TOKEN),
        refused("wrong token")
    );
}

#[test]
fn a_first_stream_that_is_not_a_hello_is_refused() {
    let first = [
        None,
        Some(Open::Subscribe(subscribe())),
        Some(Open::Command {
            id: CommandId(1),
            body: interrupt(),
        }),
    ];
    for open in first {
        assert_eq!(
            admit(open, &TOKEN),
            refused("the first stream must be a hello")
        );
    }
}

#[test]
fn a_later_stream_carries_a_command_or_a_subscription() {
    assert_eq!(
        stream(Some(Open::Command {
            id: CommandId(1),
            body: interrupt(),
        })),
        Stream::Command(CommandId(1), interrupt())
    );
    assert_eq!(
        stream(Some(Open::Subscribe(subscribe()))),
        Stream::Subscribe(subscribe())
    );
    assert_eq!(stream(Some(hello(TOKEN))), Stream::Finish);
    assert_eq!(stream(None), Stream::Finish);
}

#[test]
fn a_diff_is_unrecorded_and_credentials_run_beside_sessions() {
    let diff = SessionCommand::GetDiff {
        session: SessionId(1),
        snapshot: HostSeq(2),
    };
    assert_eq!(ledger(&Command::Session(diff.clone())), None);
    assert_eq!(
        session(diff, phone()),
        SessionRoute::Order(SessionId(1), Order::Diff(HostSeq(2)))
    );
    let configured = [
        InstanceCommand::SetCredential {
            provider: Provider::new("codex"),
            credential: proto::Credential {
                kind: String::from("provider"),
                values: std::collections::BTreeMap::new(),
            },
        },
        InstanceCommand::Remove {
            instance: InstanceId::named(
                Provider::new("codex"),
                "https://gw.example",
            ),
        },
    ]
    .map(|command| ledger(&Command::Instance(command)));
    assert_eq!(configured, [Some(Kind::Receipt); 2]);
    let send = SessionCommand::SendMessage {
        session: SessionId(1),
        text: String::from("hi"),
    };
    assert_eq!(ledger(&Command::Session(send.clone())), Some(Kind::Session));
    assert_eq!(
        session(send, phone()),
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
fn issue_commands_keep_a_receipt() {
    let new = NewIssue {
        project: proto::ProjectId::new(1),
        kind: String::from("task"),
        title: String::from("Remove sessions"),
        fields: Vec::new(),
    };
    let recorded = [
        HomeCommand::CreateIssue(new),
        HomeCommand::ChangeIssue {
            issue: IssueId::new(0, 1),
            change: IssueChange::Note(String::from("done")),
        },
    ]
    .map(|command| ledger(&Command::Home(command)));
    assert_eq!(recorded, [Some(Kind::Receipt); 2]);
}

#[test]
fn home_commands_run_every_time_and_a_session_may_start_in_a_project() {
    let project = || proto::ProjectId::new(1);
    let run = [HomeCommand::Sync, HomeCommand::Clone { project: project() }]
        .map(|command| ledger(&Command::Home(command)));
    assert_eq!(run, [None; 2]);
    let start = SessionCommand::Start {
        instance: InstanceId::own(Provider::new("claude")),
        at: proto::StartAt::Project(project()),
        model: None,
        effort: Some(Effort::High),
    };
    assert_eq!(
        ledger(&Command::Session(start.clone())),
        Some(Kind::Session)
    );
    assert_eq!(
        session(start, phone()),
        SessionRoute::Start {
            instance: InstanceId::own(Provider::new("claude")),
            at: proto::StartAt::Project(project()),
            choice: (None, Some(Effort::High)),
            by: phone(),
        }
    );
}

#[test]
fn sign_in_commands_keep_no_receipt() {
    let work = || InstanceId::own(Provider::new("claude"));
    let submit = SignInCommand::SubmitCode {
        instance: work(),
        code: SignInCode(String::from("pasted#code")),
    };
    let run = [
        SignInCommand::Start { instance: work() },
        submit.clone(),
        SignInCommand::Cancel { instance: work() },
        SignInCommand::StartCredential {
            instance: work(),
            kind: String::from("setup"),
            label: String::from("Work"),
        },
    ]
    .map(|command| ledger(&Command::SignIn(command)));
    assert_eq!(run, [None; 4]);
    assert_eq!(
        format!("{submit:?}"),
        "SubmitCode { instance: InstanceId { provider: Provider(\"claude\"), account: Own }, code: SignInCode(..) }"
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
fn finishing_keeps_a_receipt_so_a_retry_doesnt_finish_twice() {
    let finish = SessionCommand::Finish {
        session: SessionId(3),
    };
    assert_eq!(
        ledger(&Command::Session(finish.clone())),
        Some(Kind::Receipt)
    );
    assert_eq!(
        session(finish, phone()),
        SessionRoute::Finish(SessionId(3), phone())
    );
}
