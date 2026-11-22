//! The new-session form's picks: what each marks, and what picking a host,
//! an instance or a model puts back.

use client::link::LinkState;
use proto::{Availability, Instances};

use super::projects::desk_home;
use super::{
    HOST, connection, form, id, instance, paired, run, text, two_claudes,
    welcomed,
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
        welcomed(HOST, "desk", Instances(instances), Some(desk_home())),
        connection(HOST, LinkState::Live, None),
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
            FormIntent::PickProject(super::projects::pid("alpha").to_string()),
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
            Some((
                Vec::from([true, false, false]),
                Vec::from([true, false]),
                Vec::from([true, false, false]),
                Vec::from([false, true, false, false, false, false]),
            )),
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
fn a_project_needs_a_checkout_before_it_can_start() {
    let place = |project: &str| {
        picked(Vec::from([FormIntent::PickProject(
            super::projects::pid(project).to_string(),
        )]))
        .map(|shown| (shown.directory_shown, shown.start_disabled))
    };
    assert_eq!(
        [place("alpha"), place("beta"), place("zeta")],
        [
            Some((false, false)),
            Some((false, true)),
            Some((false, true))
        ]
    );
}

#[test]
fn starting_a_directory_sends_only_once_until_the_host_replies() {
    let (ready, _) = run(
        paired(),
        Vec::from([
            connection(HOST, LinkState::Live, None),
            super::roster(two_claudes()),
            Intent::Show(Place::NewSession),
            Intent::Form(FormIntent::PickModel(text("opus"))),
            Intent::Form(FormIntent::PickEffort(text("high"))),
            Intent::Form(FormIntent::TypeDirectory(text(" /src/test "))),
        ]),
    );
    let (pending, first) =
        crate::update(ready.clone(), Intent::Form(FormIntent::Start));
    let (_, second) =
        crate::update(pending.clone(), Intent::Form(FormIntent::Start));
    assert_eq!(form(&ready).map(|shown| shown.start_disabled), Some(false));
    assert_eq!(form(&pending).map(|shown| shown.start_disabled), Some(true));
    assert!(matches!(
        first.as_slice(),
        [crate::Effect::Send { host: HOST, .. }]
    ));
    assert_eq!(
        super::sent(&first),
        [proto::CommandBody::StartSession {
            instance: id("claude"),
            at: proto::StartAt::Directory(proto::HostPath(text("/src/test"))),
            model: Some(proto::ModelId(text("opus"))),
            effort: Some(proto::Effort::High),
        }]
    );
    assert_eq!(second, []);
}

#[test]
fn a_blank_directory_blocks_start_until_a_path_is_entered() {
    let (blank, effects) = run(
        paired(),
        Vec::from([
            connection(HOST, LinkState::Live, None),
            super::roster(two_claudes()),
            Intent::Show(Place::NewSession),
            Intent::Form(FormIntent::TypeDirectory(text("   "))),
            Intent::Form(FormIntent::Start),
        ]),
    );
    assert_eq!(
        form(&blank).map(|shown| (
            shown.start_disabled,
            shown.error.map(|hint| hint.text)
        )),
        Some((true, Some(text("Enter a directory path."))))
    );
    assert_eq!(effects, []);
    let (ready, _) = crate::update(
        blank,
        Intent::Form(FormIntent::TypeDirectory(text("/src/test"))),
    );
    assert_eq!(form(&ready).map(|shown| shown.start_disabled), Some(false));
}

#[test]
fn going_offline_blocks_start_until_the_chosen_host_returns() {
    let (ready, _) = run(
        paired(),
        Vec::from([
            connection(HOST, LinkState::Live, None),
            super::roster(two_claudes()),
            Intent::Show(Place::NewSession),
            Intent::Form(FormIntent::PickHost(HOST.key())),
            Intent::Form(FormIntent::TypeDirectory(text("/src/test"))),
        ]),
    );
    let (offline, _) =
        crate::update(ready, connection(HOST, LinkState::Disconnected, None));
    let (blocked, effects) =
        crate::update(offline, Intent::Form(FormIntent::Start));
    assert_eq!(
        form(&blocked).map(|shown| (
            shown.start_disabled,
            shown.error.map(|hint| hint.text)
        )),
        Some((true, Some(text("The selected host is offline."))))
    );
    assert_eq!(effects, []);
    let (online, _) =
        crate::update(blocked, connection(HOST, LinkState::Live, None));
    assert_eq!(form(&online).map(|shown| shown.start_disabled), Some(false));
    let (_, effects) = crate::update(online, Intent::Form(FormIntent::Start));
    assert!(matches!(
        effects.as_slice(),
        [crate::Effect::Send { host: HOST, .. }]
    ));
}

#[test]
fn start_waits_for_the_selected_instance_to_be_available_again() {
    let selected = id("https://gw.example");
    let (ready, _) = run(
        paired(),
        Vec::from([
            connection(HOST, LinkState::Live, None),
            super::roster(two_claudes()),
            Intent::Show(Place::NewSession),
            Intent::Form(FormIntent::PickInstance(selected.clone())),
            Intent::Form(FormIntent::TypeDirectory(text("/src/test"))),
        ]),
    );
    let mut changed = two_claudes();
    changed.0.insert(
        selected.clone(),
        instance(Availability::Unavailable {
            reason: text("sign in again"),
        }),
    );
    let (unavailable, blocked) = run(
        ready,
        Vec::from([super::roster(changed), Intent::Form(FormIntent::Start)]),
    );
    assert_eq!(
        form(&unavailable).map(|shown| shown.start_disabled),
        Some(true)
    );
    assert_eq!(
        form(&unavailable)
            .and_then(|shown| shown.instance_hint)
            .map(|hint| hint.text),
        Some(text("https://gw.example can't run on desk: sign in again"))
    );
    assert_eq!(blocked, []);

    let (missing, blocked) = run(
        unavailable,
        Vec::from([
            super::roster(super::claude(super::signed_in())),
            Intent::Form(FormIntent::Start),
        ]),
    );
    assert_eq!(form(&missing).map(|shown| shown.start_disabled), Some(true));
    assert_eq!(blocked, []);

    let (recovered, _) = crate::update(missing, super::roster(two_claudes()));
    assert_eq!(
        form(&recovered).map(|shown| shown.start_disabled),
        Some(false)
    );
    let (_, started) =
        crate::update(recovered, Intent::Form(FormIntent::Start));
    assert_eq!(
        super::sent(&started),
        [proto::CommandBody::StartSession {
            instance: selected,
            at: proto::StartAt::Directory(proto::HostPath(text("/src/test"))),
            model: None,
            effort: None,
        }]
    );
}

#[test]
fn start_waits_when_the_selected_model_or_effort_is_no_longer_offered() {
    let (ready, _) = run(
        paired(),
        Vec::from([
            connection(HOST, LinkState::Live, None),
            super::roster(two_claudes()),
            Intent::Show(Place::NewSession),
            Intent::Form(FormIntent::PickModel(text("opus"))),
            Intent::Form(FormIntent::PickEffort(text("high"))),
            Intent::Form(FormIntent::TypeDirectory(text("/src/test"))),
        ]),
    );
    let changed_offer = |models| {
        super::roster(super::claude(Availability::Available {
            auth: proto::Auth::SignedIn,
            models,
        }))
    };
    let mut low_only = super::models();
    low_only.choices[0].efforts = Vec::from([proto::Effort::Low]);
    for (models, reason) in [
        (
            proto::Models::none(),
            "The selected model is no longer offered.",
        ),
        (low_only, "The selected effort is no longer offered."),
    ] {
        let (blocked, effects) = run(
            ready.clone(),
            Vec::from([changed_offer(models), Intent::Form(FormIntent::Start)]),
        );
        assert_eq!(
            form(&blocked).map(|shown| (
                shown.start_disabled,
                shown.error.map(|hint| hint.text)
            )),
            Some((true, Some(text(reason))))
        );
        assert_eq!(effects, []);
        let (restored, _) =
            crate::update(blocked, changed_offer(super::models()));
        assert_eq!(
            form(&restored).map(|shown| shown.start_disabled),
            Some(false)
        );
        assert_eq!(
            form(&restored)
                .and_then(|shown| shown
                    .models
                    .into_iter()
                    .find(|choice| choice.selected))
                .map(|choice| choice.value),
            Some(text("opus"))
        );
        assert_eq!(
            form(&restored)
                .and_then(|shown| shown
                    .efforts
                    .into_iter()
                    .find(|choice| choice.selected))
                .map(|choice| choice.value),
            Some(text("high"))
        );
    }
}

#[test]
fn a_model_pick_arriving_after_the_offer_changes_stays_selected() {
    let (blocked, effects) = run(
        paired(),
        Vec::from([
            connection(HOST, LinkState::Live, None),
            super::roster(two_claudes()),
            Intent::Show(Place::NewSession),
            Intent::Form(FormIntent::TypeDirectory(text("/src/test"))),
            super::roster(super::claude(Availability::Available {
                auth: proto::Auth::SignedIn,
                models: proto::Models::none(),
            })),
            Intent::Form(FormIntent::PickModel(text("opus"))),
            Intent::Form(FormIntent::Start),
        ]),
    );
    assert_eq!(effects, []);
    assert_eq!(
        form(&blocked).map(|shown| (
            shown.start_disabled,
            shown.error.map(|hint| hint.text),
            shown
                .models
                .into_iter()
                .find(|choice| choice.selected)
                .map(|choice| (choice.value, choice.disabled)),
        )),
        Some((
            true,
            Some(text("The selected model is no longer offered.")),
            Some((text("opus"), true)),
        ))
    );
}

#[test]
fn refusals_remain_visible_alongside_current_start_blocks() {
    let (pending, sent) = run(
        paired(),
        Vec::from([
            welcomed(HOST, "desk", two_claudes(), Some(desk_home())),
            connection(HOST, LinkState::Live, None),
            Intent::Show(Place::NewSession),
            Intent::Form(FormIntent::TypeDirectory(text("/src/test"))),
            Intent::Form(FormIntent::Start),
        ]),
    );
    let alerts = |state: &State| {
        let Some(shown) = form(state) else {
            return Vec::new();
        };
        let mut alerts = Vec::new();
        if shown.directory_shown {
            alerts.extend(shown.directory_error);
        }
        alerts.extend(shown.instance_hint);
        alerts.extend(shown.instance_error);
        alerts.extend(shown.refusal);
        alerts.extend(shown.error);
        alerts
            .into_iter()
            .filter(|hint| hint.role == Some("alert"))
            .map(|hint| hint.text)
            .collect::<Vec<_>>()
    };

    let (offline, _) = run(
        pending.clone(),
        Vec::from([
            connection(HOST, LinkState::Disconnected, None),
            super::answer(
                &sent,
                Ok(proto::CommandResult::Rejected(
                    proto::Rejection::TurnInProgress,
                )),
            ),
        ]),
    );
    assert_eq!(
        alerts(&offline),
        [
            text("Claude Code on desk: a turn is already running"),
            text("The selected host is offline."),
        ]
    );

    let (project, _) = run(
        pending.clone(),
        Vec::from([
            Intent::Form(FormIntent::PickProject(
                super::projects::pid("alpha").to_string(),
            )),
            super::answer(
                &sent,
                Ok(proto::CommandResult::Rejected(
                    proto::Rejection::NoSuchDirectory,
                )),
            ),
        ]),
    );
    assert_eq!(
        alerts(&project),
        [text("There is no directory at /src/test on desk."),]
    );

    for (current, expected) in [
        (
            "sign in again",
            Vec::from([
                text("Claude Code can't run on desk: sign in again"),
                text("Claude Code can't run on desk: setup required"),
            ]),
        ),
        (
            "setup required",
            Vec::from([text("Claude Code can't run on desk: setup required")]),
        ),
    ] {
        let (unavailable, _) = run(
            pending.clone(),
            Vec::from([
                super::roster(super::claude(Availability::Unavailable {
                    reason: text(current),
                })),
                super::answer(
                    &sent,
                    Ok(proto::CommandResult::Rejected(
                        proto::Rejection::InstanceUnavailable {
                            reason: text("setup required"),
                        },
                    )),
                ),
            ]),
        );
        assert_eq!(alerts(&unavailable), expected);
    }
}
