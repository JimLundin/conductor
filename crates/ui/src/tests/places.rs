//! The pairing form, which place each control marks current, and what a
//! place's drafts become once another place is shown.

use proto::{Provider, TicketError};

use super::{
    HOST, SESSION, claude, claude_forms, composer, joined, opened, paired,
    roster, run, screen, signed_in, text, ticket,
};
use crate::{
    CredentialIntent, Detail, Intent, Place, Screen, SessionIntent, State, view,
};

fn pair(state: &State) -> Option<(String, bool, Option<String>)> {
    match view(state) {
        Screen::Pair(form) => {
            Some((form.text, form.connect_disabled, form.error))
        }
        Screen::Hosts(_) => None,
    }
}

#[test]
fn the_pairing_form_keeps_what_is_typed_and_says_why_a_link_is_refused() {
    let (typed, _) = run(
        State::default(),
        Vec::from([Intent::TypeJoin(text("conductor://x"))]),
    );
    let (refused, _) = run(
        typed.clone(),
        Vec::from([Intent::Join(Err(TicketError::Truncated))]),
    );
    assert_eq!(
        (pair(&typed), pair(&refused)),
        (
            Some((text("conductor://x"), false, None)),
            Some((
                text("conductor://x"),
                false,
                Some(text("That join link is not valid: ticket is truncated")),
            )),
        )
    );
}

#[test]
fn a_paired_link_leaves_nothing_typed_behind() {
    let refused = || joined(Err(TicketError::Truncated));
    let (typed_then_paired, _) = run(
        refused(),
        Vec::from([
            Intent::TypeJoin(ticket().to_text()),
            Intent::Join(Ok(ticket())),
        ]),
    );
    let (paired_now, _) =
        run(refused(), Vec::from([Intent::Join(Ok(ticket()))]));
    assert_eq!(typed_then_paired, paired_now);
}

#[test]
fn each_control_is_current_only_on_its_own_place() {
    let current = |place| {
        let (shown, _) = run(paired(), Vec::from([Intent::Show(place)]));
        screen(&shown).map(|host| {
            (
                host.add_current,
                host.credentials_current,
                host.projects_current,
                host.list.new_current,
            )
        })
    };
    assert_eq!(
        [
            current(Place::Sessions),
            current(Place::AddHost),
            current(Place::Credentials),
            current(Place::Projects),
            current(Place::NewSession),
        ],
        [
            Some((false, false, false, false)),
            Some((true, false, false, false)),
            Some((false, true, false, false)),
            Some((false, false, true, false)),
            Some((false, false, false, true)),
        ]
    );
}

/// Shows `place`, then another, then `place` again.
fn back_to(state: State, place: Place) -> State {
    run(
        state,
        Vec::from([Intent::Show(Place::Sessions), Intent::Show(place)]),
    )
    .0
}

#[test]
fn leaving_a_place_drops_what_was_typed_there() {
    let (pairing, _) = run(
        paired(),
        Vec::from([
            Intent::Show(Place::AddHost),
            Intent::TypeJoin(text("conductor://x")),
        ]),
    );
    let (filling, _) = run(
        paired(),
        Vec::from([
            super::connection(super::HOST, client::link::LinkState::Live, None),
            roster(claude(signed_in())),
            Intent::Show(Place::Credentials),
            Intent::Credential(
                HOST,
                CredentialIntent::Type {
                    provider: Provider::Claude,
                    kind: text("gateway"),
                    field: text("base_url"),
                    text: text("https://gw.example"),
                },
            ),
        ]),
    );
    let (writing, _) = opened(Vec::from([Intent::Session(
        SessionIntent::TypeMessage(text("hi")),
    )]));
    let join_text = |state: &State| {
        screen(state).and_then(|host| match host.detail {
            Detail::Pair(form) => Some(form.text),
            _ => None,
        })
    };
    assert_eq!(
        (
            join_text(&pairing),
            join_text(&back_to(pairing, Place::AddHost)),
            claude_forms(&filling).map(|forms| forms.2),
            claude_forms(&back_to(filling, Place::Credentials))
                .map(|forms| forms.2),
            composer(&writing).map(|shown| shown.0),
            composer(&back_to(writing, Place::Session(HOST, SESSION)))
                .map(|shown| shown.0),
        ),
        (
            Some(text("conductor://x")),
            Some(String::new()),
            Some(Vec::from([text("https://gw.example"), String::new()])),
            Some(Vec::from([String::new(), String::new()])),
            Some(text("hi")),
            Some(String::new()),
        )
    );
}

#[test]
fn a_late_start_cannot_open_a_session_over_a_new_form() {
    use super::{answer, form};
    use crate::FormIntent;
    use proto::{CommandResult, Rejection, SessionId};

    let (starting, effects) = run(
        paired(),
        Vec::from([
            super::connection(HOST, client::link::LinkState::Live, None),
            roster(claude(signed_in())),
            Intent::Show(Place::NewSession),
            Intent::Form(FormIntent::TypeDirectory(text("/old"))),
            Intent::Form(FormIntent::Start),
        ]),
    );
    let (returned, _) = run(
        back_to(starting, Place::NewSession),
        Vec::from([Intent::Form(FormIntent::TypeDirectory(text("/new")))]),
    );
    for result in [
        CommandResult::Started {
            session: SessionId(9),
        },
        CommandResult::Rejected(Rejection::NoSuchDirectory),
    ] {
        let (late, ignored) =
            run(returned.clone(), Vec::from([answer(&effects, Ok(result))]));
        assert_eq!(form(&late), form(&returned));
        assert_eq!(ignored, []);
    }
}
