//! The new-session form's picks: what each marks, and what picking a host,
//! an instance or a model puts back.

use alloc::collections::BTreeMap;

use client::link::{LinkState, Status, Update};
use proto::{Availability, Instances};

use super::projects::desk_home;
use super::{
    HOST, catalog, form, id, instance, paired, run, text, two_claudes,
};
use crate::{FormIntent, Intent, NewSessionForm, Place, State};

/// desk, live with its home, two Claude instances and an unavailable
/// Codex, on the new-session form after `picks`.
fn picked(picks: Vec<FormIntent>) -> Option<NewSessionForm> {
    let unavailable = instance(Availability::Unavailable {
        reason: text("no codex"),
    });
    let instances = two_claudes()
        .0
        .into_iter()
        .chain([(id("codex"), unavailable)])
        .collect();
    let shown = Vec::from([
        Intent::Link(
            HOST,
            Update::Status(Status {
                state: LinkState::Live,
                host: Some(text("desk")),
                instances: Some(Instances(instances)),
                catalog: catalog(),
                sign_ins: BTreeMap::new(),
                home: Some(desk_home()),
                error: None,
            }),
        ),
        Intent::Show(Place::NewSession),
    ]);
    let state: State = run(
        paired(),
        shown
            .into_iter()
            .chain(picks.into_iter().map(Intent::Form))
            .collect(),
    )
    .0;
    form(&state)
}

/// Which instance, model, effort and project each form has picked.
type Picks = (Vec<bool>, Vec<bool>, Vec<bool>, Vec<bool>);

fn picks(shown: NewSessionForm) -> Picks {
    let selected = |choices: Vec<crate::Choice>| {
        choices
            .into_iter()
            .map(|choice| choice.selected)
            .collect::<Vec<bool>>()
    };
    (
        shown
            .instances
            .into_iter()
            .map(|choice| choice.checked)
            .collect(),
        selected(shown.models),
        selected(shown.efforts),
        selected(shown.projects.unwrap_or_default()),
    )
}

#[test]
fn each_instance_says_whether_the_host_can_run_it() {
    assert_eq!(
        picked(Vec::new()).map(|shown| shown
            .instances
            .into_iter()
            .map(|choice| (choice.label, choice.state))
            .collect::<Vec<_>>()),
        Some(Vec::from([
            (text("Claude Code"), "available"),
            (text("https://gw.example"), "available"),
            (text("Codex"), "unavailable"),
        ]))
    );
}

#[test]
fn each_pick_marks_its_choice_and_an_earlier_pick_goes_back_to_default() {
    let model_and_effort = || {
        Vec::from([
            FormIntent::PickModel(text("opus")),
            FormIntent::PickEffort(text("high")),
            FormIntent::PickProject(text("alpha")),
        ])
    };
    let then = |pick: FormIntent| {
        picked(model_and_effort().into_iter().chain([pick]).collect())
            .map(picks)
    };
    let defaults = (
        Vec::from([true, false, false]),
        Vec::from([true, false]),
        Vec::from([true, false, false]),
        Vec::from([true, false, false, false, false, false]),
    );
    assert_eq!(
        [
            picked(model_and_effort()).map(picks),
            then(FormIntent::PickInstance(id("claude"))),
            then(FormIntent::PickHost(HOST.key())),
            picked(Vec::from([
                FormIntent::PickInstance(id("https://gw.example")),
                FormIntent::PickHost(HOST.key()),
            ]))
            .map(picks),
        ],
        [
            Some((
                Vec::from([true, false, false]),
                Vec::from([false, true]),
                Vec::from([false, false, true]),
                Vec::from([false, true, false, false, false, false]),
            )),
            Some((
                Vec::from([true, false, false]),
                Vec::from([true, false]),
                Vec::from([true, false, false]),
                Vec::from([false, true, false, false, false, false]),
            )),
            Some(defaults.clone()),
            Some(defaults),
        ]
    );
}

#[test]
fn a_model_keeps_only_an_effort_it_offers() {
    assert_eq!(
        picked(Vec::from([
            FormIntent::PickEffort(text("medium")),
            FormIntent::PickModel(text("opus")),
        ]))
        .map(|shown| picks(shown).2),
        Some(Vec::from([true, false, false]))
    );
}

#[test]
fn a_listed_project_is_enough_to_start_and_an_unlisted_one_is_ignored() {
    let place = |project: &str| {
        picked(Vec::from([FormIntent::PickProject(text(project))]))
            .map(|shown| (shown.directory_shown, shown.start_disabled))
    };
    assert_eq!(
        [place("alpha"), place("zeta")],
        [Some((false, false)), Some((true, true))]
    );
}
