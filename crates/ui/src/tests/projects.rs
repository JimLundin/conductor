//! The projects view and starting a session in a project.

use alloc::collections::BTreeMap;

use client::link::{LinkState, Status, Update};
use proto::{
    AuthToken, Checkout, Cloning, CommandBody, CommandResult, EventKind, Home,
    HomeSync, HostPath, Origin, Project, ProjectId, Rejection, StartAt, Ticket,
    Workspace,
};

use super::{
    HOST, SESSION, catalog, claude, created, event, form, id, listing, paired,
    run, screen, sent, signed_in, text,
};
use crate::{
    CheckoutRow, Detail, FormIntent, HomeLine, Intent, Listing, Place,
    ProjectGroup, ProjectIntent, ProjectsScreen, when,
};

const OTHER: crate::HostId = crate::HostId([7; 32]);
const ALPHA: &str = "/home/me/alpha";

fn laptop() -> Ticket {
    Ticket {
        host: [7; 32],
        relay: text("https://relay.example/"),
        token: AuthToken::from_bytes([2; 16]),
    }
}

fn project(
    name: &str,
    remote: Option<&str>,
    checkouts: Vec<Checkout>,
    cloning: Cloning,
) -> Project {
    Project {
        id: ProjectId(text(name)),
        name: text(name),
        title: format!("Project {name}"),
        remote: remote.map(text),
        profiles: Vec::from([text("rust")]),
        checkouts,
        cloning,
    }
}

fn at(path: &str, origin: Origin) -> Checkout {
    Checkout {
        path: HostPath(text(path)),
        origin,
    }
}

fn home(sync: HomeSync, projects: Vec<Project>) -> Home {
    Home {
        remote: text("https://github.com/me/home.git"),
        synced: Some(1_700_000_000),
        sync,
        projects,
    }
}

/// A host's status, live or not, with this home.
fn homed(
    host: crate::HostId,
    name: &str,
    state: LinkState,
    home: Option<Home>,
) -> Intent {
    Intent::Link(
        host,
        Update::Status(Status {
            state,
            host: Some(text(name)),
            instances: Some(claude(signed_in())),
            catalog: catalog(),
            sign_ins: BTreeMap::new(),
            home,
            error: None,
        }),
    )
}

/// desk's home: alpha checked out twice, beta not, a local-only project,
/// one cloning and one whose clone failed.
fn desk_home() -> Home {
    home(
        HomeSync::Synced,
        Vec::from([
            project(
                "alpha",
                Some("https://github.com/me/alpha.git"),
                Vec::from([
                    at(ALPHA, Origin::Found),
                    at("/data/projects/alpha", Origin::Cloned),
                ]),
                Cloning::Idle,
            ),
            project(
                "beta",
                Some("https://github.com/me/beta.git"),
                Vec::new(),
                Cloning::Idle,
            ),
            project("local", None, Vec::new(), Cloning::Idle),
            project("gamma", Some("g"), Vec::new(), Cloning::Running),
            project(
                "delta",
                Some("d"),
                Vec::new(),
                Cloning::Failed {
                    reason: text("denied"),
                },
            ),
        ]),
    )
}

fn projects(state: &crate::State) -> Option<ProjectsScreen> {
    screen(state).and_then(|host| match host.detail {
        Detail::Projects(shown) => Some(*shown),
        Detail::Placeholder(_)
        | Detail::Pair(_)
        | Detail::NewSession(_)
        | Detail::Credentials(_)
        | Detail::Session(_) => None,
    })
}

fn row(
    key: crate::HostId,
    host: &str,
    state: &'static str,
    paths: &[&str],
    text_shown: Option<&str>,
) -> CheckoutRow {
    CheckoutRow {
        key: key.key(),
        host: text(host),
        state,
        paths: paths.iter().copied().map(text).collect(),
        text: text_shown.map(text),
        clone: None,
    }
}

/// desk and laptop, each live with its home, on the projects view.
fn two_homes() -> crate::State {
    let (shown, _) = run(
        paired(),
        Vec::from([
            Intent::Join(Ok(laptop())),
            homed(HOST, "desk", LinkState::Live, Some(desk_home())),
            homed(
                OTHER,
                "laptop",
                LinkState::Live,
                Some(home(
                    HomeSync::Behind {
                        reason: text("offline"),
                    },
                    Vec::from([project(
                        "alpha",
                        Some("https://github.com/me/alpha.git"),
                        Vec::new(),
                        Cloning::Idle,
                    )]),
                )),
            ),
            Intent::Show(Place::Projects),
        ]),
    );
    shown
}

#[test]
fn the_projects_view_shows_each_hosts_home_and_how_it_syncs() {
    assert_eq!(
        projects(&two_homes()).map(|screen| screen.homes),
        Some(Vec::from([
            HomeLine {
                key: HOST.key(),
                host: text("desk"),
                state: "synced",
                text: text(
                    "Home synced 2023-11-14 22:13 UTC from https://github.com/me/home.git."
                ),
                sync: Some(Intent::Project(HOST, ProjectIntent::Sync)),
            },
            HomeLine {
                key: OTHER.key(),
                host: text("laptop"),
                state: "behind",
                text: text(
                    "Home as of 2023-11-14 22:13 UTC; the last pull from https://github.com/me/home.git failed: offline"
                ),
                sync: Some(Intent::Project(OTHER, ProjectIntent::Sync)),
            },
        ]))
    );
}

#[test]
fn each_project_lists_its_checkouts_on_each_host_that_lists_it() {
    let clone =
        |host: crate::HostId, name: &str, on: &str| crate::CloneButton {
            intent: Intent::Project(
                host,
                ProjectIntent::Clone(ProjectId(text(name))),
            ),
            label: format!("Clone Project {name} on {on}"),
        };
    assert_eq!(
        projects(&two_homes()).map(|screen| (screen.projects, screen.empty)),
        Some((
            Vec::from([
                ProjectGroup {
                    key: text("alpha"),
                    title: text("Project alpha"),
                    detail: text("alpha · rust"),
                    hosts: Vec::from([
                        row(
                            HOST,
                            "desk",
                            "checked-out",
                            &[ALPHA, "/data/projects/alpha (cloned here)"],
                            None
                        ),
                        CheckoutRow {
                            clone: Some(clone(OTHER, "alpha", "laptop")),
                            ..row(
                                OTHER,
                                "laptop",
                                "missing",
                                &[],
                                Some("Not checked out")
                            )
                        },
                    ]),
                },
                ProjectGroup {
                    key: text("beta"),
                    title: text("Project beta"),
                    detail: text("beta · rust"),
                    hosts: Vec::from([CheckoutRow {
                        clone: Some(clone(HOST, "beta", "desk")),
                        ..row(
                            HOST,
                            "desk",
                            "missing",
                            &[],
                            Some("Not checked out")
                        )
                    }]),
                },
                ProjectGroup {
                    key: text("delta"),
                    title: text("Project delta"),
                    detail: text("delta · rust"),
                    hosts: Vec::from([CheckoutRow {
                        clone: Some(clone(HOST, "delta", "desk")),
                        ..row(
                            HOST,
                            "desk",
                            "failed",
                            &[],
                            Some("Clone failed: denied")
                        )
                    }]),
                },
                ProjectGroup {
                    key: text("gamma"),
                    title: text("Project gamma"),
                    detail: text("gamma · rust"),
                    hosts: Vec::from([row(
                        HOST,
                        "desk",
                        "cloning",
                        &[],
                        Some("Cloning…")
                    )]),
                },
                ProjectGroup {
                    key: text("local"),
                    title: text("Project local"),
                    detail: text("local · rust"),
                    hosts: Vec::from([row(
                        HOST,
                        "desk",
                        "missing",
                        &[],
                        Some("Not checked out")
                    )]),
                },
            ]),
            None
        ))
    );
}

#[test]
fn a_host_with_no_home_or_no_link_says_so_and_offers_nothing() {
    let (shown, _) = run(
        paired(),
        Vec::from([
            Intent::Join(Ok(laptop())),
            homed(HOST, "desk", LinkState::Live, None),
            Intent::Show(Place::Projects),
        ]),
    );
    let (offline, _) = run(
        paired(),
        Vec::from([
            homed(
                HOST,
                "desk",
                LinkState::Connecting,
                Some(home(
                    HomeSync::Syncing,
                    Vec::from([project(
                        "beta",
                        Some("b"),
                        Vec::new(),
                        Cloning::Idle,
                    )]),
                )),
            ),
            Intent::Show(Place::Projects),
        ]),
    );
    assert_eq!(
        (
            lines(&shown),
            projects(&shown).and_then(|screen| screen.empty)
        ),
        (
            Some(Vec::from([
                (
                    "none",
                    text(
                        "desk has no home. Start it with --home <remote> to list its projects."
                    ),
                    false
                ),
                (
                    "offline",
                    text("Not connected to host 07070707 yet."),
                    false
                ),
            ])),
            Some(text(
                "No home lists a project yet. A host lists its home's projects once it has cloned it."
            )),
        )
    );
    assert_eq!(
        (
            lines(&offline),
            projects(&offline)
                .map(|screen| screen.projects[0].hosts[0].clone.is_some()),
        ),
        (
            Some(Vec::from([(
                "syncing",
                text("Cloning the home from https://github.com/me/home.git…"),
                false
            )])),
            Some(false),
        )
    );
}

/// Each home's line: its state, its text and whether it offers Sync.
fn lines(state: &crate::State) -> Option<Vec<(&'static str, String, bool)>> {
    projects(state).map(|screen| {
        screen
            .homes
            .into_iter()
            .map(|line| (line.state, line.text, line.sync.is_some()))
            .collect()
    })
}

#[test]
fn a_host_with_no_clone_says_why_and_offers_sync() {
    let failed = home(
        HomeSync::Failed {
            reason: text("no such repository"),
        },
        Vec::new(),
    );
    let (broken, _) = run(
        paired(),
        Vec::from([
            homed(
                HOST,
                "desk",
                LinkState::Live,
                Some(Home {
                    synced: None,
                    ..failed
                }),
            ),
            Intent::Show(Place::Projects),
        ]),
    );
    assert_eq!(
        lines(&broken),
        Some(Vec::from([(
            "failed",
            text(
                "No copy of the home from https://github.com/me/home.git: no such repository"
            ),
            true
        )]))
    );
}

#[test]
fn sync_and_clone_send_their_commands_and_a_refusal_is_the_notice() {
    let (shown, _) = run(
        paired(),
        Vec::from([
            homed(HOST, "desk", LinkState::Live, Some(desk_home())),
            Intent::Show(Place::Projects),
        ]),
    );
    let (refused, effects) = run(
        shown,
        Vec::from([
            Intent::Project(HOST, ProjectIntent::Sync),
            Intent::Project(
                HOST,
                ProjectIntent::Clone(ProjectId(text("beta"))),
            ),
        ]),
    );
    assert_eq!(
        sent(&effects),
        [
            CommandBody::SyncHome,
            CommandBody::CloneProject {
                project: ProjectId(text("beta")),
            },
        ]
    );
    let Some(crate::Effect::Send { reply, .. }) = effects.last().cloned()
    else {
        panic_free_fail();
        return;
    };
    let (answered, _) = run(
        refused,
        Vec::from([Intent::Replied(
            reply,
            Ok(CommandResult::Rejected(Rejection::NoRemote)),
        )]),
    );
    assert_eq!(
        projects(&answered).and_then(|screen| screen.notice),
        Some(text(
            "the project is local only, so there is nothing to clone"
        ))
    );
}

fn panic_free_fail() {
    assert_eq!(Some("a command was sent"), None);
}

#[test]
fn a_session_starts_in_a_checked_out_project_picked_in_the_form() {
    let (shown, _) = run(
        paired(),
        Vec::from([
            homed(HOST, "desk", LinkState::Live, Some(desk_home())),
            Intent::Show(Place::NewSession),
        ]),
    );
    let choices = |state: &crate::State| {
        form(state).map(|drawn| {
            (
                drawn.projects.map(|listed| {
                    listed
                        .into_iter()
                        .map(|choice| {
                            (choice.label, choice.selected, choice.disabled)
                        })
                        .collect::<Vec<_>>()
                }),
                drawn.directory_shown,
                drawn.start_disabled,
            )
        })
    };
    let (picked, started) = run(
        shown.clone(),
        Vec::from([
            Intent::Form(FormIntent::PickProject(text("alpha"))),
            Intent::Form(FormIntent::Start),
        ]),
    );
    assert_eq!(
        choices(&shown),
        Some((
            Some(Vec::from([
                (text("A directory"), true, false),
                (text("Project alpha"), false, false),
                (text("Project beta (not checked out)"), false, true),
                (text("Project local (not checked out)"), false, true),
                (text("Project gamma (not checked out)"), false, true),
                (text("Project delta (not checked out)"), false, true),
            ])),
            true,
            true,
        ))
    );
    assert_eq!(
        choices(&picked).map(|(_, shown_field, _)| shown_field),
        Some(false)
    );
    assert_eq!(
        sent(&started),
        [CommandBody::StartSession {
            instance: id("claude"),
            at: StartAt::Project(ProjectId(text("alpha"))),
            model: None,
            effort: None,
        }]
    );
    let (back, directory) = run(
        shown,
        Vec::from([
            Intent::Form(FormIntent::PickProject(text("alpha"))),
            Intent::Form(FormIntent::PickProject(String::new())),
            Intent::Form(FormIntent::TypeDirectory(text("/src/x"))),
            Intent::Form(FormIntent::Start),
        ]),
    );
    assert_eq!(
        choices(&back).map(|(_, shown_field, _)| shown_field),
        Some(true)
    );
    assert_eq!(
        sent(&directory),
        [CommandBody::StartSession {
            instance: id("claude"),
            at: StartAt::Directory(HostPath(text("/src/x"))),
            model: None,
            effort: None,
        }]
    );
}

#[test]
fn a_project_the_host_no_longer_lists_falls_back_to_the_directory() {
    let (gone, effects) = run(
        paired(),
        Vec::from([
            homed(HOST, "desk", LinkState::Live, Some(desk_home())),
            Intent::Show(Place::NewSession),
            Intent::Form(FormIntent::PickProject(text("alpha"))),
            homed(
                HOST,
                "desk",
                LinkState::Live,
                Some(home(HomeSync::Synced, Vec::new())),
            ),
            Intent::Form(FormIntent::Start),
        ]),
    );
    assert_eq!(
        form(&gone).map(|drawn| (
            drawn.projects,
            drawn.directory_shown,
            drawn.start_disabled
        )),
        Some((
            Some(Vec::from([crate::Choice {
                value: String::new(),
                label: text("A directory"),
                selected: true,
                disabled: false,
            }])),
            true,
            true
        ))
    );
    assert_eq!(
        sent(&effects),
        [CommandBody::StartSession {
            instance: id("claude"),
            at: StartAt::Directory(HostPath(String::new())),
            model: None,
            effort: None,
        }]
    );
    let (homeless, _) = run(
        paired(),
        Vec::from([
            homed(HOST, "desk", LinkState::Live, None),
            Intent::Show(Place::NewSession),
        ]),
    );
    assert_eq!(form(&homeless).map(|drawn| drawn.projects), Some(None));
}

#[test]
fn a_session_in_a_checkout_is_listed_by_its_project() {
    let workspace = |repository: &str| EventKind::WorkspaceCreated {
        workspace: Workspace {
            repository: HostPath(text(repository)),
            original: HostPath(text(repository)),
            worktree: HostPath(text("/data/worktrees/x")),
            directory: HostPath(text("/data/worktrees/x")),
            branch: text("conductor/x"),
        },
    };
    let titles = |repository: &str| {
        let (state, _) = run(
            paired(),
            Vec::from([
                homed(HOST, "desk", LinkState::Live, Some(desk_home())),
                event(1, created()),
                event(2, workspace(repository)),
            ]),
        );
        match listing(&state) {
            Some(Listing::Rows(rows)) => {
                rows.into_iter().map(|shown| shown.title).collect()
            }
            Some(Listing::Empty(_)) | None => Vec::new(),
        }
    };
    assert_eq!(titles(ALPHA), [text("alpha")]);
    assert_eq!(titles("/elsewhere"), [text("conductor")]);
    assert_eq!(SESSION, SESSION);
}

#[test]
fn a_time_is_a_utc_date_and_minute() {
    assert_eq!(
        [
            None,
            Some(0),
            Some(951_782_400),
            Some(1_700_000_000),
            Some(4_102_444_799)
        ]
        .map(when),
        [
            text("never"),
            text("1970-01-01 00:00 UTC"),
            text("2000-02-29 00:00 UTC"),
            text("2023-11-14 22:13 UTC"),
            text("2099-12-31 23:59 UTC"),
        ]
    );
}
