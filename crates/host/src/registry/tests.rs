use agents::Configuration;

use std::collections::{BTreeMap, BTreeSet};
use std::ops::ControlFlow::{Break, Continue};

use crate::session::SessionDirectory;

use crate::log::{Event, HostSeq};
use crate::registry::{
    Availability, Catalog, Instance, InstanceId, InstanceRefusal,
    InstanceResult, InstanceStatus, Instances, Roster,
};
use crate::session::{
    EventKind, SessionId, SessionRefusal, SessionResult, StartAt,
};
use agents::{
    Account, Auth, ClientName, CloseReason, Credential, CredentialForm, Effort,
    Field, FieldKind, ModelChoice, ModelId, Models, Provider, ProviderInfo,
};
use home::{CheckoutLocation, Home, HomeRefusal, HomeSync, ProjectId};
use vcs::{Repository, WorkingCopy, Worktree};

use super::{
    AccountAdoption, Change, Effect, InHome, Job, Launch, Launcher, OwnSignIn,
    Refreshed, Registry, StartDecision, StartEffect, StartEnd, StartReport,
    StartSession, StartStep, Started, Starting, Stored, defaults, dir, offered,
    picked, place_in_home, place_without_home, running,
};
use crate::Note;
use crate::session::Choice;
use vcs::Error as GitError;

type Test = Registry<&'static str, u8, &'static str>;
type Seen = Vec<Effect<&'static str, &'static str>>;

const GATEWAY: &str = "https://gateway.example";

fn text(value: &str) -> String {
    String::from(value)
}

fn own(provider: Provider) -> InstanceId {
    InstanceId::own(provider)
}

fn api() -> InstanceId {
    InstanceId::named(Provider::new("claude"), GATEWAY)
}

fn model(id: &str) -> ModelId {
    ModelId(text(id))
}

fn listed() -> Models {
    Models {
        choices: Vec::from([ModelChoice {
            id: model("big"),
            name: text("Big"),
            description: String::new(),
            efforts: BTreeSet::from([Effort::Low, Effort::High]),
        }]),
        default: Some(model("big")),
    }
}

fn ready(start: &'static str, own: OwnSignIn) -> Launcher<&'static str> {
    Launcher::Running {
        start,
        own,
        chosen: None,
        models: Ok(listed()),
    }
}

fn booted(
    mut launchers: BTreeMap<InstanceId, Launcher<&'static str>>,
    stored: &Stored,
) -> Test {
    let mut registry = Registry::new(stored, catalog());
    let boot = registry.boot();
    assert_eq!(boot.len(), launchers.len());
    for Starting { id, instance, .. } in boot {
        if let Some(launcher) = launchers.remove(&instance) {
            assert_eq!(
                registry.started(Started {
                    id,
                    instance,
                    launcher
                }),
                []
            );
        }
    }
    assert_eq!(launchers, BTreeMap::new());
    registry
}

fn registry(own_sign_in: OwnSignIn, stored: &Stored) -> Test {
    booted(
        BTreeMap::from([
            (own(Provider::new("claude")), ready("claude", own_sign_in)),
            (
                own(Provider::new("codex")),
                Launcher::Unavailable(text("no codex")),
            ),
        ]),
        stored,
    )
}

fn host_of(instances: &[(&'static str, InstanceId)]) -> Test {
    let mut launchers = BTreeMap::from([
        (
            own(Provider::new("claude")),
            ready("claude", OwnSignIn::Unmanaged),
        ),
        (
            own(Provider::new("codex")),
            ready("codex", OwnSignIn::Unmanaged),
        ),
    ]);
    launchers.extend(
        instances
            .iter()
            .map(|(start, id)| (id.clone(), ready(start, OwnSignIn::Missing))),
    );
    let roster = Roster(
        instances
            .iter()
            .map(|(_, id)| (id.clone(), Instance::default()))
            .collect(),
    );
    booted(launchers, &Stored::Opened(roster))
}

fn catalog() -> Catalog {
    let field = |id: &str, kind| Field {
        id: text(id),
        label: text(id),
        kind,
        placeholder: String::new(),
        required: true,
    };
    let info = |label: &str| ProviderInfo {
        label: text(label),
        note: None,
        credentials: Vec::from([CredentialForm {
            kind: text("gateway"),
            label: text("Gateway"),
            fields: Vec::from([
                field("base_url", FieldKind::Url),
                field("token", FieldKind::Secret),
            ]),
            account: text("base_url"),
            setup_sign_in: None,
        }]),
    };
    Catalog(BTreeMap::from([
        (Provider::new("claude"), info("Claude Code")),
        (Provider::new("codex"), info("Codex")),
    ]))
}

fn gateway_at(base_url: &str, token: &str) -> Credential {
    Credential {
        kind: text("gateway"),
        values: [("base_url", base_url), ("token", token)]
            .map(|(id, value)| (text(id), text(value)))
            .into(),
    }
}

fn gateway(token: &str) -> Credential {
    gateway_at(GATEWAY, token)
}

fn opened() -> Stored {
    Stored::Opened(Roster::default())
}

fn unavailable(reason: &str) -> Availability {
    Availability::Unavailable {
        reason: text(reason),
    }
}

fn stored_auth() -> Auth {
    Auth::Stored {
        kind: text("gateway"),
        url: Some(text(GATEWAY)),
    }
}

fn claude(availability: Availability) -> Instances {
    Instances(BTreeMap::from([
        (
            own(Provider::new("claude")),
            InstanceStatus { availability },
        ),
        (
            own(Provider::new("codex")),
            InstanceStatus {
                availability: unavailable("no codex"),
            },
        ),
    ]))
}

fn availability(registry: &Test, id: &InstanceId) -> Option<Availability> {
    registry.instances().availability(id).cloned()
}

fn refuse(refusal: InstanceRefusal) -> InstanceResult {
    InstanceResult::Refused(refusal)
}

fn stored_note(id: &InstanceId) -> Effect<&'static str, &'static str> {
    Effect::Log(Note::CredentialStored {
        instance: id.clone(),
    })
}

fn job(
    number: u64,
    id: &InstanceId,
    start: &'static str,
    credential: Option<Credential>,
) -> Job<&'static str> {
    Job {
        id: number,
        instance_id: id.clone(),
        adapter: start,
        credential,
    }
}

fn refreshed(
    number: u64,
    id: &InstanceId,
    configured: Result<Configuration, String>,
) -> Refreshed {
    Refreshed {
        id: number,
        instance: id.clone(),
        configured,
    }
}

fn listing() -> Configuration {
    Configuration {
        models: Ok(listed()),
        named: None,
    }
}

fn saving(registry: &mut Test, effects: Seen) -> Seen {
    match effects.into_iter().next() {
        Some(Effect::Save {
            roster,
            change,
            reply,
        }) => registry.saved(roster, change, reply, Ok(())),
        other => other.into_iter().collect(),
    }
}

fn set(
    registry: &mut Test,
    provider: Provider,
    credential: Credential,
    reply: &'static str,
) -> Seen {
    let effects = registry.set_credential(provider, credential, reply);
    saving(registry, effects)
}

fn with_credential(roster: Roster, id: &InstanceId, token: &str) -> Roster {
    let mut changed = roster;
    changed.0.insert(
        id.clone(),
        Instance {
            credential: Some(gateway(token)),
            ..Instance::default()
        },
    );
    changed
}

#[test]
fn a_start_is_refused_until_directory_instance_and_model_all_hold() {
    let registry = registry(OwnSignIn::Unmanaged, &opened());
    let claude = own(Provider::new("claude"));
    assert_eq!(
        [
            registry.launch_session(&claude, false, None, None),
            registry.launch_session(&api(), true, None, None),
            registry.launch_session(
                &own(Provider::new("codex")),
                true,
                None,
                None
            ),
            registry.launch_session(&claude, true, Some(model("tiny")), None),
            registry.launch_session(&claude, true, None, Some(Effort::Max)),
        ],
        [
            Err(SessionRefusal::NoSuchDirectory),
            Err(SessionRefusal::UnknownInstance),
            Err(SessionRefusal::InstanceUnavailable {
                reason: text("no codex")
            }),
            Err(SessionRefusal::NotOffered {
                reason: text("this host doesn't offer the model tiny")
            }),
            Err(SessionRefusal::NotOffered {
                reason: text("Big doesn't take the effort max")
            }),
        ]
    );
    assert_eq!(
        registry.launch_session(&claude, true, None, Some(Effort::High)),
        Ok(Launch {
            start: "claude",
            choice: Choice {
                model: Some(model("big")),
                effort: Some(Effort::High),
            },
            offered: listed(),
        })
    );
}

#[test]
fn every_host_has_each_provider_s_own_instance_beside_its_accounts() {
    let defaults = defaults(&catalog());
    let accounts = Roster(BTreeMap::from([(api(), Instance::default())]));
    assert_eq!(
        defaults.0.keys().cloned().collect::<Vec<InstanceId>>(),
        [own(Provider::new("claude")), own(Provider::new("codex"))]
    );
    assert_eq!(
        [
            Stored::Fresh.roster(&catalog()),
            Stored::Unreadable(text("sealed")).roster(&catalog()),
            Stored::Opened(accounts.clone()).roster(&catalog()),
        ],
        [
            defaults.clone(),
            defaults.clone(),
            Roster(defaults.0.into_iter().chain(accounts.0).collect()),
        ]
    );
    assert_eq!(
        registry(OwnSignIn::Unmanaged, &Stored::Fresh).instances(),
        claude(Availability::Available {
            auth: Auth::Unmanaged,
            models: listed(),
        })
    );
}

#[test]
fn an_instance_s_dirs_are_its_provider_and_a_hash_of_its_account() {
    assert_eq!(
        [
            dir(&own(Provider::new("claude"))),
            dir(&InstanceId::named(
                Provider::new("claude"),
                "me@example.com"
            )),
            dir(&InstanceId::named(Provider::new("codex"), "me@example.com")),
            dir(&api()),
        ],
        [
            text("claude"),
            text("claude-8c2a47d3bdb8d309"),
            text("codex-8c2a47d3bdb8d309"),
            text("claude-7ba7f84d34407e97"),
        ]
    );
}

#[test]
fn the_command_line_picks_an_account_or_the_provider_s_own_instance() {
    let roster =
        Stored::Opened(Roster(BTreeMap::from([(api(), Instance::default())])))
            .roster(&catalog());
    assert_eq!(
        [
            picked(&roster, &Provider::new("claude"), None),
            picked(&roster, &Provider::new("claude"), Some(GATEWAY)),
            picked(&roster, &Provider::new("claude"), Some("me@example.com")),
            picked(&roster, &Provider::new("codex"), Some("me@example.com")),
        ],
        [
            Ok(own(Provider::new("claude"))),
            Ok(api()),
            Err(text(
                "this host has no instance claude me@example.com; its claude accounts are https://gateway.example"
            )),
            Err(text("this host has no instance codex me@example.com")),
        ]
    );
}

#[test]
fn an_instance_without_a_sign_in_or_a_readable_store_is_unavailable() {
    let missing = registry(OwnSignIn::Missing, &opened());
    let unreadable =
        registry(OwnSignIn::SignedIn, &Stored::Unreadable(text("sealed")));
    let signed_in = registry(OwnSignIn::SignedIn, &opened());
    assert_eq!(
        [
            missing.instances(),
            unreadable.instances(),
            signed_in.instances()
        ],
        [
            claude(unavailable("no credential configured")),
            claude(unavailable("sealed")),
            claude(Availability::Available {
                auth: Auth::SignedIn,
                models: listed(),
            }),
        ]
    );
    assert_eq!(
        missing.launch_session(&own(Provider::new("claude")), true, None, None),
        Err(SessionRefusal::InstanceUnavailable {
            reason: text("no credential configured")
        })
    );
    assert_eq!(
        Registry::<&str, u8, &str>::new(&opened(), catalog()).launch_session(
            &own(Provider::new("claude")),
            true,
            None,
            None
        ),
        Err(SessionRefusal::InstanceUnavailable {
            reason: text("this host has no instance claude")
        })
    );
}

#[test]
fn a_session_is_found_and_continued_only_while_its_instance_runs() {
    let mut registry = host_of(&[("api", api())]);
    registry.admit(SessionId(1), own(Provider::new("claude")), 7);
    registry.admit(SessionId(2), api(), 8);
    assert_eq!(
        (
            registry.find(SessionId(1)),
            registry.find(SessionId(3)),
            registry.relaunch(SessionId(1)),
            registry.relaunch(SessionId(2)),
            registry.relaunch(SessionId(3)),
        ),
        (
            Some(7),
            None,
            Ok((7, "claude", listed())),
            Err(SessionRefusal::InstanceUnavailable {
                reason: text("no credential configured")
            }),
            Err(SessionRefusal::UnknownSession),
        )
    );
    let _ = set(&mut registry, Provider::new("claude"), gateway("t"), "set");
    assert_eq!(
        registry.relaunch(SessionId(2)),
        Err(SessionRefusal::InstanceUnavailable {
            reason: text("refreshing the instance's models")
        })
    );
}

#[test]
fn two_instances_of_one_provider_keep_their_own_credentials_and_agents() {
    let mut registry = host_of(&[("api", api())]);
    let boot_jobs = registry.jobs;
    assert_eq!(
        set(
            &mut registry,
            Provider::new("claude"),
            gateway("api"),
            "set"
        ),
        Vec::from([
            stored_note(&api()),
            Effect::Refresh(job(
                boot_jobs + 1,
                &api(),
                "api",
                Some(gateway("api"))
            )),
        ])
    );
    assert_eq!(
        (
            availability(&registry, &own(Provider::new("claude"))),
            availability(&registry, &api()),
        ),
        (
            Some(Availability::Available {
                auth: Auth::Unmanaged,
                models: listed(),
            }),
            Some(unavailable("refreshing the instance's models")),
        )
    );
    let _ = registry.refreshed(refreshed(boot_jobs + 1, &api(), Ok(listing())));
    assert_eq!(
        availability(&registry, &api()),
        Some(Availability::Available {
            auth: stored_auth(),
            models: listed(),
        })
    );
    let started = |registry: &Test, id: &InstanceId| {
        registry
            .launch_session(id, true, None, None)
            .map(|launch| launch.start)
    };
    assert_eq!(
        [
            started(&registry, &own(Provider::new("claude"))),
            started(&registry, &api())
        ],
        [Ok("claude"), Ok("api")]
    );
}

#[test]
fn each_codex_instance_refreshes_its_own_app_server() {
    let codex_api = InstanceId::named(Provider::new("codex"), GATEWAY);
    let mut registry = host_of(&[("codex-api", codex_api.clone())]);
    let boot_jobs = registry.jobs;
    assert_eq!(
        set(&mut registry, Provider::new("codex"), gateway("key"), "set"),
        Vec::from([
            stored_note(&codex_api),
            Effect::Refresh(job(
                boot_jobs + 1,
                &codex_api,
                "codex-api",
                Some(gateway("key"))
            )),
        ])
    );
    registry.admit(SessionId(1), own(Provider::new("codex")), 1);
    assert_eq!(
        (
            registry
                .launch_session(&own(Provider::new("codex")), true, None, None)
                .map(|launch| launch.start),
            registry.relaunch(SessionId(1)),
            registry
                .launch_session(&codex_api, true, None, None)
                .map(|launch| launch.start),
        ),
        (
            Ok("codex"),
            Ok((1, "codex", listed())),
            Err(SessionRefusal::InstanceUnavailable {
                reason: text("refreshing the instance's models")
            }),
        )
    );
}

#[test]
fn a_credential_of_a_new_account_adds_its_instance_and_starts_its_cli() {
    let mut registry = registry(OwnSignIn::Unmanaged, &opened());
    let boot_jobs = registry.jobs;
    let added = Instance {
        credential: Some(gateway("t")),
        ..Instance::default()
    };
    assert_eq!(
        registry.set_credential(Provider::new("claude"), gateway("t"), "add"),
        Vec::from([Effect::Save {
            roster: with_credential(defaults(&catalog()), &api(), "t"),
            change: Change::Added(api()),
            reply: "add",
        }])
    );
    assert_eq!(
        set(&mut registry, Provider::new("claude"), gateway("t"), "add"),
        Vec::from([
            Effect::Log(Note::InstanceAdded { instance: api() }),
            Effect::Start(Starting {
                mode: crate::start::Mode::Change,
                id: boot_jobs + 1,
                instance: api(),
                settings: added,
            }),
        ])
    );
    assert_eq!(
        (
            availability(&registry, &api()),
            set(&mut registry, Provider::new("claude"), gateway("t2"), "set"),
        ),
        (
            Some(unavailable("starting the instance's CLI")),
            Vec::from([stored_note(&api())]),
        )
    );
    assert_eq!(
        registry.started(Started {
            id: boot_jobs + 1,
            instance: api(),
            launcher: ready("api", OwnSignIn::Missing),
        }),
        Vec::from([Effect::Refresh(job(
            boot_jobs + 2,
            &api(),
            "api",
            Some(gateway("t2"))
        ))])
    );
    assert_eq!(
        registry.refreshed(refreshed(boot_jobs + 2, &api(), Ok(listing()))),
        Vec::from([
            Effect::Reply("add", InstanceResult::Accepted),
            Effect::Reply("set", InstanceResult::Accepted),
        ])
    );
    assert_eq!(
        registry.set_credential(
            Provider::new("codex"),
            gateway_at("", "t"),
            "none"
        ),
        Vec::from([Effect::Reply(
            "none",
            refuse(InstanceRefusal::InvalidCredential {
                reason: text("base_url is required")
            })
        )])
    );
}

#[test]
fn a_start_for_an_instance_since_removed_is_dropped() {
    let mut registry = registry(OwnSignIn::Unmanaged, &opened());
    let boot_jobs = registry.jobs;
    let codex_api = InstanceId::named(Provider::new("codex"), GATEWAY);
    let _ = set(&mut registry, Provider::new("codex"), gateway("k"), "add");
    let removed =
        registry.remove_instance(codex_api.clone(), &BTreeSet::new(), "remove");
    let result = InstanceResult::Accepted;
    assert_eq!(
        saving(&mut registry, removed),
        Vec::from([
            Effect::Log(Note::InstanceRemoved {
                instance: codex_api.clone()
            }),
            Effect::Reply("add", result.clone()),
            Effect::Reply("remove", result),
        ])
    );
    assert_eq!(
        registry.started(Started {
            id: boot_jobs + 1,
            instance: codex_api.clone(),
            launcher: ready("codex-api", OwnSignIn::Unmanaged),
        }),
        Vec::new()
    );
    assert_eq!(registry.instances().of(&codex_api), None);
}

#[test]
fn a_start_from_before_an_instance_was_added_again_is_dropped() {
    let mut registry = registry(OwnSignIn::Unmanaged, &opened());
    let boot_jobs = registry.jobs;
    let codex_api = InstanceId::named(Provider::new("codex"), GATEWAY);
    let _ = set(&mut registry, Provider::new("codex"), gateway("k"), "add");
    let removed =
        registry.remove_instance(codex_api.clone(), &BTreeSet::new(), "remove");
    let _ = saving(&mut registry, removed);
    let again =
        set(&mut registry, Provider::new("codex"), gateway("k"), "again");
    let stale = registry.started(Started {
        id: boot_jobs + 1,
        instance: codex_api.clone(),
        launcher: ready("old", OwnSignIn::Unmanaged),
    });
    let starting = availability(&registry, &codex_api);
    let current = registry.started(Started {
        id: boot_jobs + 2,
        instance: codex_api.clone(),
        launcher: ready("codex-api", OwnSignIn::Unmanaged),
    });
    assert_eq!(
        (again, stale, starting, current),
        (
            Vec::from([
                Effect::Log(Note::InstanceAdded {
                    instance: codex_api.clone()
                }),
                Effect::Start(Starting {
                    mode: crate::start::Mode::Change,
                    id: boot_jobs + 2,
                    instance: codex_api,
                    settings: Instance {
                        credential: Some(gateway("k")),
                        ..Instance::default()
                    },
                }),
            ]),
            Vec::new(),
            Some(unavailable("starting the instance's CLI")),
            Vec::from([Effect::Reply("again", InstanceResult::Accepted)]),
        )
    );
}

#[test]
fn an_instance_is_removed_only_while_no_session_runs_on_it() {
    let mut registry = host_of(&[("api", api())]);
    registry.admit(SessionId(1), api(), 1);
    registry.admit(SessionId(2), api(), 2);
    registry.admit(SessionId(3), own(Provider::new("codex")), 3);
    assert_eq!(
        registry.remove_instance(
            api(),
            &BTreeSet::from([SessionId(1), SessionId(3)]),
            "busy"
        ),
        Vec::from([Effect::Reply(
            "busy",
            refuse(InstanceRefusal::InstanceInUse { sessions: 1 })
        )])
    );
    let removed = registry.remove_instance(
        api(),
        &BTreeSet::from([SessionId(3)]),
        "remove",
    );
    let _ = saving(&mut registry, removed);
    assert_eq!(
        (
            registry.instances().of(&api()),
            registry.relaunch(SessionId(1)),
            registry.find(SessionId(2)),
        ),
        (
            None,
            Err(SessionRefusal::InstanceUnavailable {
                reason: text(
                    "this host has no instance claude https://gateway.example"
                )
            }),
            Some(2),
        )
    );
    let kept = registry.remove_instance(
        own(Provider::new("claude")),
        &BTreeSet::new(),
        "own",
    );
    assert_eq!(
        kept,
        Vec::from([Effect::Reply("own", InstanceResult::Accepted)])
    );
    assert!(
        registry
            .instances()
            .of(&own(Provider::new("claude")))
            .is_some()
    );
}

#[test]
fn continue_reopens_a_closed_session_for_instance_removal_checks() {
    let event = |session, kind| Event {
        seq: HostSeq(1),
        session: SessionId(session),
        kind,
    };
    let closed = || EventKind::SessionClosed {
        reason: CloseReason::Stopped {
            reason: text("the host restarted"),
        },
    };
    let history = [
        event(
            1,
            EventKind::SessionCreated {
                instance: own(Provider::new("claude")),
                directory: SessionDirectory(text("/work")),
                by: ClientName(text("phone")),
                model: None,
                effort: None,
            },
        ),
        event(2, EventKind::SessionReady),
        event(3, EventKind::SessionReady),
        event(2, closed()),
        event(3, closed()),
        event(3, EventKind::SessionReady),
    ];
    assert_eq!(
        running(&history),
        BTreeSet::from([SessionId(1), SessionId(3)])
    );
}

#[test]
fn credential_changes_coalesce_and_a_stale_refresh_is_dropped() {
    let mut registry = host_of(&[("api", api())]);
    let boot_jobs = registry.jobs;
    assert_eq!(
        set(
            &mut registry,
            Provider::new("claude"),
            gateway("first"),
            "first"
        ),
        Vec::from([
            stored_note(&api()),
            Effect::Refresh(job(
                boot_jobs + 1,
                &api(),
                "api",
                Some(gateway("first"))
            ))
        ])
    );
    assert_eq!(
        (
            set(
                &mut registry,
                Provider::new("claude"),
                gateway("middle"),
                "middle"
            ),
            set(
                &mut registry,
                Provider::new("claude"),
                gateway("newest"),
                "newest"
            ),
            availability(&registry, &api()),
        ),
        (
            Vec::from([stored_note(&api())]),
            Vec::from([stored_note(&api())]),
            Some(unavailable("refreshing the instance's models")),
        )
    );
    let offered = Configuration {
        models: Ok(Models::none()),
        named: Some(model("gw")),
    };
    assert_eq!(
        registry.refreshed(refreshed(
            boot_jobs + 1,
            &api(),
            Ok(offered.clone())
        )),
        Vec::from([Effect::Refresh(job(
            boot_jobs + 3,
            &api(),
            "api",
            Some(gateway("newest"))
        ))])
    );
    let current = Availability::Available {
        auth: stored_auth(),
        models: Models {
            choices: Vec::new(),
            default: Some(model("gw")),
        },
    };
    let answered = registry.refreshed(refreshed(
        boot_jobs + 3,
        &api(),
        Ok(offered.clone()),
    ));
    let result = InstanceResult::Accepted;
    assert_eq!(
        answered,
        Vec::from([
            Effect::Reply("first", result.clone()),
            Effect::Reply("middle", result.clone()),
            Effect::Reply("newest", result),
        ])
    );
    assert_eq!(
        registry.refreshed(refreshed(boot_jobs + 2, &api(), Ok(offered))),
        Vec::new()
    );
    assert_eq!(availability(&registry, &api()), Some(current));
}

#[test]
fn a_failed_refresh_leaves_the_instance_unavailable_until_the_next() {
    let mut registry = host_of(&[("api", api())]);
    let boot_jobs = registry.jobs;
    let _ = set(&mut registry, Provider::new("claude"), gateway("t"), "set");
    let failed = registry.refreshed(refreshed(
        boot_jobs + 1,
        &api(),
        Err(text("probe failed")),
    ));
    assert_eq!(
        failed,
        Vec::from([
            Effect::Log(Note::RefreshFailed {
                instance: api(),
                reason: text("probe failed"),
            }),
            Effect::Reply("set", InstanceResult::Accepted),
        ])
    );
    assert_eq!(
        (
            availability(&registry, &api()),
            registry.refreshed(refreshed(boot_jobs + 1, &api(), Ok(listing()))),
        ),
        (Some(unavailable("probe failed")), Vec::new())
    );
}

#[test]
fn credentials_are_checked_and_an_unreadable_store_is_only_moved_aside() {
    let mut registry =
        registry(OwnSignIn::Unmanaged, &Stored::Unreadable(text("sealed")));
    let bad = gateway_at("ftp://x", "t");
    let not_saved = |reply, reason: &str| {
        Vec::from([Effect::Reply(
            reply,
            refuse(InstanceRefusal::CredentialNotSaved {
                reason: text(reason),
            }),
        )])
    };
    let codex = own(Provider::new("codex"));
    assert_eq!(
        (
            registry.set_credential(Provider::new("claude"), bad, "bad"),
            registry.set_credential(
                Provider::new("claude"),
                gateway("t"),
                "set"
            ),
            registry.remove_instance(api(), &BTreeSet::new(), "unknown"),
            registry.remove_instance(codex.clone(), &BTreeSet::new(), "remove"),
            registry.moved_aside(codex.clone(), "remove", Err(text("busy"))),
            registry.saved(
                Roster::default(),
                Change::Credential(api()),
                "save",
                Err(text("disk full"))
            ),
        ),
        (
            Vec::from([Effect::Reply(
                "bad",
                refuse(InstanceRefusal::InvalidCredential {
                    reason: text("base_url must be http or https")
                })
            )]),
            not_saved("set", "sealed"),
            Vec::from([Effect::Reply(
                "unknown",
                refuse(InstanceRefusal::UnknownInstance)
            )]),
            Vec::from([Effect::MoveAside {
                instance: codex.clone(),
                reply: "remove",
            }]),
            not_saved("remove", "busy"),
            not_saved("save", "disk full"),
        )
    );
    let moved = registry.moved_aside(codex, "remove", Ok(()));
    assert_eq!(
        moved,
        Vec::from([Effect::Reply("remove", InstanceResult::Accepted)])
    );
    assert_eq!(
        registry.set_credential(Provider::new("claude"), gateway("t"), "set"),
        Vec::from([Effect::Save {
            roster: with_credential(defaults(&catalog()), &api(), "t"),
            change: Change::Added(api()),
            reply: "set",
        }])
    );
}

#[test]
fn an_instance_the_host_cannot_run_answers_a_credential_change_at_once() {
    let mut registry = registry(OwnSignIn::Unmanaged, &opened());
    let codex = own(Provider::new("codex"));
    assert_eq!(
        registry.saved(
            defaults(&catalog()),
            Change::Credential(codex.clone()),
            "set",
            Ok(())
        ),
        Vec::from([
            stored_note(&codex),
            Effect::Reply("set", InstanceResult::Accepted),
        ])
    );
    assert_eq!(
        registry.launch_session(&codex, true, None, None),
        Err(SessionRefusal::InstanceUnavailable {
            reason: text("no codex")
        })
    );
}

#[test]
fn the_default_is_the_credential_s_model_then_the_chosen_then_the_cli_s() {
    let offer = |named: Option<&str>, chosen: Option<&str>| {
        offered(
            Configuration {
                models: Ok(listed()),
                named: named.map(model),
            },
            chosen.map(model).as_ref(),
        )
        .ok()
        .and_then(|models| models.default)
    };
    assert_eq!(
        [
            offer(Some("gw"), Some("flag")),
            offer(None, Some("flag")),
            offer(None, None),
        ],
        [Some(model("gw")), Some(model("flag")), Some(model("big"))]
    );
}

#[test]
fn a_sign_in_that_names_no_account_signs_the_home_in_and_relists_its_models() {
    let mut registry = registry(OwnSignIn::Missing, &opened());
    let boot_jobs = registry.jobs;
    let claude = own(Provider::new("claude"));
    let before = availability(&registry, &claude);
    let refresh =
        registry.signed_in(claude.clone(), None, &BTreeSet::new(), "signed in");
    let during = availability(&registry, &claude);
    let answered =
        registry.refreshed(refreshed(boot_jobs + 1, &claude, Ok(listing())));
    assert_eq!(
        (
            before,
            refresh,
            during,
            availability(&registry, &claude),
            registry.settings(&claude),
            registry.settings(&api()),
        ),
        (
            Some(unavailable("no credential configured")),
            Vec::from([Effect::Refresh(job(
                boot_jobs + 1,
                &claude,
                "claude",
                None
            ))]),
            Some(unavailable("refreshing the instance's models")),
            Some(Availability::Available {
                auth: Auth::SignedIn,
                models: listed(),
            }),
            Some(Instance::default()),
            None,
        )
    );
    assert_eq!(
        answered,
        Vec::from([Effect::Reply("signed in", InstanceResult::Accepted)])
    );
}

#[test]
fn a_sign_in_that_names_an_account_makes_its_instance() {
    let mut registry = registry(OwnSignIn::Missing, &opened());
    let boot_jobs = registry.jobs;
    let claude = own(Provider::new("claude"));
    let me = InstanceId::named(Provider::new("claude"), "me@example.com");
    let signed = registry.signed_in(
        claude.clone(),
        Some(Account::Named(text("me@example.com"))),
        &BTreeSet::new(),
        "signed in",
    );
    let mut roster = defaults(&catalog());
    roster.0.insert(me.clone(), Instance::default());
    assert_eq!(
        signed,
        Vec::from([Effect::Save {
            roster,
            change: Change::SignedIn(AccountAdoption {
                from: claude.clone(),
                to: me.clone(),
            }),
            reply: "signed in",
        }])
    );
    assert_eq!(
        saving(&mut registry, signed),
        Vec::from([
            Effect::Log(Note::InstanceAdded {
                instance: me.clone()
            }),
            Effect::Start(Starting {
                mode: crate::start::Mode::Change,
                id: boot_jobs + 1,
                instance: me.clone(),
                settings: Instance::default(),
            }),
            Effect::Start(Starting {
                mode: crate::start::Mode::Change,
                id: boot_jobs + 2,
                instance: claude.clone(),
                settings: Instance::default(),
            }),
        ])
    );
    let starting = availability(&registry, &me);
    let own_started = registry.started(Started {
        id: boot_jobs + 2,
        instance: claude.clone(),
        launcher: ready("claude", OwnSignIn::Missing),
    });
    let answered = registry.started(Started {
        id: boot_jobs + 1,
        instance: me.clone(),
        launcher: ready("me", OwnSignIn::SignedIn),
    });
    assert_eq!(
        (starting, own_started, answered),
        (
            Some(unavailable("starting the instance's CLI")),
            Vec::new(),
            Vec::from([Effect::Reply("signed in", InstanceResult::Accepted)]),
        )
    );
    assert_eq!(
        (
            availability(&registry, &me),
            availability(&registry, &claude),
            registry.signed_in(
                me.clone(),
                Some(Account::Named(text("me@example.com"))),
                &BTreeSet::new(),
                "again"
            ),
        ),
        (
            Some(Availability::Available {
                auth: Auth::SignedIn,
                models: listed(),
            }),
            Some(unavailable("no credential configured")),
            Vec::from([Effect::Refresh(job(boot_jobs + 3, &me, "me", None))]),
        )
    );
}

#[test]
fn an_account_signed_in_from_a_credential_s_instance_takes_no_credential() {
    let me = InstanceId::named(Provider::new("claude"), "me@example.com");
    let mut signed = defaults(&catalog());
    signed.0.insert(me.clone(), Instance::default());
    assert_eq!(
        AccountAdoption {
            from: api(),
            to: me
        }
        .plan(&with_credential(defaults(&catalog()), &api(), "t")),
        Some(signed)
    );
}

#[test]
fn an_instance_has_one_session_on_an_issue_until_it_is_removed()
-> Result<(), &'static str> {
    let issue = home::IssueId::new("00000005")
        .map(|id| home::IssueKey {
            project: home::ProjectId::new(1),
            id,
        })
        .ok_or("invalid fixture identity")?;
    let event = |session: u128, kind: EventKind| Event {
        seq: HostSeq(1),
        session: SessionId(session),
        kind,
    };
    let created = |session: u128, instance: InstanceId| {
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
    };
    let own = InstanceId::own(Provider::new("claude"));
    let other =
        InstanceId::named(Provider::new("claude"), "https://gateway.example");
    let history = Vec::from([
        created(1, own.clone()),
        event(
            1,
            EventKind::OnIssue {
                issue: issue.clone(),
            },
        ),
        created(2, other.clone()),
    ]);
    assert_refused(
        start_request().placed(Ok(placement(true)?), &history),
        SessionRefusal::IssueTaken {
            session: SessionId(1),
        },
    );
    assert_eq!(super::on_issue(&history, &issue, &other), None);
    assert_eq!(
        super::on_issue(
            &history,
            &home::IssueId::new("00000006")
                .map(|id| home::IssueKey {
                    project: home::ProjectId::new(1),
                    id
                })
                .ok_or("invalid fixture identity")?,
            &own
        ),
        None
    );
    let removed = [
        history,
        Vec::from([event(1, EventKind::SessionRemoved { kept: None })]),
    ]
    .concat();
    assert!(matches!(
        start_request().placed::<&str, u8>(Ok(placement(true)?), &removed),
        Continue(StartStep {
            effect: StartEffect::ReadyBranch(_),
            ..
        })
    ));
    Ok(())
}

#[test]
fn a_brief_that_did_not_reach_the_agent_says_why() {
    assert_eq!(super::brief_sent(Some(&SessionResult::Accepted)), None);
    assert_eq!(
        super::brief_sent(Some(&SessionResult::Refused(
            SessionRefusal::SessionClosed
        ))),
        Some(EventKind::Error {
            message: text(
                "The issue's brief was not sent: the session has closed. Send it again from the issue.",
            ),
        })
    );
    for answer in [
        None,
        Some(&SessionResult::Started {
            session: SessionId(1),
        }),
    ] {
        assert!(matches!(
            super::brief_sent(answer),
            Some(EventKind::Error { .. })
        ));
    }
}

fn start_request() -> super::StartRequest {
    super::StartRequest {
        instance: own(Provider::new("claude")),
        model: None,
        effort: None,
        by: ClientName(text("phone")),
    }
}

fn placement(on_issue: bool) -> Result<super::Placement, &'static str> {
    let issue = home::IssueId::new("00000005")
        .map(|id| home::IssueKey {
            project: home::ProjectId::new(1),
            id,
        })
        .ok_or("invalid fixture identity")?;
    Ok(super::Placement {
        project: None,
        directory: SessionDirectory(text("/src")),
        on: on_issue.then(|| home::issue::Start {
            issue,
            checkout: CheckoutLocation(text("/src")),
            branch: text("refs/heads/issue/5"),
            brief: text("Implement the issue"),
        }),
    })
}

fn home_with(checkouts: &[&str]) -> Home {
    Home {
        remote: text("origin"),
        synced_secs: None,
        sync: HomeSync::Synced,
        unpushed: 0,
        flow: None,
        projects: Vec::from([home::Project {
            id: ProjectId::new(1),
            name: text("conductor"),
            title: text("Conductor"),
            remote: None,
            profiles: Vec::new(),
            provides: Vec::new(),
            checkouts: checkouts
                .iter()
                .map(|path| home::Checkout {
                    path: CheckoutLocation(text(path)),
                    origin: home::Origin::Found,
                })
                .collect(),
            cloning: home::Cloning::Idle,
            issues: Vec::new(),
        }]),
    }
}

fn place(
    at: StartAt,
    home: Option<&Home>,
) -> Result<super::Placement, HomeRefusal> {
    place_without_home(at).or_else(|start| place_in_home(start, home))
}

#[test]
fn a_session_starts_in_a_directory_or_a_projects_first_checkout() {
    let directory = SessionDirectory(text("/work"));
    let shown = home_with(&["/a", "/b"]);
    let unchecked = home_with(&[]);
    let project = ProjectId::new(1);
    assert_eq!(
        [
            place(StartAt::Directory(directory.clone()), None),
            place(StartAt::Project(project), Some(&shown)),
            place(StartAt::Project(project), None),
            place(StartAt::Project(ProjectId::new(2)), Some(&shown)),
            place(StartAt::Project(project), Some(&unchecked)),
        ]
        .map(|placed| placed.map(|at| at.directory)),
        [
            Ok(directory),
            Ok(SessionDirectory(text("/a"))),
            Err(HomeRefusal::NoHome),
            Err(HomeRefusal::UnknownProject),
            Err(HomeRefusal::NotCheckedOut),
        ]
    );
}

#[test]
fn only_a_start_in_a_project_or_an_issue_waits_on_the_home()
-> Result<(), &'static str> {
    let directory = SessionDirectory(text("/work"));
    let project = ProjectId::new(1);
    let issue = home::IssueId::new("00000105")
        .map(|id| home::IssueKey {
            project: home::ProjectId::new(1),
            id,
        })
        .ok_or("invalid fixture identity")?;
    assert_eq!(
        [
            place_without_home(StartAt::Directory(directory.clone())),
            place_without_home(StartAt::Project(project)),
            place_without_home(StartAt::Issue(issue.clone())),
        ],
        [
            Ok(super::Placement {
                project: None,
                directory,
                on: None,
            }),
            Err(InHome::Project(project)),
            Err(InHome::Issue(issue)),
        ]
    );
    Ok(())
}

fn start_workspace() -> crate::workspace::Workspace {
    crate::workspace::Workspace {
        repository: Repository(text("/src")),
        original: WorkingCopy(text("/src")),
        worktree: Worktree(text("/worktrees/1")),
        directory: SessionDirectory(text("/worktrees/1")),
        branch: text("refs/heads/session/1"),
    }
}

type Decision = StartDecision<&'static str, u8>;
type Workflow = StartSession<&'static str, u8>;

fn assert_refused(decision: Decision, reason: SessionRefusal) {
    assert!(matches!(decision, Break(StartEnd::Finished(outcome))
        if outcome.result == SessionResult::Refused(reason) && outcome.progress.is_none()));
}

fn at_directory(
    request: super::StartRequest,
    registry: &mut Test,
) -> Result<Workflow, &'static str> {
    let placed = placement(true)?;
    let Continue(StartStep {
        effect: StartEffect::ReadyBranch(branch),
        next,
    }) = request.placed(Ok(placed.clone()), &[])
    else {
        return Err("expected branch readiness");
    };
    assert_eq!(Some(branch), placed.on);
    let Continue(StartStep {
        effect: StartEffect::CheckDirectory(path),
        next: check,
    }) = next.report(registry, StartReport::Branch(Ok(())))
    else {
        return Err("expected a directory check");
    };
    assert_eq!(path, placed.directory);
    Ok(check)
}

fn at_workspace(registry: &mut Test) -> Result<Workflow, &'static str> {
    let check = at_directory(start_request(), registry)?;
    let Continue(StartStep {
        effect: StartEffect::CreateWorkspace { directory, base },
        next,
    }) = check.report(registry, StartReport::Directory(true))
    else {
        return Err("expected workspace creation");
    };
    assert_eq!(directory, SessionDirectory(text("/src")));
    assert_eq!(base.as_deref(), Some("refs/heads/issue/5"));
    Ok(next)
}

fn at_snapshot(
    registry: &mut Test,
    session: SessionId,
) -> Result<Workflow, &'static str> {
    let workspace = start_workspace();
    let prepare = at_workspace(registry)?;
    let Continue(StartStep {
        effect:
            StartEffect::Snapshot {
                session: id,
                workspace: worktree,
            },
        next,
    }) = prepare.report(
        registry,
        StartReport::Workspace(session, Ok(workspace.clone())),
    )
    else {
        return Err("expected a workspace snapshot");
    };
    assert_eq!((id, worktree), (session, workspace));
    Ok(next)
}

#[test]
fn placement_and_branch_failures_end_the_start() -> Result<(), &'static str> {
    let mut registry = registry(OwnSignIn::SignedIn, &opened());
    assert_refused(
        start_request().placed(Err(HomeRefusal::NoHome), &[]),
        SessionRefusal::Home(HomeRefusal::NoHome),
    );
    let Continue(StartStep {
        effect: StartEffect::ReadyBranch(_),
        next,
    }) = start_request().placed(Ok(placement(true)?), &[])
    else {
        return Err("expected branch readiness");
    };
    let reason = SessionRefusal::WorkspaceFailure {
        reason: text("issue branches diverged"),
    };
    assert_refused(
        next.report(&mut registry, StartReport::Branch(Err(reason.clone()))),
        reason,
    );
    Ok(())
}

#[test]
fn a_start_checks_the_directory_and_instance_after_branch_readiness()
-> Result<(), &'static str> {
    let mut registry = registry(OwnSignIn::SignedIn, &opened());
    let unknown = super::StartRequest {
        instance: InstanceId::named(Provider::new("claude"), "unknown"),
        ..start_request()
    };
    for (exists, reason) in [
        (false, SessionRefusal::NoSuchDirectory),
        (true, SessionRefusal::UnknownInstance),
    ] {
        let check = at_directory(unknown.clone(), &mut registry)?;
        assert_refused(
            check.report(&mut registry, StartReport::Directory(exists)),
            reason,
        );
    }
    Ok(())
}

#[test]
fn workspace_and_snapshot_failures_stop_before_opening_records()
-> Result<(), &'static str> {
    let mut registry = registry(OwnSignIn::SignedIn, &opened());
    let session = SessionId(1);
    let reason = SessionRefusal::WorkspaceFailure {
        reason: text("git failed"),
    };
    for (error, refusal) in [
        (GitError::NotRepository, SessionRefusal::NotGitRepository),
        (GitError::Failed(text("git failed")), reason.clone()),
    ] {
        let prepare = at_workspace(&mut registry)?;
        assert_refused(
            prepare.report(
                &mut registry,
                StartReport::Workspace(session, Err(error)),
            ),
            refusal,
        );
    }
    let snapshot = at_snapshot(&mut registry, session)?;
    assert_refused(
        snapshot.report(
            &mut registry,
            StartReport::Snapshot(Err(GitError::Failed(text("git failed")))),
        ),
        reason,
    );
    Ok(())
}

fn recorded(session: SessionId, seed: [EventKind; 2]) -> [Event; 2] {
    let [workspace, snapshot] = seed;
    [workspace, snapshot].map(|kind| Event {
        seq: HostSeq(1),
        session,
        kind,
    })
}

fn initial_seed() -> [EventKind; 2] {
    [
        EventKind::WorkspaceCreated {
            workspace: start_workspace(),
        },
        EventKind::SnapshotRecorded {
            snapshot: crate::workspace::Snapshot {
                turn_start: None,
                commit: text("initial commit"),
            },
        },
    ]
}

#[test]
fn opening_records_seed_the_agent_and_issue_work_follows_spawn()
-> Result<(), &'static str> {
    let mut registry = registry(OwnSignIn::SignedIn, &opened());
    let session = SessionId(1);
    let snapshot = at_snapshot(&mut registry, session)?;
    let Continue(StartStep {
        effect:
            StartEffect::Record {
                session: id,
                before,
                seed,
            },
        next: opening,
    }) = snapshot.report(
        &mut registry,
        StartReport::Snapshot(Ok(text("initial commit"))),
    )
    else {
        return Err("expected opening records");
    };
    assert_eq!(id, session);
    assert_eq!(
        before,
        [
            EventKind::SessionCreated {
                instance: start_request().instance,
                directory: SessionDirectory(text("/src")),
                by: ClientName(text("phone")),
                model: Some(model("big")),
                effort: None,
            },
            EventKind::OnIssue {
                issue: home::IssueId::new("00000005")
                    .map(|id| home::IssueKey {
                        project: home::ProjectId::new(1),
                        id
                    })
                    .ok_or("invalid fixture identity")?
            },
        ]
    );
    assert_eq!(*seed, initial_seed());
    let records = recorded(session, *seed);
    let Continue(StartStep {
        effect:
            StartEffect::Spawn {
                session: id,
                history,
                ..
            },
        next: spawn,
    }) = opening.report(
        &mut registry,
        StartReport::Recorded(Box::new(records.clone())),
    )
    else {
        return Err("expected spawn");
    };
    assert_eq!((id, *history), (session, records));
    let Continue(StartStep {
        effect:
            StartEffect::Brief {
                session: id,
                handle,
                text: brief,
                by,
            },
        next: briefing,
    }) = spawn.report(&mut registry, StartReport::Spawned(7))
    else {
        return Err("expected issue brief");
    };
    assert_eq!(
        (id, handle, brief, by),
        (
            session,
            7,
            text("Implement the issue"),
            ClientName(text("phone"))
        )
    );
    let Break(StartEnd::Finished(done)) =
        briefing.report(&mut registry, StartReport::BriefQueued)
    else {
        return Err("expected completion");
    };
    assert_eq!(registry.find(session), Some(7));
    assert_eq!(done.result, SessionResult::Started { session });
    assert_eq!(
        done.progress,
        Some((
            home::IssueId::new("00000005")
                .map(|id| home::IssueKey {
                    project: home::ProjectId::new(1),
                    id
                })
                .ok_or("invalid fixture identity")?,
            ClientName(text("phone"))
        ))
    );
    Ok(())
}

#[test]
fn lost_opening_records_stop_the_start() -> Result<(), &'static str> {
    let mut registry = registry(OwnSignIn::SignedIn, &opened());
    let session = SessionId(1);
    let snapshot = at_snapshot(&mut registry, session)?;
    let Continue(StartStep {
        effect: StartEffect::Record { .. },
        next: opening,
    }) = snapshot
        .report(&mut registry, StartReport::Snapshot(Ok(text("commit"))))
    else {
        return Err("expected opening records");
    };
    assert!(matches!(
        opening.report(&mut registry, StartReport::Stopped),
        Break(StartEnd::Stopped)
    ));
    assert_eq!(registry.find(session), None);
    Ok(())
}

#[test]
fn a_plain_start_checks_its_directory_without_an_issue_branch()
-> Result<(), &'static str> {
    let mut registry = registry(OwnSignIn::SignedIn, &opened());
    let Continue(StartStep {
        effect: StartEffect::CheckDirectory(path),
        next,
    }) = start_request().placed(Ok(placement(false)?), &[])
    else {
        return Err("expected a directory check");
    };
    let Continue(StartStep {
        effect:
            StartEffect::CreateWorkspace {
                directory,
                base: None,
            },
        ..
    }) = next.report(&mut registry, StartReport::Directory(true))
    else {
        return Err("expected a workspace without an issue base");
    };
    assert_eq!(path, directory);
    Ok(())
}

#[test]
fn a_refresh_from_a_removed_instance_cannot_advance_its_replacement() {
    let instance = api();
    let mut registry = host_of(&[("api", instance.clone())]);
    let boot = registry.jobs;
    let _ = set(
        &mut registry,
        Provider::new("claude"),
        gateway("old"),
        "old credential",
    );
    let old = boot + 1;
    for effect in
        registry.remove_instance(instance.clone(), &BTreeSet::new(), "remove")
    {
        if let Effect::Save {
            roster,
            change,
            reply,
        } = effect
        {
            let _ = registry.saved(roster, change, reply, Ok(()));
        }
    }
    let _ = set(
        &mut registry,
        Provider::new("claude"),
        gateway("replacement"),
        "re-add",
    );
    let launched = registry.jobs;
    let _ = registry.started(Started {
        id: launched,
        instance: instance.clone(),
        launcher: ready("replacement", OwnSignIn::Missing),
    });
    let _ = set(
        &mut registry,
        Provider::new("claude"),
        gateway("current"),
        "current",
    );
    let active = registry.jobs;
    let _ = set(
        &mut registry,
        Provider::new("claude"),
        gateway("next"),
        "next",
    );
    let next = registry.jobs;

    assert_eq!(
        registry.refreshed(refreshed(old, &instance, Ok(listing()))),
        []
    );
    assert_eq!(
        registry.refreshed(refreshed(next, &instance, Ok(listing()))),
        []
    );
    assert_eq!(
        registry.refreshed(refreshed(active, &instance, Ok(listing()))),
        [Effect::Refresh(job(
            next,
            &instance,
            "replacement",
            Some(gateway("next"))
        ))]
    );
    assert_eq!(
        registry.refreshed(refreshed(old, &instance, Ok(listing()))),
        []
    );
    let complete =
        registry.refreshed(refreshed(next, &instance, Ok(listing())));
    let outcome = InstanceResult::Accepted;
    assert_eq!(
        complete,
        [
            Effect::Reply("current", outcome.clone()),
            Effect::Reply("next", outcome),
        ]
    );
}

#[test]
fn adoption_protects_sessions_on_the_source_and_destination() {
    let source = InstanceId::named(Provider::new("claude"), "source");
    let destination = InstanceId::named(Provider::new("claude"), "destination");
    let mut registry = host_of(&[
        ("source", source.clone()),
        ("destination", destination.clone()),
    ]);
    registry.admit(SessionId(1), source.clone(), 1);
    registry.admit(SessionId(2), destination, 2);
    for busy in [SessionId(1), SessionId(2)] {
        assert_eq!(
            registry.signed_in(
                source.clone(),
                Some(Account::Named(text("destination"))),
                &BTreeSet::from([busy]),
                "sign in",
            ),
            [Effect::Reply(
                "sign in",
                refuse(InstanceRefusal::InstanceInUse { sessions: 1 })
            )],
        );
    }
    assert!(matches!(
        registry
            .signed_in(
                source,
                Some(Account::Named(text("destination"))),
                &BTreeSet::new(),
                "sign in"
            )
            .as_slice(),
        [Effect::Save {
            change: Change::SignedIn(_),
            ..
        }]
    ));
}
