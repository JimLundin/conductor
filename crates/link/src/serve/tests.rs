use std::collections::BTreeMap;

use crate::AuthToken;
use crate::CommandId;
use agents::{ClientName, Effort, Provider, SignInCode};
use home::canvas::{
    Block, Canvas, CanvasEntry, CanvasFile, CanvasKey, CanvasName,
};
use home::issue::{EditId, Issue, IssueChange, NewIssue};
use home::project::Project;
use home::sync::Sync;
use home::{
    Found, Home, HomeCommand, HomeRefusal, HomeResult, IssueId, ProjectId,
    UnixMillis,
};
use home_store::{CanvasStore, Memory, StoreError};
use host::fleet::{DeviceHello, Identity, Role, Signed};
use host::log::{Epoch, HostSeq};
use host::registry::{InstanceCommand, InstanceId};
use host::session::{SessionCommand, SessionId};
use host::sign_in::SignInCommand;

use crate::log::Kind;
use crate::{Command, Hello, Open, Subscribe};

use super::{
    Admission, Stream, admit, canvas_key, device, device_hello, kept_canvas,
    ledger, operation, stream,
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

fn a_device() -> DeviceHello {
    DeviceHello {
        certificate: Signed::new(Vec::from([1]), Vec::from([2])),
        proof: Vec::from([3]),
    }
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
            id: CommandId(uuid::Uuid::from_u128(1)),
            body: interrupt(),
        }),
        Some(Open::FleetHello(a_device())),
    ];
    for open in first {
        assert_eq!(
            admit(open, &TOKEN),
            refused("the first stream must be a hello")
        );
    }
}

#[test]
fn a_fleet_host_asks_the_fleet_about_a_device_and_refuses_anything_else() {
    assert_eq!(
        device_hello(Some(Open::FleetHello(a_device()))),
        Ok(a_device())
    );
    let required = refused("a current fleet device certificate is required");
    for open in [None, Some(hello(TOKEN)), Some(Open::Subscribe(subscribe()))] {
        assert_eq!(device_hello(open), Err(required.clone()));
    }
    let owner = Identity {
        fleet: [1; 32],
        device: [2; 32],
        name: String::from("Jim's phone"),
        role: Role::Owner,
    };
    assert_eq!(
        device(Some(owner)),
        Admission::Admitted(ClientName(String::from("Jim's phone")))
    );
    assert_eq!(device(None), required);
}

#[test]
fn a_later_stream_carries_a_command_or_a_subscription() {
    assert_eq!(
        stream(Some(Open::Command {
            id: CommandId(uuid::Uuid::from_u128(1)),
            body: interrupt(),
        })),
        Stream::Command(CommandId(uuid::Uuid::from_u128(1)), interrupt())
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
    assert_eq!(ledger(&Command::Session(diff)), None);
    let configured = [
        InstanceCommand::SetCredential {
            provider: Provider::new("codex"),
            credential: agents::Credential {
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
    assert_eq!(ledger(&Command::Session(send)), Some(Kind::Session));
}

#[test]
fn issue_commands_keep_a_receipt() {
    let new = NewIssue {
        project: home::ProjectId::new(1),
        kind: String::from("task"),
        title: String::from("Remove sessions"),
        fields: Vec::new(),
    };
    let recorded = [
        HomeCommand::CreateIssue {
            edit: EditId(1),
            issue: new,
        },
        HomeCommand::ChangeIssue {
            edit: EditId(2),
            issue: IssueId::new(0, 1),
            change: IssueChange::Note(String::from("done")),
        },
    ]
    .map(|command| ledger(&Command::Home(command)));
    assert_eq!(recorded, [Some(Kind::Receipt); 2]);
}

#[test]
fn home_commands_run_every_time_and_a_session_start_runs_alone() {
    let project = || home::ProjectId::new(1);
    let run = [HomeCommand::Sync, HomeCommand::Clone { project: project() }]
        .map(|command| ledger(&Command::Home(command)));
    assert_eq!(run, [None; 2]);
    let start = SessionCommand::Start {
        instance: InstanceId::own(Provider::new("claude")),
        at: host::session::StartAt::Project(project()),
        model: None,
        effort: Some(Effort::High),
    };
    assert_eq!(ledger(&Command::Session(start)), Some(Kind::Session));
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
fn finishing_keeps_a_receipt_so_a_retry_doesnt_finish_twice() {
    let finish = SessionCommand::Finish {
        session: SessionId(3),
    };
    assert_eq!(ledger(&Command::Session(finish)), Some(Kind::Receipt));
}

#[test]
fn a_command_is_named_in_log_lines_by_its_operation() {
    assert_eq!(
        [
            operation(&interrupt()),
            operation(&Command::Home(HomeCommand::Sync)),
            operation(&Command::SignIn(SignInCommand::Cancel {
                instance: InstanceId::own(Provider::new("claude")),
            })),
        ],
        ["interrupt", "sync_home", "cancel_sign_in"]
    );
}

const PROJECT: ProjectId = ProjectId::new(1);
const DESIGNED: IssueId = IssueId::new(1, 0);
const BARE: IssueId = IssueId::new(2, 0);

fn issue(id: IssueId) -> Issue {
    Issue {
        id,
        project: PROJECT,
        title: String::from("Serve canvases"),
        kind: String::from("task"),
        status: String::from("open"),
        stage: None,
        fields: BTreeMap::new(),
        links: BTreeMap::new(),
        body: String::new(),
        sections: Vec::new(),
        checklist: Vec::new(),
        log: Vec::new(),
        branch: format!("issue/{id}"),
        canvases: Vec::new(),
    }
}

fn a_home() -> Home {
    listed_home(&BTreeMap::new())
}

fn listed_home(listed: &BTreeMap<IssueId, Vec<CanvasEntry>>) -> Home {
    home::view(
        "origin",
        &Sync::new(),
        &[Project {
            id: PROJECT,
            name: String::from("conductor"),
            title: String::from("Conductor"),
            remote: None,
            root: String::new(),
            profiles: Vec::new(),
            provides: Vec::new(),
        }],
        Found {
            checkouts: &BTreeMap::new(),
            cloning: &BTreeMap::new(),
            flow: None,
            issues: &BTreeMap::from([(
                PROJECT,
                Vec::from([issue(DESIGNED), issue(BARE)]),
            )]),
            canvases: listed,
            unpushed: 0,
        },
    )
}

fn named(text: &str) -> Result<CanvasName, String> {
    CanvasName::parse(text).map_err(|error| error.to_string())
}

fn key(issue: IssueId, name: &str) -> Result<CanvasKey, String> {
    Ok(CanvasKey {
        project: PROJECT,
        issue,
        name: named(name)?,
    })
}

fn written(at: u64, by: &str) -> Result<CanvasFile, String> {
    Ok(CanvasFile {
        written: UnixMillis(at),
        by: String::from(by),
        canvas: Canvas::new(
            format!("By {by}"),
            Vec::from([Block::Prose {
                text: String::from("The host serves it."),
            }]),
        )
        .map_err(|error| error.to_string())?,
    })
}

fn two_canvases() -> Result<Memory, String> {
    let mut store = Memory::default();
    store
        .put(&key(DESIGNED, "design")?, &written(10, "devbox·claude")?)
        .map_err(|error| error.to_string())?;
    store
        .put(&key(DESIGNED, "a-plan")?, &written(20, "laptop·codex")?)
        .map_err(|error| error.to_string())?;
    Ok(store)
}

#[test]
fn an_issue_with_two_canvases_is_served_with_both_entries() -> Result<(), String>
{
    served_entries(&two_canvases()?)
}

fn served_entries(store: &Memory) -> Result<(), String> {
    let listed = [DESIGNED, BARE]
        .into_iter()
        .map(|id| {
            Ok((
                id,
                store
                    .list(&PROJECT, &id)
                    .map_err(|e| e.to_string())?
                    .canvases
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|broken| broken.error.to_string())?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let served = listed_home(&listed);
    let canvases = served.projects[0]
        .issues
        .iter()
        .map(|issue| issue.canvases.clone())
        .collect::<Vec<_>>();
    let entry =
        |name: &str, at: u64, by: &str| -> Result<CanvasEntry, String> {
            Ok(CanvasEntry {
                name: named(name)?,
                written: UnixMillis(at),
                by: String::from(by),
            })
        };
    assert_eq!(
        canvases,
        [
            Vec::from([
                entry("a-plan", 20, "laptop·codex")?,
                entry("design", 10, "devbox·claude")?,
            ]),
            Vec::new(),
        ]
    );
    Ok(())
}

#[test]
fn a_canvas_of_a_known_issue_is_keyed_by_the_issues_project()
-> Result<(), String> {
    assert_eq!(
        canvas_key(Some(&a_home()), DESIGNED, named("design")?),
        Ok(key(DESIGNED, "design")?)
    );
    Ok(())
}

#[test]
fn a_canvas_of_an_issue_the_home_lacks_or_of_no_home_is_refused()
-> Result<(), String> {
    assert_eq!(
        [
            canvas_key(Some(&a_home()), IssueId::new(3, 0), named("design")?),
            canvas_key(None, DESIGNED, named("design")?),
        ],
        [Err(HomeRefusal::UnknownIssue), Err(HomeRefusal::NoHome),]
    );
    Ok(())
}

#[test]
fn a_kept_canvas_is_answered_and_a_missing_one_refused() -> Result<(), String> {
    assert_eq!(
        kept_canvas(Ok::<_, StoreError>(Some(written(10, "devbox·claude")?))),
        HomeResult::Canvas(written(10, "devbox·claude")?)
    );
    assert_eq!(
        kept_canvas(Ok::<_, StoreError>(None)),
        HomeResult::Refused(HomeRefusal::UnknownCanvas)
    );
    Ok(())
}

#[test]
fn a_failing_store_leaves_the_canvas_unavailable() {
    let refusal = HomeRefusal::CanvasUnavailable {
        reason: String::from("the disk is gone"),
    };
    assert_eq!(
        kept_canvas::<StoreError>(Err(StoreError::Unavailable {
            reason: String::from("the disk is gone"),
        })),
        HomeResult::Refused(refusal.clone())
    );
    assert_eq!(
        refusal.label(),
        "the canvas can't be read now: the disk is gone"
    );
}

#[test]
fn a_canvas_is_read_every_time_and_named_get_canvas() -> Result<(), String> {
    let command = Command::Home(HomeCommand::Canvas {
        issue: DESIGNED,
        name: named("design")?,
    });
    assert_eq!(
        (ledger(&command), operation(&command)),
        (None, "get_canvas")
    );
    Ok(())
}
