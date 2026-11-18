use std::collections::{BTreeMap, BTreeSet};

use proto::{
    Auth, Availability, Catalog, ClientName, CloseReason, CommandResult,
    Credential, CredentialForm, Effort, Event, EventKind, Field, FieldKind,
    HostPath, HostSeq, Instance, InstanceId, InstanceStatus, Instances,
    ModelChoice, ModelId, Models, Provider, ProviderInfo, Rejection, Roster,
    SessionId,
};

use super::{
    Change, Effect, Job, Launch, Launcher, Listing, Offer, OwnSignIn,
    Refreshed, Registry, Started, Starting, Stored, defaults, running, slug,
    unprepared,
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

fn id(name: &str) -> InstanceId {
    InstanceId::named(name)
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

/// A fresh host: its default Claude instance runs and signs in as `own`
/// says, and its Codex instance can't run.
fn registry(own: OwnSignIn, stored: &Stored) -> Test {
    Registry::new(
        BTreeMap::from([
            (id("claude"), ready("claude", own)),
            (id("codex"), Launcher::Unavailable(text("no codex"))),
        ]),
        stored,
        catalog(),
    )
}

/// A host with these instances, each running on a starter named after
/// it.
fn host_of(instances: &[(&'static str, &str, Provider)]) -> Test {
    Registry::new(
        instances
            .iter()
            .map(|(name, _, _)| (id(name), ready(name, OwnSignIn::Unmanaged)))
            .collect(),
        &Stored::Opened(roster(instances)),
        catalog(),
    )
}

fn roster(instances: &[(&'static str, &str, Provider)]) -> Roster {
    Roster(
        instances
            .iter()
            .map(|&(name, label, provider)| {
                (id(name), Instance::new(text(label), provider))
            })
            .collect(),
    )
}

/// Each provider takes a gateway: a URL and a token.
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
    Stored::Opened(defaults(&catalog()))
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

/// The default instances, Claude's as `availability` says.
fn claude(availability: Availability) -> Instances {
    Instances(BTreeMap::from([
        (
            id("claude"),
            InstanceStatus {
                label: text("Claude Code"),
                provider: Provider::Claude,
                availability,
            },
        ),
        (
            id("codex"),
            InstanceStatus {
                label: text("Codex"),
                provider: Provider::Codex,
                availability: unavailable("no codex"),
            },
        ),
    ]))
}

fn availability(registry: &Test, name: &str) -> Option<Availability> {
    registry.instances().availability(&id(name)).cloned()
}

fn rejected(rejection: Rejection) -> CommandResult {
    CommandResult::Rejected(rejection)
}

fn stored_note(name: &str) -> Effect<&'static str, &'static str> {
    Effect::Log(Note::CredentialStored { instance: id(name) })
}

fn job(
    number: u64,
    name: &'static str,
    credential: Option<Credential>,
) -> Job<&'static str> {
    Job {
        id: number,
        instance: id(name),
        start: name,
        credential,
        signs_in: Ok(()),
    }
}

fn refreshed(
    number: u64,
    name: &str,
    listing: Result<Listing, String>,
) -> Refreshed {
    Refreshed {
        id: number,
        instance: id(name),
        listing,
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
    name: &str,
    credential: Credential,
    reply: &'static str,
) -> Seen {
    let effects = registry.set_credential(id(name), credential, reply);
    saving(registry, effects)
}

fn add(
    registry: &mut Test,
    provider: Provider,
    label: &str,
    reply: &'static str,
) -> Seen {
    let effects = registry.add_instance(provider, label, reply);
    saving(registry, effects)
}

#[test]
fn a_start_is_refused_until_directory_instance_and_model_all_hold() {
    let registry = registry(OwnSignIn::Unmanaged, &opened());
    assert_eq!(
        [
            registry.start(&id("claude"), false, None, None),
            registry.start(&id("gemini"), true, None, None),
            registry.start(&id("codex"), true, None, None),
            registry.start(&id("claude"), true, Some(model("tiny")), None),
            registry.start(&id("claude"), true, None, Some(Effort::Max)),
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
        registry.start(&id("claude"), true, None, Some(Effort::High)),
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
fn a_fresh_host_has_one_instance_per_provider_named_after_it() {
    let defaults = roster(&[
        ("claude", "Claude Code", Provider::Claude),
        ("codex", "Codex", Provider::Codex),
    ]);
    assert_eq!(
        [
            Stored::Fresh.roster(&catalog()),
            Stored::Unreadable(text("sealed")).roster(&catalog()),
            Stored::Opened(Roster::default()).roster(&catalog()),
        ],
        [defaults.clone(), defaults, Roster::default()]
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
        missing.start(&id("claude"), true, None, None),
        Err(Rejection::InstanceUnavailable {
            reason: text("no credential configured")
        })
    );
    assert_eq!(
        Registry::<&str, u8, &str>::new(BTreeMap::new(), &opened(), catalog())
            .start(&id("claude"), true, None, None),
        Err(Rejection::InstanceUnavailable {
            reason: text("this host has no instance claude")
        })
    );
}

#[test]
fn a_session_is_found_and_continued_only_while_its_instance_runs() {
    let mut registry = registry(OwnSignIn::Unmanaged, &opened());
    registry.admit(SessionId(1), id("claude"), 7);
    registry.admit(SessionId(2), id("codex"), 8);
    assert_eq!(
        (
            registry.find(SessionId(1)),
            registry.find(SessionId(3)),
            registry.relaunch(SessionId(1)),
            registry.relaunch(SessionId(2)),
            registry.relaunch(SessionId(3)),
            registry.launcher(&id("claude")),
            registry.launcher(&id("codex")),
        ),
        (
            Some(7),
            None,
            Ok((7, listed())),
            Err(Rejection::InstanceUnavailable {
                reason: text("no codex")
            }),
            Err(Rejection::UnknownSession),
            Ok("claude"),
            Err(text("no codex")),
        )
    );
    let _ = set(&mut registry, "claude", gateway("t"), "set");
    assert_eq!(
        registry.relaunch(SessionId(1)),
        Err(Rejection::InstanceUnavailable {
            reason: text("refreshing the instance's models")
        })
    );
}

#[test]
fn two_instances_of_one_provider_keep_their_own_credentials_and_agents() {
    let mut registry = host_of(&[
        ("claude", "Claude (subscription)", Provider::Claude),
        ("claude-api", "Claude (API)", Provider::Claude),
    ]);
    assert_eq!(
        set(&mut registry, "claude-api", gateway("api"), "set"),
        Vec::from([
            stored_note("claude-api"),
            Effect::Refresh(job(1, "claude-api", Some(gateway("api")))),
        ])
    );
    assert_eq!(
        (
            availability(&registry, "claude"),
            availability(&registry, "claude-api"),
        ),
        (
            Some(Availability::Available {
                auth: Auth::Unmanaged,
                models: listed(),
            }),
            Some(unavailable("refreshing the instance's models")),
        )
    );
    let _ = registry.refreshed(refreshed(
        1,
        "claude-api",
        Ok(Listing {
            models: listed(),
            named: None,
        }),
    ));
    assert_eq!(
        availability(&registry, "claude-api"),
        Some(Availability::Available {
            auth: stored_auth(),
            models: listed(),
        })
    );
    let started = |registry: &Test, name: &str| {
        registry
            .start(&id(name), true, None, None)
            .map(|launch| (launch.start, launch.provider))
    };
    assert_eq!(
        [
            started(&registry, "claude"),
            started(&registry, "claude-api")
        ],
        [
            Ok(("claude", Provider::Claude)),
            Ok(("claude-api", Provider::Claude))
        ]
    );
}

#[test]
fn each_codex_instance_refreshes_its_own_app_server() {
    let mut registry = host_of(&[
        ("codex", "Codex", Provider::Codex),
        ("codex-api", "Codex (API)", Provider::Codex),
    ]);
    assert_eq!(
        set(&mut registry, "codex-api", gateway("key"), "set"),
        Vec::from([
            stored_note("codex-api"),
            Effect::Refresh(job(1, "codex-api", Some(gateway("key")))),
        ])
    );
    registry.admit(SessionId(1), id("codex"), 1);
    assert_eq!(
        (
            registry
                .start(&id("codex"), true, None, None)
                .map(|launch| launch.start),
            registry.relaunch(SessionId(1)),
            registry
                .start(&id("codex-api"), true, None, None)
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
fn an_added_instance_is_named_from_its_label_and_starts_its_cli() {
    let mut registry = registry(OwnSignIn::Unmanaged, &opened());
    let added = Instance::new(text("Claude (API)"), Provider::Claude);
    assert_eq!(
        registry.add_instance(Provider::Claude, " Claude (API) ", "add"),
        Vec::from([Effect::Save {
            roster: Roster(
                defaults(&catalog())
                    .0
                    .into_iter()
                    .chain([(id("claude-api"), added.clone())])
                    .collect()
            ),
            change: Change::Added(id("claude-api")),
            reply: "add",
        }])
    );
    assert_eq!(
        add(&mut registry, Provider::Claude, "Claude (API)", "add"),
        Vec::from([
            Effect::Log(Note::InstanceAdded {
                instance: id("claude-api"),
                provider: Provider::Claude,
            }),
            Effect::Start(Starting {
                id: 1,
                instance: id("claude-api"),
                settings: added,
            }),
        ])
    );
    assert_eq!(
        (
            availability(&registry, "claude-api"),
            set(&mut registry, "claude-api", gateway("t"), "set"),
        ),
        (
            Some(unavailable("starting the instance's CLI")),
            Vec::from([stored_note("claude-api")]),
        )
    );
    assert_eq!(
        registry.started(Started {
            id: 1,
            instance: id("claude-api"),
            launcher: ready("claude-api", OwnSignIn::Missing),
        }),
        Vec::from([Effect::Refresh(job(2, "claude-api", Some(gateway("t"))))])
    );
    let result =
        |registry: &Test| CommandResult::Instances(registry.instances());
    assert_eq!(
        registry.refreshed(refreshed(
            2,
            "claude-api",
            Ok(Listing {
                models: listed(),
                named: None,
            })
        )),
        Vec::from([
            Effect::Reply("add", result(&registry)),
            Effect::Reply("set", result(&registry)),
        ])
    );
    assert_eq!(
        [
            registry.add_instance(Provider::Codex, "Claude (API)", "taken"),
            registry.add_instance(Provider::Codex, "  ", "empty"),
        ],
        [
            Vec::from([Effect::Reply(
                "taken",
                rejected(Rejection::InvalidLabel {
                    reason: text("another instance is called Claude (API)")
                })
            )]),
            Vec::from([Effect::Reply(
                "empty",
                rejected(Rejection::InvalidLabel {
                    reason: text("an instance needs a label")
                })
            )]),
        ]
    );
    let _ = add(&mut registry, Provider::Claude, "claude api", "again");
    assert_eq!(
        registry
            .instances()
            .of(&id("claude-api-2"))
            .map(|it| &it.label),
        Some(&text("claude api"))
    );
}

#[test]
fn a_start_for_an_instance_since_removed_is_dropped() {
    let mut registry = registry(OwnSignIn::Unmanaged, &opened());
    let _ = add(&mut registry, Provider::Codex, "Codex (API)", "add");
    let removed =
        registry.remove_instance(id("codex-api"), &BTreeSet::new(), "remove");
    let result = CommandResult::Instances(claude(Availability::Available {
        auth: Auth::Unmanaged,
        models: listed(),
    }));
    assert_eq!(
        saving(&mut registry, removed),
        Vec::from([
            Effect::Log(Note::InstanceRemoved {
                instance: id("codex-api")
            }),
            Effect::Reply("add", result.clone()),
            Effect::Reply("remove", result),
        ])
    );
    assert_eq!(
        registry.started(Started {
            id: 1,
            instance: id("codex-api"),
            launcher: ready("codex-api", OwnSignIn::Unmanaged),
        }),
        Vec::new()
    );
    assert_eq!(registry.instances().of(&id("codex-api")), None);
}

#[test]
fn a_rename_keeps_the_numberand_takes_a_label_no_other_instance_has() {
    let mut registry = registry(OwnSignIn::Unmanaged, &opened());
    let renamed = registry.rename_instance(id("claude"), "Claude (work)", "r");
    let seen = saving(&mut registry, renamed);
    let instances = registry.instances();
    assert_eq!(
        seen,
        Vec::from([
            Effect::Log(Note::InstanceRenamed {
                instance: id("claude")
            }),
            Effect::Reply("r", CommandResult::Instances(instances.clone())),
        ])
    );
    assert_eq!(
        instances.of(&id("claude")).map(|it| &it.label),
        Some(&text("Claude (work)"))
    );
    assert_eq!(
        [
            registry.rename_instance(id("claude"), "Codex", "taken"),
            registry.rename_instance(id("gemini"), "Gemini", "unknown"),
        ],
        [
            Vec::from([Effect::Reply(
                "taken",
                rejected(Rejection::InvalidLabel {
                    reason: text("another instance is called Codex")
                })
            )]),
            Vec::from([Effect::Reply(
                "unknown",
                rejected(Rejection::UnknownInstance)
            )]),
        ]
    );
    assert_eq!(
        registry
            .rename_instance(id("claude"), "Claude (work)", "same")
            .len(),
        1
    );
}

#[test]
fn an_instance_is_removed_only_while_no_session_runs_on_it() {
    let mut registry = registry(OwnSignIn::Unmanaged, &opened());
    registry.admit(SessionId(1), id("claude"), 1);
    registry.admit(SessionId(2), id("claude"), 2);
    registry.admit(SessionId(3), id("codex"), 3);
    assert_eq!(
        registry.remove_instance(
            id("claude"),
            &BTreeSet::from([SessionId(1), SessionId(3)]),
            "busy"
        ),
        Vec::from([Effect::Reply(
            "busy",
            rejected(Rejection::InstanceInUse { sessions: 1 })
        )])
    );
    let removed = registry.remove_instance(
        id("claude"),
        &BTreeSet::from([SessionId(3)]),
        "remove",
    );
    let _ = saving(&mut registry, removed);
    assert_eq!(
        (
            registry.instances().of(&id("claude")),
            registry.relaunch(SessionId(1)),
            registry.find(SessionId(2)),
        ),
        (
            None,
            Err(Rejection::InstanceUnavailable {
                reason: text("this host has no instance claude")
            }),
            Some(2),
        )
    );
    let _ = add(&mut registry, Provider::Claude, "Claude", "add");
    assert!(registry.instances().of(&id("claude-2")).is_some());
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
                instance: id("claude"),
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
fn an_numberis_the_label_s_letters_and_digits_joined_by_dashes() {
    assert_eq!(
        ["Claude (API)", "  Work -- laptop ", "Ünïcode", "Codex 2"].map(slug),
        [
            text("claude-api"),
            text("work-laptop"),
            text("n-code"),
            text("codex-2")
        ]
    );
    let mut registry = registry(OwnSignIn::Unmanaged, &opened());
    let _ = add(&mut registry, Provider::Codex, "–", "add");
    assert!(registry.instances().of(&id("codex-2")).is_some());
}

#[test]
fn credential_changes_coalesce_and_a_stale_refresh_is_dropped() {
    let mut registry = registry(OwnSignIn::Missing, &opened());
    assert_eq!(
        set(&mut registry, "claude", gateway("first"), "first"),
        Vec::from([
            stored_note("claude"),
            Effect::Refresh(job(1, "claude", Some(gateway("first"))))
        ])
    );
    assert_eq!(
        (
            set(&mut registry, "claude", gateway("middle"), "middle"),
            set(&mut registry, "claude", gateway("newest"), "newest"),
            registry.instances(),
        ),
        (
            Vec::from([stored_note("claude")]),
            Vec::from([stored_note("claude")]),
            claude(unavailable("refreshing the instance's models")),
        )
    );
    let offered = Listing {
        models: Models::none(),
        named: Some(model("gw")),
    };
    assert_eq!(
        registry.refreshed(refreshed(1, "claude", Ok(offered.clone()))),
        Vec::from([Effect::Refresh(job(3, "claude", Some(gateway("newest"))))])
    );
    let current = claude(Availability::Available {
        auth: stored_auth(),
        models: Models {
            choices: Vec::new(),
            default: Some(model("gw")),
        },
    });
    let result = CommandResult::Instances(current.clone());
    assert_eq!(
        registry.refreshed(refreshed(3, "claude", Ok(offered.clone()))),
        Vec::from([
            Effect::Reply("first", result.clone()),
            Effect::Reply("middle", result.clone()),
            Effect::Reply("newest", result),
        ])
    );
    assert_eq!(
        registry.refreshed(refreshed(2, "claude", Ok(offered))),
        Vec::new()
    );
    assert_eq!(registry.instances(), current);
}

#[test]
fn a_failed_refresh_leaves_the_instance_unavailable_until_the_next() {
    let mut registry = registry(OwnSignIn::Unmanaged, &opened());
    let _ = set(&mut registry, "claude", gateway("t"), "set");
    let failed = CommandResult::Instances(claude(unavailable("probe failed")));
    assert_eq!(
        registry.refreshed(refreshed(1, "claude", Err(text("probe failed")))),
        Vec::from([
            Effect::Log(Note::RefreshFailed {
                instance: id("claude"),
                reason: text("probe failed"),
            }),
            Effect::Reply("set", failed),
        ])
    );
    assert_eq!(
        registry.refreshed(refreshed(
            1,
            "claude",
            Ok(Listing {
                models: listed(),
                named: None,
            })
        )),
        Vec::new()
    );
    let cleared = registry.clear_credential(id("claude"), "clear");
    assert_eq!(
        cleared,
        Vec::from([Effect::Save {
            roster: defaults(&catalog()),
            change: Change::Credential(id("claude")),
            reply: "clear",
        }])
    );
    assert_eq!(
        saving(&mut registry, cleared),
        Vec::from([
            stored_note("claude"),
            Effect::Refresh(job(2, "claude", None))
        ])
    );
}

#[test]
fn a_refresh_that_leaves_no_sign_in_fails_after_the_credential_takes() {
    let mut registry = registry(OwnSignIn::Missing, &opened());
    let cleared = registry.clear_credential(id("claude"), "clear");
    assert_eq!(
        saving(&mut registry, cleared),
        Vec::from([
            stored_note("claude"),
            Effect::Refresh(Job {
                signs_in: Err(text("no credential configured")),
                ..job(1, "claude", None)
            }),
        ])
    );
}

#[test]
fn credentials_are_checked_and_an_unreadable_store_is_only_cleared() {
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
    assert_eq!(
        (
            registry.set_credential(id("claude"), bad, "bad"),
            registry.set_credential(id("gemini"), gateway("t"), "unknown"),
            registry.set_credential(id("claude"), gateway("t"), "set"),
            registry.add_instance(Provider::Claude, "API", "add"),
            registry.remove_instance(id("codex"), &BTreeSet::new(), "remove"),
            registry.clear_credential(id("claude"), "clear"),
            registry.moved_aside(id("claude"), "clear", Err(text("busy"))),
            registry.saved(
                Roster::default(),
                Change::Credential(id("claude")),
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
            Vec::from([Effect::Reply(
                "unknown",
                rejected(Rejection::UnknownInstance)
            )]),
            not_saved("set", "sealed"),
            not_saved("add", "sealed"),
            not_saved("remove", "sealed"),
            Vec::from([Effect::MoveAside {
                instance: id("claude"),
                reply: "clear",
            }]),
            not_saved("clear", "busy"),
            not_saved("save", "disk full"),
        )
    );
    assert_eq!(
        registry.moved_aside(id("claude"), "clear", Ok(())),
        Vec::from([Effect::Save {
            roster: defaults(&catalog()),
            change: Change::Credential(id("claude")),
            reply: "clear",
        }])
    );
    let mut with_gateway = defaults(&catalog());
    if let Some(instance) = with_gateway.0.get_mut(&id("claude")) {
        instance.credential = Some(gateway("t"));
    }
    assert_eq!(
        registry.set_credential(id("claude"), gateway("t"), "set"),
        Vec::from([Effect::Save {
            roster: with_gateway,
            change: Change::Credential(id("claude")),
            reply: "set",
        }])
    );
}

#[test]
fn an_instance_the_host_cannot_run_answers_a_credential_change_at_once() {
    let mut registry = registry(OwnSignIn::Unmanaged, &opened());
    assert_eq!(
        registry.saved(
            defaults(&catalog()),
            Change::Credential(id("codex")),
            "clear",
            Ok(())
        ),
        Vec::from([
            stored_note("codex"),
            Effect::Reply(
                "clear",
                CommandResult::Instances(claude(Availability::Available {
                    auth: Auth::Unmanaged,
                    models: listed(),
                }))
            ),
        ])
    );
    assert_eq!(
        registry.start(&id("codex"), true, None, None),
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
