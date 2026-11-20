use std::collections::{BTreeMap, BTreeSet};

use proto::{
    Account, Auth, Availability, Catalog, ClientName, CloseReason,
    CommandResult, Credential, CredentialForm, Effort, Event, EventKind, Field,
    FieldKind, HostPath, HostSeq, Instance, InstanceId, InstanceStatus,
    Instances, ModelChoice, ModelId, Models, Provider, ProviderInfo, Rejection,
    Roster, SessionId,
};

use super::{
    Change, Effect, Job, Launch, Launcher, Listing, Offer, OwnSignIn,
    Refreshed, Registry, Started, Starting, Stored, defaults, dir, picked,
    running, signed_in_as, unprepared,
};
use crate::Note;
use crate::session::Choice;
use crate::workspace::GitError;

type Test = Registry<&'static str, u8, &'static str>;
type Seen = Vec<Effect<&'static str, &'static str>>;

const GATEWAY: &str = "https://gateway.example";

fn text(value: &str) -> String {
    String::from(value)
}

fn own(provider: Provider) -> InstanceId {
    InstanceId::own(provider)
}

/// Claude's instance of the gateway account.
fn api() -> InstanceId {
    InstanceId::named(Provider::Claude, GATEWAY)
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
            efforts: Vec::from([Effort::Low, Effort::High]),
        }]),
        default: Some(model("big")),
    }
}

fn ready(start: &'static str, own: OwnSignIn) -> Launcher<&'static str> {
    Launcher::Ready {
        start,
        own,
        offer: Offer {
            listed: listed(),
            named: None,
            chosen: None,
        },
    }
}

/// A fresh host: Claude's own instance runs and signs in as `own` says,
/// and Codex's can't run.
fn registry(own_sign_in: OwnSignIn, stored: &Stored) -> Test {
    Registry::new(
        BTreeMap::from([
            (own(Provider::Claude), ready("claude", own_sign_in)),
            (
                own(Provider::Codex),
                Launcher::Unavailable(text("no codex")),
            ),
        ]),
        stored,
        catalog(),
    )
}

/// A host with these instances besides the own ones, each running on the
/// starter named with it.
fn host_of(instances: &[(&'static str, InstanceId)]) -> Test {
    let mut launchers = BTreeMap::from([
        (own(Provider::Claude), ready("claude", OwnSignIn::Unmanaged)),
        (own(Provider::Codex), ready("codex", OwnSignIn::Unmanaged)),
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
    Registry::new(launchers, &Stored::Opened(roster), catalog())
}

/// Each provider takes a gateway: a URL, which is its account, and a
/// token.
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
        }]),
    };
    Catalog(BTreeMap::from([
        (Provider::Claude, info("Claude Code")),
        (Provider::Codex, info("Codex")),
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

/// The own instances, Claude's as `availability` says.
fn claude(availability: Availability) -> Instances {
    Instances(BTreeMap::from([
        (own(Provider::Claude), InstanceStatus { availability }),
        (
            own(Provider::Codex),
            InstanceStatus {
                availability: unavailable("no codex"),
            },
        ),
    ]))
}

fn availability(registry: &Test, id: &InstanceId) -> Option<Availability> {
    registry.instances().availability(id).cloned()
}

fn rejected(rejection: Rejection) -> CommandResult {
    CommandResult::Rejected(rejection)
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
        instance: id.clone(),
        start,
        credential,
        signs_in: Ok(()),
    }
}

fn refreshed(
    number: u64,
    id: &InstanceId,
    listing: Result<Listing, String>,
) -> Refreshed {
    Refreshed {
        id: number,
        instance: id.clone(),
        listing,
    }
}

fn listing() -> Listing {
    Listing {
        models: listed(),
        named: None,
    }
}

/// Performs a Save the registry asks for, as the shell would.
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
    let claude = own(Provider::Claude);
    assert_eq!(
        [
            registry.start(&claude, false, None, None),
            registry.start(&api(), true, None, None),
            registry.start(&own(Provider::Codex), true, None, None),
            registry.start(&claude, true, Some(model("tiny")), None),
            registry.start(&claude, true, None, Some(Effort::Max)),
        ],
        [
            Err(Rejection::NoSuchDirectory),
            Err(Rejection::UnknownInstance),
            Err(Rejection::InstanceUnavailable {
                reason: text("no codex")
            }),
            Err(Rejection::NotOffered {
                reason: text("this host doesn't offer the model tiny")
            }),
            Err(Rejection::NotOffered {
                reason: text("Big doesn't take the effort max")
            }),
        ]
    );
    assert_eq!(
        registry.start(&claude, true, None, Some(Effort::High)),
        Ok(Launch {
            start: "claude",
            choice: Choice {
                model: Some(model("big")),
                effort: Some(Effort::High),
            },
            offered: listed(),
            provider: Provider::Claude,
        })
    );
}

#[test]
fn every_host_has_each_provider_s_own_instance_beside_its_accounts() {
    let defaults = defaults(&catalog());
    let accounts = Roster(BTreeMap::from([(api(), Instance::default())]));
    assert_eq!(
        defaults.0.keys().cloned().collect::<Vec<InstanceId>>(),
        [own(Provider::Claude), own(Provider::Codex)]
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
            dir(&own(Provider::Claude)),
            dir(&InstanceId::named(Provider::Claude, "me@example.com")),
            dir(&InstanceId::named(Provider::Codex, "me@example.com")),
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
            picked(&roster, Provider::Claude, None),
            picked(&roster, Provider::Claude, Some(GATEWAY)),
            picked(&roster, Provider::Claude, Some("me@example.com")),
            picked(&roster, Provider::Codex, Some("me@example.com")),
        ],
        [
            Ok(own(Provider::Claude)),
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
        missing.start(&own(Provider::Claude), true, None, None),
        Err(Rejection::InstanceUnavailable {
            reason: text("no credential configured")
        })
    );
    assert_eq!(
        Registry::<&str, u8, &str>::new(BTreeMap::new(), &opened(), catalog())
            .start(&own(Provider::Claude), true, None, None),
        Err(Rejection::InstanceUnavailable {
            reason: text("this host has no instance claude")
        })
    );
}

#[test]
fn a_session_is_found_and_continued_only_while_its_instance_runs() {
    let mut registry = host_of(&[("api", api())]);
    registry.admit(SessionId(1), own(Provider::Claude), 7);
    registry.admit(SessionId(2), api(), 8);
    assert_eq!(
        (
            registry.find(SessionId(1)),
            registry.find(SessionId(3)),
            registry.relaunch(SessionId(1)),
            registry.relaunch(SessionId(2)),
            registry.relaunch(SessionId(3)),
            registry.launcher(&own(Provider::Claude)),
            registry.launcher(&api()),
        ),
        (
            Some(7),
            None,
            Ok((7, listed())),
            Err(Rejection::InstanceUnavailable {
                reason: text("no credential configured")
            }),
            Err(Rejection::UnknownSession),
            Ok("claude"),
            Ok("api"),
        )
    );
    let _ = set(&mut registry, Provider::Claude, gateway("t"), "set");
    assert_eq!(
        registry.relaunch(SessionId(2)),
        Err(Rejection::InstanceUnavailable {
            reason: text("refreshing the instance's models")
        })
    );
}

#[test]
fn two_instances_of_one_provider_keep_their_own_credentials_and_agents() {
    let mut registry = host_of(&[("api", api())]);
    assert_eq!(
        set(&mut registry, Provider::Claude, gateway("api"), "set"),
        Vec::from([
            stored_note(&api()),
            Effect::Refresh(job(1, &api(), "api", Some(gateway("api")))),
        ])
    );
    assert_eq!(
        (
            availability(&registry, &own(Provider::Claude)),
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
    let _ = registry.refreshed(refreshed(1, &api(), Ok(listing())));
    assert_eq!(
        availability(&registry, &api()),
        Some(Availability::Available {
            auth: stored_auth(),
            models: listed(),
        })
    );
    let started = |registry: &Test, id: &InstanceId| {
        registry
            .start(id, true, None, None)
            .map(|launch| (launch.start, launch.provider))
    };
    assert_eq!(
        [
            started(&registry, &own(Provider::Claude)),
            started(&registry, &api())
        ],
        [
            Ok(("claude", Provider::Claude)),
            Ok(("api", Provider::Claude))
        ]
    );
}

#[test]
fn each_codex_instance_refreshes_its_own_app_server() {
    let codex_api = InstanceId::named(Provider::Codex, GATEWAY);
    let mut registry = host_of(&[("codex-api", codex_api.clone())]);
    assert_eq!(
        set(&mut registry, Provider::Codex, gateway("key"), "set"),
        Vec::from([
            stored_note(&codex_api),
            Effect::Refresh(job(
                1,
                &codex_api,
                "codex-api",
                Some(gateway("key"))
            )),
        ])
    );
    registry.admit(SessionId(1), own(Provider::Codex), 1);
    assert_eq!(
        (
            registry
                .start(&own(Provider::Codex), true, None, None)
                .map(|launch| launch.start),
            registry.relaunch(SessionId(1)),
            registry
                .start(&codex_api, true, None, None)
                .map(|launch| launch.start),
        ),
        (
            Ok("codex"),
            Ok((1, listed())),
            Err(Rejection::InstanceUnavailable {
                reason: text("refreshing the instance's models")
            }),
        )
    );
}

#[test]
fn a_credential_of_a_new_account_adds_its_instance_and_starts_its_cli() {
    let mut registry = registry(OwnSignIn::Unmanaged, &opened());
    let added = Instance {
        credential: Some(gateway("t")),
        ..Instance::default()
    };
    assert_eq!(
        registry.set_credential(Provider::Claude, gateway("t"), "add"),
        Vec::from([Effect::Save {
            roster: with_credential(defaults(&catalog()), &api(), "t"),
            change: Change::Added(api()),
            reply: "add",
        }])
    );
    assert_eq!(
        set(&mut registry, Provider::Claude, gateway("t"), "add"),
        Vec::from([
            Effect::Log(Note::InstanceAdded { instance: api() }),
            Effect::Start(Starting {
                id: 1,
                instance: api(),
                settings: added,
            }),
        ])
    );
    assert_eq!(
        (
            availability(&registry, &api()),
            set(&mut registry, Provider::Claude, gateway("t2"), "set"),
        ),
        (
            Some(unavailable("starting the instance's CLI")),
            Vec::from([stored_note(&api())]),
        )
    );
    assert_eq!(
        registry.started(Started {
            id: 1,
            instance: api(),
            launcher: ready("api", OwnSignIn::Missing),
        }),
        Vec::from([Effect::Refresh(job(
            2,
            &api(),
            "api",
            Some(gateway("t2"))
        ))])
    );
    let result =
        |registry: &Test| CommandResult::Instances(registry.instances());
    assert_eq!(
        registry.refreshed(refreshed(2, &api(), Ok(listing()))),
        Vec::from([
            Effect::Reply("add", result(&registry)),
            Effect::Reply("set", result(&registry)),
        ])
    );
    assert_eq!(
        registry.set_credential(Provider::Codex, gateway_at("", "t"), "none"),
        Vec::from([Effect::Reply(
            "none",
            rejected(Rejection::InvalidCredential {
                reason: text("base_url is required")
            })
        )])
    );
}

#[test]
fn a_start_for_an_instance_since_removed_is_dropped() {
    let mut registry = registry(OwnSignIn::Unmanaged, &opened());
    let codex_api = InstanceId::named(Provider::Codex, GATEWAY);
    let _ = set(&mut registry, Provider::Codex, gateway("k"), "add");
    let removed =
        registry.remove_instance(codex_api.clone(), &BTreeSet::new(), "remove");
    let result = CommandResult::Instances(claude(Availability::Available {
        auth: Auth::Unmanaged,
        models: listed(),
    }));
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
            id: 1,
            instance: codex_api.clone(),
            launcher: ready("codex-api", OwnSignIn::Unmanaged),
        }),
        Vec::new()
    );
    assert_eq!(registry.instances().of(&codex_api), None);
}

/// Starting an instance added again runs only its new CLI: the start of
/// the one removed meanwhile is dropped.
#[test]
fn a_start_from_before_an_instance_was_added_again_is_dropped() {
    let mut registry = registry(OwnSignIn::Unmanaged, &opened());
    let codex_api = InstanceId::named(Provider::Codex, GATEWAY);
    let _ = set(&mut registry, Provider::Codex, gateway("k"), "add");
    let removed =
        registry.remove_instance(codex_api.clone(), &BTreeSet::new(), "remove");
    let _ = saving(&mut registry, removed);
    let again = set(&mut registry, Provider::Codex, gateway("k"), "again");
    let stale = registry.started(Started {
        id: 1,
        instance: codex_api.clone(),
        launcher: ready("old", OwnSignIn::Unmanaged),
    });
    let starting = availability(&registry, &codex_api);
    let current = registry.started(Started {
        id: 2,
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
                    id: 2,
                    instance: codex_api,
                    settings: Instance {
                        credential: Some(gateway("k")),
                        ..Instance::default()
                    },
                }),
            ]),
            Vec::new(),
            Some(unavailable("starting the instance's CLI")),
            Vec::from([Effect::Reply(
                "again",
                CommandResult::Instances(registry.instances())
            )]),
        )
    );
}

#[test]
fn an_instance_is_removed_only_while_no_session_runs_on_it() {
    let mut registry = host_of(&[("api", api())]);
    registry.admit(SessionId(1), api(), 1);
    registry.admit(SessionId(2), api(), 2);
    registry.admit(SessionId(3), own(Provider::Codex), 3);
    assert_eq!(
        registry.remove_instance(
            api(),
            &BTreeSet::from([SessionId(1), SessionId(3)]),
            "busy"
        ),
        Vec::from([Effect::Reply(
            "busy",
            rejected(Rejection::InstanceInUse { sessions: 1 })
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
            Err(Rejection::InstanceUnavailable {
                reason: text(
                    "this host has no instance claude https://gateway.example"
                )
            }),
            Some(2),
        )
    );
    let kept = registry.remove_instance(
        own(Provider::Claude),
        &BTreeSet::new(),
        "own",
    );
    assert_eq!(
        kept,
        Vec::from([Effect::Reply(
            "own",
            CommandResult::Instances(registry.instances())
        )])
    );
    assert!(registry.instances().of(&own(Provider::Claude)).is_some());
}

#[test]
fn running_sessions_are_those_created_or_ready_and_not_closed_since() {
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
                instance: own(Provider::Claude),
                provider: Provider::Claude,
                directory: HostPath(text("/work")),
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
    assert_eq!(
        set(&mut registry, Provider::Claude, gateway("first"), "first"),
        Vec::from([
            stored_note(&api()),
            Effect::Refresh(job(1, &api(), "api", Some(gateway("first"))))
        ])
    );
    assert_eq!(
        (
            set(&mut registry, Provider::Claude, gateway("middle"), "middle"),
            set(&mut registry, Provider::Claude, gateway("newest"), "newest"),
            availability(&registry, &api()),
        ),
        (
            Vec::from([stored_note(&api())]),
            Vec::from([stored_note(&api())]),
            Some(unavailable("refreshing the instance's models")),
        )
    );
    let offered = Listing {
        models: Models::none(),
        named: Some(model("gw")),
    };
    assert_eq!(
        registry.refreshed(refreshed(1, &api(), Ok(offered.clone()))),
        Vec::from([Effect::Refresh(job(
            3,
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
    let answered =
        registry.refreshed(refreshed(3, &api(), Ok(offered.clone())));
    let result = CommandResult::Instances(registry.instances());
    assert_eq!(
        answered,
        Vec::from([
            Effect::Reply("first", result.clone()),
            Effect::Reply("middle", result.clone()),
            Effect::Reply("newest", result),
        ])
    );
    assert_eq!(
        registry.refreshed(refreshed(2, &api(), Ok(offered))),
        Vec::new()
    );
    assert_eq!(availability(&registry, &api()), Some(current));
}

#[test]
fn a_failed_refresh_leaves_the_instance_unavailable_until_the_next() {
    let mut registry = host_of(&[("api", api())]);
    let _ = set(&mut registry, Provider::Claude, gateway("t"), "set");
    let failed =
        registry.refreshed(refreshed(1, &api(), Err(text("probe failed"))));
    assert_eq!(
        failed,
        Vec::from([
            Effect::Log(Note::RefreshFailed {
                instance: api(),
                reason: text("probe failed"),
            }),
            Effect::Reply(
                "set",
                CommandResult::Instances(registry.instances())
            ),
        ])
    );
    assert_eq!(
        (
            availability(&registry, &api()),
            registry.refreshed(refreshed(1, &api(), Ok(listing()))),
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
            rejected(Rejection::CredentialNotSaved {
                reason: text(reason),
            }),
        )])
    };
    let codex = own(Provider::Codex);
    assert_eq!(
        (
            registry.set_credential(Provider::Claude, bad, "bad"),
            registry.set_credential(Provider::Claude, gateway("t"), "set"),
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
                rejected(Rejection::InvalidCredential {
                    reason: text("base_url must be http or https")
                })
            )]),
            not_saved("set", "sealed"),
            Vec::from([Effect::Reply(
                "unknown",
                rejected(Rejection::UnknownInstance)
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
        Vec::from([Effect::Reply(
            "remove",
            CommandResult::Instances(registry.instances())
        )])
    );
    assert_eq!(
        registry.set_credential(Provider::Claude, gateway("t"), "set"),
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
    let codex = own(Provider::Codex);
    assert_eq!(
        registry.saved(
            defaults(&catalog()),
            Change::Credential(codex.clone()),
            "set",
            Ok(())
        ),
        Vec::from([
            stored_note(&codex),
            Effect::Reply(
                "set",
                CommandResult::Instances(claude(Availability::Available {
                    auth: Auth::Unmanaged,
                    models: listed(),
                }))
            ),
        ])
    );
    assert_eq!(
        registry.start(&codex, true, None, None),
        Err(Rejection::InstanceUnavailable {
            reason: text("no codex")
        })
    );
}

#[test]
fn the_default_is_the_credential_s_model_then_the_chosen_then_the_cli_s() {
    let offer = |named: Option<&str>, chosen: Option<&str>| {
        Offer {
            listed: listed(),
            named: named.map(model),
            chosen: chosen.map(model),
        }
        .models()
        .default
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
fn a_directory_outside_git_is_refused_as_such() {
    assert_eq!(
        [
            unprepared(GitError::NotRepository),
            unprepared(GitError::Failed(text("git worktree add: exit 128"))),
        ],
        [
            Rejection::NotGitRepository,
            Rejection::WorkspaceFailure {
                reason: text("git worktree add: exit 128")
            },
        ]
    );
}

#[test]
fn a_sign_in_that_names_no_account_signs_the_home_in_and_relists_its_models() {
    let mut registry = registry(OwnSignIn::Missing, &opened());
    let claude = own(Provider::Claude);
    let before = availability(&registry, &claude);
    let refresh = registry.signed_in(claude.clone(), None, "signed in");
    let during = availability(&registry, &claude);
    let answered = registry.refreshed(refreshed(1, &claude, Ok(listing())));
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
            Vec::from([Effect::Refresh(job(1, &claude, "claude", None))]),
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
        Vec::from([Effect::Reply(
            "signed in",
            CommandResult::Instances(registry.instances())
        )])
    );
}

/// The own instance's home becomes the account's instance, whose CLI
/// starts in it, and the own instance starts afresh for the next sign-in.
#[test]
fn a_sign_in_that_names_an_account_makes_its_instance() {
    let mut registry = registry(OwnSignIn::Missing, &opened());
    let claude = own(Provider::Claude);
    let me = InstanceId::named(Provider::Claude, "me@example.com");
    let signed = registry.signed_in(
        claude.clone(),
        Some(Account::Named(text("me@example.com"))),
        "signed in",
    );
    let mut roster = defaults(&catalog());
    roster.0.insert(me.clone(), Instance::default());
    assert_eq!(
        signed,
        Vec::from([Effect::Save {
            roster,
            change: Change::SignedIn {
                from: claude.clone(),
                to: me.clone(),
            },
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
                id: 1,
                instance: me.clone(),
                settings: Instance::default(),
            }),
            Effect::Start(Starting {
                id: 2,
                instance: claude.clone(),
                settings: Instance::default(),
            }),
        ])
    );
    let starting = availability(&registry, &me);
    let own_started = registry.started(Started {
        id: 2,
        instance: claude.clone(),
        launcher: ready("claude", OwnSignIn::Missing),
    });
    let answered = registry.started(Started {
        id: 1,
        instance: me.clone(),
        launcher: ready("me", OwnSignIn::SignedIn),
    });
    assert_eq!(
        (starting, own_started, answered),
        (
            Some(unavailable("starting the instance's CLI")),
            Vec::new(),
            Vec::from([Effect::Reply(
                "signed in",
                CommandResult::Instances(registry.instances())
            )]),
        )
    );
    assert_eq!(
        (
            availability(&registry, &me),
            availability(&registry, &claude),
            registry.signed_in(
                me.clone(),
                Some(Account::Named(text("me@example.com"))),
                "again"
            ),
        ),
        (
            Some(Availability::Available {
                auth: Auth::SignedIn,
                models: listed(),
            }),
            Some(unavailable("no credential configured")),
            Vec::from([Effect::Refresh(job(3, &me, "me", None))]),
        )
    );
}

/// The account's instance signs in on its own, so it keeps none of the
/// credential of the instance it signed in from.
#[test]
fn an_account_signed_in_from_a_credential_s_instance_takes_no_credential() {
    let me = InstanceId::named(Provider::Claude, "me@example.com");
    let mut signed = defaults(&catalog());
    signed.0.insert(me.clone(), Instance::default());
    assert_eq!(
        signed_in_as(
            &with_credential(defaults(&catalog()), &api(), "t"),
            &api(),
            Some(Account::Named(text("me@example.com"))),
        ),
        Some((signed, me))
    );
}

#[test]
fn an_instance_has_one_session_on_an_issue_until_it_is_removed() {
    let issue = proto::IssueId(5);
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
                provider: Provider::Claude,
                directory: HostPath(String::from("/src")),
                by: ClientName(String::from("phone")),
                model: None,
                effort: None,
            },
        )
    };
    let own = InstanceId::own(Provider::Claude);
    let other = InstanceId::named(Provider::Claude, "https://gateway.example");
    let history = Vec::from([
        created(1, own.clone()),
        event(1, EventKind::OnIssue { issue }),
        created(2, other.clone()),
    ]);
    assert_eq!(super::on_issue(&history, issue, &own), Some(SessionId(1)));
    assert_eq!(super::on_issue(&history, issue, &other), None);
    assert_eq!(super::on_issue(&history, proto::IssueId(6), &own), None);
    let removed = [
        history,
        Vec::from([event(1, EventKind::SessionRemoved { kept: None })]),
    ]
    .concat();
    assert_eq!(super::on_issue(&removed, issue, &own), None);
}

#[test]
fn a_brief_that_did_not_reach_the_agent_says_why() {
    assert_eq!(super::unbriefed(Some(&CommandResult::Accepted)), None);
    assert_eq!(
        super::unbriefed(Some(&CommandResult::Rejected(
            Rejection::SessionClosed
        ))),
        Some(String::from(
            "The issue's brief was not sent: the session has closed. Send it again from the issue."
        ))
    );
    assert!(
        super::unbriefed(None)
            .is_some_and(|message| message.contains("stopped"))
    );
}
