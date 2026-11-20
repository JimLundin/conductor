//! The credentials screen: what each field takes, what a sign-in forgets
//! when it runs again, and what the host's answers say.

use proto::{CommandResult, SignInStep};

use super::{
    HOST, answer, claude, claude_forms, id, paired, pane_of, roster, run,
    sign_in, sign_in_of, signed_in, signing, text, two_claudes,
};
use crate::{CredentialIntent, Intent, Place, State};

#[test]
fn each_field_takes_its_kind_of_input_and_an_empty_form_doesnt_save() {
    let (shown, _) = run(
        paired(),
        Vec::from([
            super::connection(super::HOST, client::link::LinkState::Live, None),
            roster(two_claudes()),
            Intent::Show(Place::Credentials),
        ]),
    );
    let fields = pane_of(&shown).map(|pane| {
        pane.add
            .into_iter()
            .flat_map(|forms| forms.forms)
            .flat_map(|form| form.fields)
            .map(|field| (field.field, field.input, field.autocomplete))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        (fields, claude_forms(&shown).map(|forms| forms.0)),
        (
            Some(Vec::from([
                (text("base_url"), "url", "off"),
                (text("token"), "password", "new-password"),
                (text("name"), "text", "off"),
                (text("base_url"), "url", "off"),
                (text("model"), "text", "off"),
                (text("key"), "password", "new-password"),
            ])),
            Some(true),
        )
    );
}

#[test]
fn only_a_sign_in_takes_accepted_for_an_answer() {
    let (removing, effects) = run(
        paired(),
        Vec::from([
            super::connection(HOST, client::link::LinkState::Live, None),
            roster(two_claudes()),
            Intent::Show(Place::Credentials),
            Intent::Credential(
                HOST,
                CredentialIntent::Remove(id("https://gw.example")),
            ),
        ]),
    );
    let (answered, _) = run(
        removing,
        Vec::from([answer(&effects, Ok(CommandResult::Accepted))]),
    );
    assert_eq!(
        pane_of(&answered).and_then(|pane| pane
            .instances
            .into_iter()
            .find(|instance| instance.instance == id("https://gw.example"))
            .map(|instance| instance.outcome.map(|hint| hint.text))),
        Some(Some(text("desk gave an unexpected answer.")))
    );
}

#[test]
fn a_removal_says_so_once_the_instance_has_left_the_list() {
    let api = || id("https://gw.example");
    let (removing, effects) = run(
        paired(),
        Vec::from([
            roster(two_claudes()),
            Intent::Show(Place::Credentials),
            Intent::Credential(HOST, CredentialIntent::Remove(api())),
        ]),
    );
    let (answered, _) = run(
        removing,
        Vec::from([answer(
            &effects,
            Ok(CommandResult::Instances(claude(signed_in()))),
        )]),
    );
    let (gone, _) =
        run(answered.clone(), Vec::from([roster(claude(signed_in()))]));
    let removed = |state: &State| {
        pane_of(state)
            .and_then(|pane| pane.removed)
            .map(|hint| hint.text)
    };
    assert_eq!(
        (removed(&answered), removed(&gone)),
        (None, Some(text("Removed from desk.")))
    );
}

#[test]
fn starting_or_cancelling_a_sign_in_forgets_the_code_typed_and_copied() {
    let open = signing(Some(SignInStep::Open {
        url: text("https://sign.example/device"),
        code: Some(text("ABCD-12345")),
        paste: true,
    }));
    let (typed, _) = run(
        open,
        Vec::from([
            sign_in(CredentialIntent::TypeSignInCode(
                id("claude"),
                text("abc"),
            )),
            sign_in(CredentialIntent::CopySignInCode(
                id("claude"),
                text("ABCD-12345"),
            )),
        ]),
    );
    let after = |intent| run(typed.clone(), Vec::from([sign_in(intent)])).0;
    let kept = |state: &State| {
        sign_in_of(state)
            .and_then(|view| view.paste.zip(view.code))
            .map(|(paste, code)| (paste.value, code.copy))
    };
    assert_eq!(
        [
            kept(&typed),
            kept(&after(CredentialIntent::StartSignIn(id("claude")))),
            kept(&after(CredentialIntent::CancelSignIn(id("claude")))),
        ],
        [
            Some((text("abc"), text("Copied"))),
            Some((String::new(), text("Copy"))),
            Some((String::new(), text("Copy"))),
        ]
    );
}

#[test]
fn a_credential_reply_cannot_fill_the_outcome_of_a_new_visit() {
    let remove = Intent::Credential(
        HOST,
        CredentialIntent::Remove(id("https://gw.example")),
    );
    let (removing, effects) = run(
        paired(),
        Vec::from([
            super::connection(HOST, client::link::LinkState::Live, None),
            roster(two_claudes()),
            Intent::Show(Place::Credentials),
            remove.clone(),
        ]),
    );
    let (returned, _) = run(
        removing,
        Vec::from([
            Intent::Show(Place::Sessions),
            Intent::Show(Place::Credentials),
            remove,
        ]),
    );
    let (late, ignored) = run(
        returned.clone(),
        Vec::from([answer(&effects, Err(text("old failure")))]),
    );
    assert_eq!(pane_of(&late), pane_of(&returned));
    assert_eq!(ignored, []);
}
