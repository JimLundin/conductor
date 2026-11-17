use std::collections::BTreeMap;

use agents::Note;
use proto::{
    Auth, Availability, ClaudeCredential, CommandResult, Credential,
    Credentials, Effort, ModelChoice, ModelId, Models, Provider, Providers,
    Rejection, SessionId,
};

use super::{
    Effect, Job, Launch, Launcher, Listing, Offer, OwnSignIn, Refreshed,
    Registry, Stored,
};
use crate::session::Choice;

type Test = Registry<&'static str, u8, &'static str>;
type Seen = Vec<Effect<&'static str, &'static str>>;

const GATEWAY: &str = "https://gateway.example";

fn text(value: &str) -> String {
    String::from(value)
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

fn ready(own: OwnSignIn) -> Launcher<&'static str> {
    Launcher::Ready {
        start: "claude",
        own,
        offer: Offer {
            listed: listed(),
            named: None,
            chosen: None,
        },
    }
}

fn registry(own: OwnSignIn, stored: Stored) -> Test {
    Registry::new(
        BTreeMap::from([
            (Provider::Claude, ready(own)),
            (Provider::Codex, Launcher::Unavailable(text("no codex"))),
        ]),
        stored,
    )
}

fn opened() -> Stored {
    Stored::Opened(Credentials::none())
}

fn gateway(token: &str) -> Credential {
    Credential::Claude(ClaudeCredential::Gateway {
        base_url: text(GATEWAY),
        token: text(token),
    })
}

fn unavailable(reason: &str) -> Availability {
    Availability::Unavailable {
        reason: text(reason),
    }
}

fn claude(availability: Availability) -> Providers {
    Providers(BTreeMap::from([
        (Provider::Claude, availability),
        (Provider::Codex, unavailable("no codex")),
    ]))
}

fn rejected(rejection: Rejection) -> CommandResult {
    CommandResult::Rejected(rejection)
}

fn stored_note() -> Effect<&'static str, &'static str> {
    Effect::Log(Note::info(text("stored a credential change for Claude")))
}

fn job(id: u64, credentials: Credentials) -> Job<&'static str> {
    Job {
        id,
        provider: Provider::Claude,
        start: "claude",
        credentials,
        signs_in: Ok(()),
    }
}

fn refreshed(id: u64, listing: Result<Listing, String>) -> Refreshed {
    Refreshed {
        id,
        provider: Provider::Claude,
        listing,
    }
}

/// Sets `credential` and saves it, as the shell would.
fn set(
    registry: &mut Test,
    credential: Credential,
    reply: &'static str,
) -> Seen {
    let saved = registry.set_credential(credential, reply);
    let Some(Effect::Save {
        provider,
        credentials,
        reply,
    }) = saved.into_iter().next()
    else {
        return Vec::new();
    };
    registry.saved(provider, credentials, reply, Ok(()))
}

#[test]
fn a_start_is_refused_until_directory_provider_and_model_all_hold() {
    let registry = registry(OwnSignIn::Unmanaged, opened());
    assert_eq!(
        [
            registry.start(Provider::Claude, false, None, None),
            registry.start(Provider::Codex, true, None, None),
            registry.start(Provider::Claude, true, Some(model("tiny")), None),
            registry.start(Provider::Claude, true, None, Some(Effort::Max)),
        ],
        [
            Err(Rejection::NoSuchDirectory),
            Err(Rejection::ProviderUnavailable {
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
        registry.start(Provider::Claude, true, None, Some(Effort::High)),
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
fn a_provider_without_a_sign_in_or_a_readable_store_is_unavailable() {
    let missing = registry(OwnSignIn::Missing, opened());
    let unreadable =
        registry(OwnSignIn::SignedIn, Stored::Unreadable(text("sealed")));
    let signed_in = registry(OwnSignIn::SignedIn, opened());
    assert_eq!(
        [
            missing.providers(),
            unreadable.providers(),
            signed_in.providers()
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
        missing.start(Provider::Claude, true, None, None),
        Err(Rejection::ProviderUnavailable {
            reason: text("no credential configured")
        })
    );
    assert_eq!(
        Registry::<&str, u8, &str>::new(BTreeMap::new(), opened()).start(
            Provider::Claude,
            true,
            None,
            None
        ),
        Err(Rejection::ProviderUnavailable {
            reason: text("this host doesn't run Claude")
        })
    );
}

#[test]
fn a_session_is_found_and_continued_only_while_its_provider_runs() {
    let mut registry = registry(OwnSignIn::Unmanaged, opened());
    registry.admit(SessionId(1), Provider::Claude, 7);
    registry.admit(SessionId(2), Provider::Codex, 8);
    assert_eq!(
        (
            registry.find(SessionId(1)),
            registry.find(SessionId(3)),
            registry.relaunch(SessionId(1)),
            registry.relaunch(SessionId(2)),
            registry.relaunch(SessionId(3)),
            registry.launcher(Provider::Claude),
            registry.launcher(Provider::Codex),
        ),
        (
            Some(7),
            None,
            Ok((7, listed())),
            Err(Rejection::ProviderUnavailable {
                reason: text("no codex")
            }),
            Err(Rejection::UnknownSession),
            Ok("claude"),
            Err(text("no codex")),
        )
    );
    let _ = set(&mut registry, gateway("t"), "set");
    assert_eq!(
        registry.relaunch(SessionId(1)),
        Err(Rejection::ProviderUnavailable {
            reason: text("refreshing provider models")
        })
    );
}

#[test]
fn credential_changes_coalesce_and_a_stale_refresh_is_dropped() {
    let mut registry = registry(OwnSignIn::Missing, opened());
    let first = Credentials::none().set(gateway("first"));
    let newest = Credentials::none().set(gateway("newest"));
    assert_eq!(
        set(&mut registry, gateway("first"), "first"),
        Vec::from([stored_note(), Effect::Refresh(job(1, first))])
    );
    assert_eq!(
        (
            set(&mut registry, gateway("middle"), "middle"),
            set(&mut registry, gateway("newest"), "newest"),
            registry.providers(),
        ),
        (
            Vec::from([stored_note()]),
            Vec::from([stored_note()]),
            claude(unavailable("refreshing provider models")),
        )
    );
    let offered = Listing {
        models: Models::none(),
        named: Some(model("gw")),
    };
    assert_eq!(
        registry.refreshed(refreshed(1, Ok(offered.clone()))),
        Vec::from([Effect::Refresh(job(3, newest))])
    );
    let result = CommandResult::Providers(claude(Availability::Available {
        auth: Auth::Gateway {
            base_url: text(GATEWAY),
        },
        models: Models {
            choices: Vec::new(),
            default: Some(model("gw")),
        },
    }));
    assert_eq!(
        registry.refreshed(refreshed(3, Ok(offered.clone()))),
        Vec::from([
            Effect::Reply("first", result.clone()),
            Effect::Reply("middle", result.clone()),
            Effect::Reply("newest", result.clone()),
        ])
    );
    assert_eq!(registry.refreshed(refreshed(2, Ok(offered))), Vec::new());
    assert_eq!(
        registry.providers(),
        match result {
            CommandResult::Providers(providers) => providers,
            _ => Providers(BTreeMap::new()),
        }
    );
}

#[test]
fn a_failed_refresh_leaves_the_provider_unavailable_until_the_next() {
    let mut registry = registry(OwnSignIn::Unmanaged, opened());
    let _ = set(&mut registry, gateway("t"), "set");
    let failed = CommandResult::Providers(claude(unavailable("probe failed")));
    assert_eq!(
        registry.refreshed(refreshed(1, Err(text("probe failed")))),
        Vec::from([
            Effect::Log(Note::warn(text(
                "provider refresh failed for Claude: probe failed"
            ))),
            Effect::Reply("set", failed),
        ])
    );
    assert_eq!(
        registry.refreshed(refreshed(
            1,
            Ok(Listing {
                models: listed(),
                named: None,
            })
        )),
        Vec::new()
    );
    let cleared = registry.clear_credential(Provider::Claude, "clear");
    assert_eq!(
        cleared,
        Vec::from([Effect::Save {
            provider: Provider::Claude,
            credentials: Credentials::none(),
            reply: "clear",
        }])
    );
    assert_eq!(
        registry.saved(Provider::Claude, Credentials::none(), "clear", Ok(())),
        Vec::from([
            stored_note(),
            Effect::Refresh(job(2, Credentials::none()))
        ])
    );
}

#[test]
fn a_refresh_that_leaves_no_sign_in_fails_after_the_credentials_take() {
    let mut registry = registry(OwnSignIn::Missing, opened());
    assert_eq!(
        registry.clear_credential(Provider::Claude, "clear"),
        Vec::from([Effect::Save {
            provider: Provider::Claude,
            credentials: Credentials::none(),
            reply: "clear",
        }])
    );
    assert_eq!(
        registry.saved(Provider::Claude, Credentials::none(), "clear", Ok(())),
        Vec::from([
            stored_note(),
            Effect::Refresh(Job {
                signs_in: Err(text("no credential configured")),
                ..job(1, Credentials::none())
            }),
        ])
    );
}

#[test]
fn credentials_are_checked_and_an_unreadable_store_is_only_cleared() {
    let mut registry =
        registry(OwnSignIn::Unmanaged, Stored::Unreadable(text("sealed")));
    let bad = Credential::Claude(ClaudeCredential::Gateway {
        base_url: text("ftp://x"),
        token: text("t"),
    });
    assert_eq!(
        (
            registry.set_credential(bad, "bad"),
            registry.set_credential(gateway("t"), "set"),
            registry.clear_credential(Provider::Claude, "clear"),
            registry.moved_aside(Provider::Claude, "clear", Err(text("busy"))),
            registry.saved(
                Provider::Claude,
                Credentials::none(),
                "save",
                Err(text("disk full"))
            ),
        ),
        (
            Vec::from([Effect::Reply(
                "bad",
                rejected(Rejection::InvalidCredential {
                    reason: text("the base URL must be http or https")
                })
            )]),
            Vec::from([Effect::Reply(
                "set",
                rejected(Rejection::CredentialNotSaved {
                    reason: text("sealed")
                })
            )]),
            Vec::from([Effect::MoveAside {
                provider: Provider::Claude,
                reply: "clear",
            }]),
            Vec::from([Effect::Reply(
                "clear",
                rejected(Rejection::CredentialNotSaved {
                    reason: text("busy")
                })
            )]),
            Vec::from([Effect::Reply(
                "save",
                rejected(Rejection::CredentialNotSaved {
                    reason: text("disk full")
                })
            )]),
        )
    );
    assert_eq!(
        registry.moved_aside(Provider::Claude, "clear", Ok(())),
        Vec::from([Effect::Save {
            provider: Provider::Claude,
            credentials: Credentials::none(),
            reply: "clear",
        }])
    );
    assert_eq!(
        registry.set_credential(gateway("t"), "set"),
        Vec::from([Effect::Save {
            provider: Provider::Claude,
            credentials: Credentials::none().set(gateway("t")),
            reply: "set",
        }])
    );
}

#[test]
fn a_provider_the_host_cannot_run_answers_a_credential_change_at_once() {
    let mut registry = registry(OwnSignIn::Unmanaged, opened());
    let codex = Credentials::none();
    assert_eq!(
        registry.saved(Provider::Codex, codex, "clear", Ok(())),
        Vec::from([
            Effect::Log(Note::info(text(
                "stored a credential change for Codex"
            ))),
            Effect::Reply(
                "clear",
                CommandResult::Providers(claude(Availability::Available {
                    auth: Auth::Unmanaged,
                    models: listed(),
                }))
            ),
        ])
    );
    assert_eq!(
        registry.start(Provider::Codex, true, None, None),
        Err(Rejection::ProviderUnavailable {
            reason: text("no codex")
        })
    );
}

#[test]
fn the_default_is_the_credentials_model_then_the_chosen_then_the_clis() {
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
    assert_eq!(
        [
            Stored::Unreadable(text("sealed")).credentials(),
            Stored::Opened(Credentials::none().set(gateway("t"))).credentials(),
        ],
        [Credentials::none(), Credentials::none().set(gateway("t"))]
    );
}
