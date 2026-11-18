//! Several hosts at once: each its own group, link and commands, and the
//! list of them that local storage keeps.

use alloc::collections::BTreeMap;

use client::link::{LinkState, Status, Update};
use proto::{
    AuthToken, ClientName, CommandBody, CommandResult, Event, EventKind,
    HostPath, HostSeq, Instances, Provider, SessionId, Ticket,
};

use super::{
    HOST, SESSION, catalog, claude, created, form, id, instance, joined,
    listing, paired, pane, run, screen, signed_in, text, ticket,
};
use crate::{
    Detail, Effect, FormIntent, Intent, Listing, Place, Reply, ReplyTo, Saved,
    Screen, SessionIntent, State, Stored, decode, encode, view,
};

const OTHER: crate::HostId = crate::HostId([7; 32]);
const THEIRS: SessionId = SessionId(3);

fn other() -> Ticket {
    Ticket {
        host: [7; 32],
        relay: text("https://relay.example/"),
        token: AuthToken::from_bytes([2; 16]),
    }
}

fn named(
    host: crate::HostId,
    name: &str,
    state: LinkState,
    instances: Instances,
) -> Intent {
    Intent::Link(
        host,
        Update::Status(Status {
            state,
            host: Some(text(name)),
            instances: Some(instances),
            catalog: catalog(),
            sign_ins: BTreeMap::new(),
            error: match state {
                LinkState::Live => None,
                LinkState::Disconnected
                | LinkState::Connecting
                | LinkState::Authenticating
                | LinkState::CatchingUp => Some(text("timed out")),
            },
        }),
    )
}

fn codex() -> Instances {
    Instances(BTreeMap::from([(
        id("codex"),
        instance("Codex", Provider::Codex, signed_in()),
    )]))
}

fn on(
    host: crate::HostId,
    seq: u64,
    session: SessionId,
    directory: &str,
) -> Intent {
    Intent::Link(
        host,
        Update::Event(Event {
            seq: HostSeq(seq),
            session,
            kind: EventKind::SessionCreated {
                instance: id("codex"),
                provider: Provider::Codex,
                directory: HostPath(text(directory)),
                by: ClientName(text("desk")),
                model: None,
                effort: None,
            },
        }),
    )
}

/// `desk` (this module's `HOST`, Claude) live with a session, and
/// `laptop` (`OTHER`, Codex) in `state` with one of its own.
fn two(state: LinkState) -> State {
    let (both, _) = run(
        paired(),
        Vec::from([
            named(HOST, "desk", LinkState::Live, claude(signed_in())),
            Intent::Link(
                HOST,
                Update::Event(Event {
                    seq: HostSeq(1),
                    session: SESSION,
                    kind: created(),
                }),
            ),
            Intent::Join(Ok(other())),
            named(OTHER, "laptop", state, codex()),
            on(OTHER, 1, THEIRS, "/home/me/notes"),
        ]),
    );
    both
}

fn groups(state: &State) -> Vec<(String, String, Option<String>, Vec<String>)> {
    screen(state).map_or_else(Vec::new, |shown| {
        shown
            .list
            .groups
            .into_iter()
            .map(|group| {
                (
                    group.status.host,
                    group.status.state,
                    group.status.error,
                    match group.listing {
                        Listing::Rows(rows) => {
                            rows.into_iter().map(|row| row.title).collect()
                        }
                        Listing::Empty(text) => Vec::from([text]),
                    },
                )
            })
            .collect()
    })
}

#[test]
fn each_host_lists_its_sessions_under_its_name_and_link_state() {
    let state = two(LinkState::Live);
    let (opened, _) = run(
        state.clone(),
        Vec::from([Intent::Show(Place::Session(OTHER, THEIRS))]),
    );
    assert_eq!(
        (
            groups(&state),
            screen(&state).map(|shown| shown.links.len()),
            pane(&opened).map(|shown| (shown.title, shown.subtitle)),
        ),
        (
            Vec::from([
                (
                    text("desk"),
                    text("Live"),
                    None,
                    Vec::from([text("conductor")])
                ),
                (
                    text("laptop"),
                    text("Live"),
                    None,
                    Vec::from([text("notes")])
                ),
            ]),
            Some(2),
            Some((text("notes"), text("/home/me/notes on laptop"))),
        )
    );
}

#[test]
fn a_command_goes_to_the_host_of_the_session_or_form_it_comes_from() {
    let (typed, effects) = run(
        two(LinkState::Live),
        Vec::from([
            Intent::Show(Place::Session(OTHER, THEIRS)),
            Intent::Link(
                OTHER,
                Update::Event(Event {
                    seq: HostSeq(2),
                    session: THEIRS,
                    kind: EventKind::SessionReady,
                }),
            ),
            Intent::Session(SessionIntent::TypeMessage(text("hi"))),
            Intent::Session(SessionIntent::SendMessage),
            Intent::Show(Place::Credentials),
            Intent::Credential(
                HOST,
                crate::CredentialIntent::StartSignIn(id("claude")),
            ),
        ]),
    );
    let started = Reply {
        host: OTHER,
        visit: 3,
        to: ReplyTo::Start,
    };
    let (answered, _) = run(
        typed,
        Vec::from([
            Intent::Show(Place::NewSession),
            Intent::Replied(
                started,
                Ok(CommandResult::Started {
                    session: SessionId(9),
                }),
            ),
        ]),
    );
    let targets = effects
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Send { host, body, reply } => {
                Some((host, body, reply.host))
            }
            Effect::SaveHosts(_)
            | Effect::Focus(_)
            | Effect::ShowEffort(_)
            | Effect::Copy(_) => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        (targets, screen(&answered).map(|shown| shown.shown)),
        (
            Vec::from([
                (
                    OTHER,
                    CommandBody::SendMessage {
                        session: THEIRS,
                        text: text("hi"),
                    },
                    OTHER
                ),
                (
                    HOST,
                    CommandBody::StartSignIn {
                        instance: id("claude"),
                    },
                    HOST
                ),
            ]),
            // The session started on laptop opens; it shows once its
            // events arrive, until then the placeholder.
            Some("list"),
        )
    );
}

#[test]
fn one_host_offline_leaves_the_other_live() {
    let state = two(LinkState::Connecting);
    let (left, _) =
        run(state.clone(), Vec::from([Intent::Show(Place::NewSession)]));
    let hosts = form(&left).map(|shown| {
        shown
            .hosts
            .into_iter()
            .map(|choice| (choice.label, choice.selected))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        (
            groups(&state)
                .into_iter()
                .map(|(name, link, error, _)| (name, link, error))
                .collect::<Vec<_>>(),
            hosts,
        ),
        (
            Vec::from([
                (text("desk"), text("Live"), None),
                (text("laptop"), text("Connecting"), Some(text("timed out"))),
            ]),
            Some(Vec::from([
                (text("desk"), true),
                (text("laptop (offline)"), false),
            ])),
        )
    );
}

#[test]
fn forgetting_a_host_drops_its_sessions_and_ignores_what_it_still_sends() {
    let (forgot, effects) = run(
        two(LinkState::Live),
        Vec::from([
            Intent::Show(Place::Session(OTHER, THEIRS)),
            Intent::Forget(OTHER),
            on(OTHER, 2, SessionId(4), "/home/me/late"),
            Intent::Replied(
                Reply {
                    host: OTHER,
                    visit: 1,
                    to: ReplyTo::Notice,
                },
                Err(text("gone")),
            ),
        ]),
    );
    let (last, _) = run(forgot.clone(), Vec::from([Intent::Forget(HOST)]));
    assert_eq!(
        (
            groups(&forgot)
                .into_iter()
                .map(|(name, ..)| name)
                .collect::<Vec<_>>(),
            screen(&forgot).map(|shown| shown.shown),
            effects.last().cloned(),
            matches!(view(&last), Screen::Pair(_)),
        ),
        (
            Vec::from([text("desk")]),
            Some("list"),
            Some(Effect::SaveHosts(Vec::from([Saved {
                ticket: ticket(),
                name: Some(text("desk")),
            }]))),
            true,
        )
    );
}

#[test]
fn pairing_a_host_again_takes_its_new_ticket_and_keeps_its_sessions() {
    let renewed = Ticket {
        token: AuthToken::from_bytes([3; 16]),
        ..other()
    };
    let (added, _) = run(
        two(LinkState::Live),
        Vec::from([Intent::Show(Place::AddHost)]),
    );
    let (again, saved) = run(
        added.clone(),
        Vec::from([
            Intent::TypeJoin(renewed.to_text()),
            Intent::Join(Ok(renewed.clone())),
        ]),
    );
    let keys = |state: &State| {
        screen(state).map_or_else(Vec::new, |shown| {
            shown
                .links
                .into_iter()
                .map(|dial| dial.key)
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(
        (
            screen(&added).map(|shown| matches!(shown.detail, Detail::Pair(_))),
            keys(&again),
            groups(&again)
                .into_iter()
                .map(|(name, _, _, rows)| (name, rows))
                .collect::<Vec<_>>(),
            saved,
            screen(&again).map(|shown| shown.shown),
        ),
        (
            Some(true),
            Vec::from([renewed.to_text(), ticket().to_text()]),
            Vec::from([
                (text("desk"), Vec::from([text("conductor")])),
                (text("laptop"), Vec::from([text("notes")])),
            ]),
            Vec::from([Effect::SaveHosts(Vec::from([
                Saved {
                    ticket: renewed,
                    name: Some(text("laptop")),
                },
                Saved {
                    ticket: ticket(),
                    name: Some(text("desk")),
                },
            ]))]),
            Some("list"),
        )
    );
}

#[test]
fn a_new_session_picks_a_host_then_one_of_its_instances() {
    let state = two(LinkState::Live);
    let (theirs, _) = run(
        state,
        Vec::from([
            Intent::Show(Place::Session(OTHER, THEIRS)),
            Intent::Show(Place::NewSession),
        ]),
    );
    let (mine, started) = run(
        theirs.clone(),
        Vec::from([
            Intent::Form(FormIntent::PickHost(HOST.key())),
            Intent::Form(FormIntent::TypeDirectory(text("/src/x"))),
            Intent::Form(FormIntent::Start),
        ]),
    );
    let shown = |state: &State| {
        form(state).map(|shown| {
            (
                shown
                    .hosts
                    .into_iter()
                    .filter(|choice| choice.selected)
                    .map(|choice| choice.label)
                    .collect::<Vec<_>>(),
                shown
                    .instances
                    .into_iter()
                    .map(|choice| choice.label)
                    .collect::<Vec<_>>(),
            )
        })
    };
    assert_eq!(
        (shown(&theirs), shown(&mine), started),
        (
            Some((Vec::from([text("laptop")]), Vec::from([text("Codex")]))),
            Some((Vec::from([text("desk")]), Vec::from([text("Claude Code")]))),
            Vec::from([Effect::Send {
                host: HOST,
                body: CommandBody::StartSession {
                    instance: id("claude"),
                    directory: HostPath(text("/src/x")),
                    model: None,
                    effort: None,
                },
                reply: Reply {
                    host: HOST,
                    visit: 2,
                    to: ReplyTo::Start,
                },
            }]),
        )
    );
}

#[test]
fn the_single_saved_ticket_moves_into_the_list() {
    let single = Stored {
        hosts: None,
        ticket: Some(ticket().to_text()),
    };
    let migrated = State::new(crate::Opening {
        stored: single,
        joined: None,
    });
    let both = Vec::from([
        Saved {
            ticket: other(),
            name: Some(text("laptop")),
        },
        Saved {
            ticket: ticket(),
            name: None,
        },
    ]);
    let listed = Stored {
        hosts: Some(encode(&both)),
        ticket: Some(ticket().to_text()),
    };
    let broken = Stored {
        hosts: Some(text(r#"{"00": {"ticket": "not a ticket"}}"#)),
        ticket: None,
    };
    let joined_too = joined(Ok(other()));
    assert_eq!(
        (
            migrated.saved(),
            screen(&migrated).map(|shown| shown.links.len()),
            decode(&listed),
            encode(&both)
                .starts_with(&format!("{{\"{}\":{{\"ticket\":", OTHER.key())),
            decode(&broken),
            decode(&Stored::default()),
            joined_too.saved().len(),
            listing(&State::new(crate::Opening {
                stored: listed,
                joined: Some(Ok(other())),
            })),
        ),
        (
            Vec::from([Saved {
                ticket: ticket(),
                name: None,
            }]),
            Some(1),
            both,
            true,
            Vec::new(),
            Vec::new(),
            1,
            Some(Listing::Empty(text("No sessions on host 09090909 yet."))),
        )
    );
}
