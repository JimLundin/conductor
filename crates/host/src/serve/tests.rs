use proto::{
    AuthToken, CheckpointId, ClientName, Command, CommandBody, CommandId,
    CommandResult, Effort, Epoch, Hello, HostSeq, InstanceId, Models, Open,
    Provider, Rejection, SessionId, SignInCode, Subscribe,
};

use super::{
    Admission, Configure, HomeOrder, Ledger, Order, Relaunch, Route,
    SignInOrder, Stream, admit, route, stream,
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

fn command(body: CommandBody) -> Command {
    Command {
        id: CommandId(1),
        body,
    }
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
        Some(Open::Command(command(CommandBody::Interrupt {
            session: SessionId(1),
        }))),
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
    let interrupt = command(CommandBody::Interrupt {
        session: SessionId(1),
    });
    assert_eq!(
        stream(Some(Open::Command(interrupt.clone()))),
        Stream::Command(interrupt)
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
    let diff = route(
        CommandBody::GetDiff {
            session: SessionId(1),
            checkpoint: CheckpointId(2),
        },
        phone(),
    );
    assert_eq!(
        diff,
        Route::Order(SessionId(1), Order::Diff(CheckpointId(2)))
    );
    assert_eq!(diff.ledger(), Ledger::Unrecorded);
    let work = || InstanceId::named(Provider::Codex, "https://gw.example");
    let gateway = || proto::Credential {
        kind: String::from("provider"),
        values: std::collections::BTreeMap::new(),
    };
    let configured = [
        CommandBody::SetCredential {
            provider: Provider::Codex,
            credential: gateway(),
        },
        CommandBody::RemoveInstance { instance: work() },
    ]
    .map(|body| route(body, phone()));
    assert_eq!(
        configured.each_ref().map(Route::ledger),
        [Ledger::Credential; 2]
    );
    assert_eq!(
        configured,
        [
            Route::Configure(Configure::SetCredential {
                provider: Provider::Codex,
                credential: gateway(),
            }),
            Route::Configure(Configure::Remove(work())),
        ]
    );
    let send = route(
        CommandBody::SendMessage {
            session: SessionId(1),
            text: String::from("hi"),
        },
        phone(),
    );
    assert_eq!(
        send,
        Route::Order(
            SessionId(1),
            Order::Send {
                text: String::from("hi"),
                by: phone(),
            }
        )
    );
    assert_eq!(send.ledger(), Ledger::Session);
}

#[test]
fn home_commands_run_every_time_and_a_session_may_start_in_a_project() {
    let project = || proto::ProjectId(String::from("conductor"));
    let routed = [
        CommandBody::SyncHome,
        CommandBody::CloneProject { project: project() },
    ]
    .map(|body| route(body, phone()));
    assert_eq!(
        routed.each_ref().map(Route::ledger),
        [Ledger::Unrecorded; 2]
    );
    assert_eq!(
        routed,
        [
            Route::Home(HomeOrder::Sync),
            Route::Home(HomeOrder::Clone(project())),
        ]
    );
    let started = route(
        CommandBody::StartSession {
            instance: InstanceId::own(Provider::Claude),
            at: proto::StartAt::Project(project()),
            model: None,
            effort: Some(Effort::High),
        },
        phone(),
    );
    assert_eq!(started.ledger(), Ledger::Session);
    assert_eq!(
        started,
        Route::Start {
            instance: InstanceId::own(Provider::Claude),
            at: proto::StartAt::Project(project()),
            choice: (None, Some(Effort::High)),
            by: phone(),
        }
    );
}

#[test]
fn sign_in_commands_keep_no_receipt() {
    let work = || InstanceId::own(Provider::Claude);
    let code = || SignInCode(String::from("pasted#code"));
    let routed = [
        CommandBody::StartSignIn { instance: work() },
        CommandBody::SubmitSignInCode {
            instance: work(),
            code: code(),
        },
        CommandBody::CancelSignIn { instance: work() },
    ]
    .map(|body| route(body, phone()));
    assert_eq!(
        routed.each_ref().map(Route::ledger),
        [Ledger::Unrecorded; 3]
    );
    assert_eq!(
        routed,
        [
            Route::SignIn(SignInOrder::Start(work())),
            Route::SignIn(SignInOrder::Submit(work(), code())),
            Route::SignIn(SignInOrder::Cancel(work())),
        ]
    );
    assert_eq!(
        format!("{:?}", routed[1]),
        "SignIn(Submit(InstanceId { provider: Claude, account: Own }, SignInCode(..)))"
    );
}

#[test]
fn relaunching_orders_take_the_offered_models() {
    assert_eq!(
        route(
            CommandBody::SetEffort {
                session: SessionId(1),
                effort: Effort::High,
            },
            phone(),
        ),
        Route::Relaunch(SessionId(1), Relaunch::Effort(Effort::High))
    );
    assert_eq!(
        Relaunch::Continue.order(Models::none()),
        Order::Start(Models::none())
    );
    assert_eq!(
        Relaunch::Undo(CheckpointId(3)).order(Models::none()),
        Order::Undo(CheckpointId(3))
    );
}

#[test]
fn an_undo_whose_session_stopped_fails_closed() {
    assert_eq!(Relaunch::Undo(CheckpointId(3)).closed(), None);
    assert_eq!(
        Relaunch::Continue.closed(),
        Some(CommandResult::Rejected(Rejection::SessionClosed))
    );
}

#[test]
fn an_undo_order_forks_its_conversation_with_the_given_id() {
    assert_eq!(
        Order::Undo(CheckpointId(3)).session(String::from("fork")),
        crate::session::Order::Undo {
            checkpoint: CheckpointId(3),
            fork: String::from("fork"),
        }
    );
}
