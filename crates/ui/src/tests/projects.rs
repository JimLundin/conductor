//! The projects view and starting a session in a project.

use std::collections::BTreeMap;

use client::link::{LinkState, Update};
use proto::flow::{Field, FieldType, Flow, Kind, Role, Status};
use proto::{
    AuthToken, ChecklistChange, ChecklistItem, Checkout, Cloning, CommandBody,
    CommandResult, Datum, Entry, EventKind, Feed, Home, HomeSync, HostPath,
    Issue, IssueChange, IssueId, Item, LogEvent, NewIssue, Origin, Part,
    Project, ProjectId, Rejection, Section, StartAt, Ticket, Workspace,
};

use super::{
    HOST, SESSION, claude, connection, created, event, form, id, listing,
    paired, run, screen, sent, signed_in, text, welcomed,
};
use crate::{
    CheckoutRow, Detail, Effect, FieldPicker, FormIntent, HomeLine, Intent,
    IssueIntent, IssueList, IssueRow, IssueScreen, Listing, NewIssueIntent,
    Pick, Place, ProjectGroup, ProjectIntent, ProjectsScreen, StatusGroup,
    when,
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
        provides: Vec::new(),
        checkouts,
        cloning,
        issues: Vec::new(),
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
        unpushed: 0,
        flow: Some(Box::new(flow())),
        projects,
    }
}

fn enumerated(name: &str, words: &[&str], default: Option<&str>) -> Field {
    Field {
        name: text(name),
        value: FieldType::Enum(words.iter().copied().map(text).collect()),
        default: default.map(text),
    }
}

fn kind(name: &str, label: &str, fields: Vec<Field>) -> Kind {
    Kind {
        name: text(name),
        label: text(label),
        icon: None,
        colour: None,
        sections: Vec::new(),
        fields,
        order: Vec::new(),
    }
}

/// A home's flow, like Conductor's own: six statuses, a priority, labels,
/// a branch and when an issue was opened.
fn flow() -> Flow {
    let status = |name: &str, role| Status {
        name: text(name),
        role,
        colour: None,
    };
    let field = |name: &str, value| Field {
        name: text(name),
        value,
        default: None,
    };
    Flow {
        default_kind: text("task"),
        statuses: Vec::from([
            status("open", Role::Idle),
            status("ready", Role::Startable),
            status("active", Role::Active),
            status("review", Role::Review),
            status("done", Role::Terminal),
            status("dropped", Role::Dropped),
        ]),
        fields: Vec::from([
            enumerated(
                "priority",
                &["low", "normal", "high", "urgent"],
                Some("normal"),
            ),
            field("labels", FieldType::Texts),
            field("branch", FieldType::Text),
            field("created", FieldType::Time),
        ]),
        kinds: Vec::from([kind("task", "Task", Vec::new())]),
        relations: Vec::new(),
        stages: Vec::new(),
        rules: Vec::new(),
    }
}

/// desk's home: alpha checked out twice, beta not, a local-only project,
/// one cloning and one whose clone failed.
pub(super) fn desk_home() -> Home {
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

/// A project's issue list with no issues, read from desk.
fn no_issues(name: &str) -> IssueList {
    IssueList {
        groups: Vec::new(),
        new_title: String::new(),
        project: ProjectId(text(name)),
        create: None,
    }
}

fn projects(state: &crate::State) -> Option<ProjectsScreen> {
    screen(state).and_then(|host| match host.detail {
        Detail::Projects(shown) => Some(*shown),
        Detail::Placeholder(_)
        | Detail::Pair(_)
        | Detail::NewSession(_)
        | Detail::Credentials(_)
        | Detail::Session(_)
        | Detail::Issue(_) => None,
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
            Intent::Show(Place::AddHost),
            Intent::Join(Ok(laptop())),
            welcomed(HOST, "desk", claude(signed_in()), Some(desk_home())),
            connection(HOST, LinkState::Live, None),
            welcomed(
                OTHER,
                "laptop",
                claude(signed_in()),
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
            connection(OTHER, LinkState::Live, None),
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
                    issues: no_issues("alpha"),
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
                    issues: no_issues("beta"),
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
                    issues: no_issues("delta"),
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
                    issues: no_issues("gamma"),
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
                    issues: no_issues("local"),
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
            Intent::Show(Place::AddHost),
            Intent::Join(Ok(laptop())),
            welcomed(HOST, "desk", claude(signed_in()), None),
            connection(HOST, LinkState::Live, None),
            Intent::Show(Place::Projects),
        ]),
    );
    let (offline, _) = run(
        paired(),
        Vec::from([
            welcomed(
                HOST,
                "desk",
                claude(signed_in()),
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
            connection(HOST, LinkState::Connecting, None),
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
            welcomed(
                HOST,
                "desk",
                claude(signed_in()),
                Some(Home {
                    synced: None,
                    ..failed
                }),
            ),
            connection(HOST, LinkState::Live, None),
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
            welcomed(HOST, "desk", claude(signed_in()), Some(desk_home())),
            connection(HOST, LinkState::Live, None),
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
            welcomed(HOST, "desk", claude(signed_in()), Some(desk_home())),
            connection(HOST, LinkState::Live, None),
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
fn a_removed_project_stays_selected_until_the_directory_is_explicitly_chosen() {
    let (picked, _) = run(
        paired(),
        Vec::from([
            welcomed(HOST, "desk", claude(signed_in()), Some(desk_home())),
            connection(HOST, LinkState::Live, None),
            Intent::Show(Place::NewSession),
            Intent::Form(FormIntent::TypeDirectory(text("/src/x"))),
            Intent::Form(FormIntent::PickProject(text("alpha"))),
        ]),
    );
    assert_eq!(form(&picked).map(|drawn| drawn.start_disabled), Some(false));
    let (gone, effects) = run(
        picked,
        Vec::from([
            Intent::Link(
                HOST,
                Update::Feed(Feed::Home(home(HomeSync::Synced, Vec::new()))),
            ),
            Intent::Form(FormIntent::Start),
        ]),
    );
    assert_eq!(
        form(&gone).map(|drawn| (
            drawn.projects,
            drawn.directory_shown,
            drawn.start_disabled,
            drawn.error.map(|hint| hint.text),
        )),
        Some((
            Some(Vec::from([
                crate::Choice {
                    value: String::new(),
                    label: text("A directory"),
                    selected: false,
                    disabled: false,
                },
                crate::Choice {
                    value: text("alpha"),
                    label: text("alpha (no longer listed)"),
                    selected: true,
                    disabled: true,
                },
            ])),
            false,
            true,
            Some(text(
                "The selected project is no longer listed on this host."
            )),
        ))
    );
    assert_eq!(sent(&effects), []);
    let (directory, _) = run(
        gone,
        Vec::from([Intent::Form(FormIntent::PickProject(String::new()))]),
    );
    assert_eq!(
        form(&directory)
            .map(|drawn| (drawn.directory_shown, drawn.start_disabled)),
        Some((true, false)),
    );
    let (_, effects) =
        run(directory, Vec::from([Intent::Form(FormIntent::Start)]));
    assert_eq!(
        sent(&effects),
        [CommandBody::StartSession {
            instance: id("claude"),
            at: StartAt::Directory(HostPath(text("/src/x"))),
            model: None,
            effort: None,
        }]
    );
}

#[test]
fn a_project_stays_selected_across_hosts_and_needs_a_checkout_there() {
    let (laptop, effects) = run(
        two_homes(),
        Vec::from([
            Intent::Show(Place::NewSession),
            Intent::Form(FormIntent::TypeDirectory(text("/src/x"))),
            Intent::Form(FormIntent::PickProject(text("alpha"))),
            Intent::Form(FormIntent::PickHost(OTHER.key())),
            Intent::Form(FormIntent::Start),
        ]),
    );
    assert_eq!(sent(&effects), []);
    assert_eq!(
        form(&laptop).map(|drawn| (
            drawn.projects.and_then(|choices| choices
                .into_iter()
                .find(|choice| choice.selected)),
            drawn.directory_shown,
            drawn.start_disabled,
            drawn.error.map(|hint| hint.text),
        )),
        Some((
            Some(crate::Choice {
                value: text("alpha"),
                label: text("Project alpha (not checked out)"),
                selected: true,
                disabled: true,
            }),
            false,
            true,
            Some(text(
                "The selected project is not checked out on this host."
            )),
        ))
    );
    let (homeless, effects) = run(
        laptop,
        Vec::from([
            welcomed(OTHER, "laptop", claude(signed_in()), None),
            Intent::Form(FormIntent::Start),
        ]),
    );
    assert_eq!(sent(&effects), []);
    assert_eq!(
        form(&homeless).map(|drawn| (
            drawn.projects.and_then(|choices| choices
                .into_iter()
                .find(|choice| choice.selected)),
            drawn.start_disabled,
            drawn.error.map(|hint| hint.text),
        )),
        Some((
            Some(crate::Choice {
                value: text("alpha"),
                label: text("alpha (no longer listed)"),
                selected: true,
                disabled: true,
            }),
            true,
            Some(text("This host has no home configured.")),
        ))
    );
    let (ready, _) = run(
        homeless,
        Vec::from([Intent::Form(FormIntent::PickHost(HOST.key()))]),
    );
    assert_eq!(form(&ready).map(|drawn| drawn.start_disabled), Some(false));
    let (_, effects) = run(ready, Vec::from([Intent::Form(FormIntent::Start)]));
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Send { host, body: CommandBody::StartSession { at: StartAt::Project(id), .. }, .. }
        if *host == HOST && id.0 == "alpha"
    )));
}

#[test]
fn a_host_without_home_offers_a_directory_by_default() {
    let (homeless, _) = run(
        paired(),
        Vec::from([
            welcomed(HOST, "desk", claude(signed_in()), None),
            connection(HOST, LinkState::Live, None),
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
                welcomed(HOST, "desk", claude(signed_in()), Some(desk_home())),
                connection(HOST, LinkState::Live, None),
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

const OPENED: IssueId = IssueId(1);
const CLOSED: IssueId = IssueId(2);

fn issue(id: IssueId, title: &str, status: &str) -> Issue {
    Issue {
        id,
        project: ProjectId(text("alpha")),
        title: text(title),
        kind: text("task"),
        status: text(status),
        stage: None,
        fields: BTreeMap::from([
            (text("priority"), Datum::Text(text("high"))),
            (text("labels"), Datum::Texts(Vec::from([text("app")]))),
            (text("created"), Datum::Text(text("2026-11-18T10:12:00Z"))),
        ]),
        links: BTreeMap::new(),
        sections: Vec::from([Section {
            heading: String::new(),
            text: text("Why it matters."),
        }]),
        branch: text("issue/x"),
        checklist: Vec::from([
            ChecklistItem {
                done: true,
                text: text("protocol"),
            },
            ChecklistItem {
                done: false,
                text: text("browser"),
            },
        ]),
        log: Vec::from([Entry {
            at: None,
            by: None,
            event: LogEvent::Note,
            text: text("2026-11-18 desk · iPhone: started"),
        }]),
    }
}

fn issue_project(checkouts: Vec<Checkout>) -> Project {
    let mut alpha = project("alpha", Some("a"), checkouts, Cloning::Idle);
    alpha.issues = Vec::from([
        issue(OPENED, "Sign in", "open"),
        issue(CLOSED, "Pair", "done"),
    ]);
    alpha
}

fn with_issues() -> crate::State {
    let alpha = issue_project(Vec::from([at(ALPHA, Origin::Found)]));
    let (shown, _) = run(
        paired(),
        Vec::from([
            welcomed(
                HOST,
                "desk",
                claude(signed_in()),
                Some(home(HomeSync::Synced, Vec::from([alpha]))),
            ),
            connection(HOST, LinkState::Live, None),
            Intent::Show(Place::Projects),
        ]),
    );
    shown
}

fn reply_of(effects: &[Effect]) -> Option<crate::Reply> {
    effects.iter().find_map(|effect| match effect {
        Effect::Send { reply, .. } => Some(reply.clone()),
        _ => None,
    })
}

#[test]
fn a_project_lists_its_issues_by_status_and_a_new_one_opens_once_made()
-> Result<(), &'static str> {
    let alpha = || ProjectId(text("alpha"));
    let listed = projects(&with_issues())
        .and_then(|screen| screen.projects.into_iter().next())
        .map(|group| group.issues);
    let row = |id: IssueId, title: &str| IssueRow {
        key: format!("{:032x}", id.0),
        title: text(title),
        detail: text("high · app · 1/2 done"),
        open: Intent::Show(Place::Issue(HOST, id)),
    };
    assert_eq!(
        listed.map(|list| list.groups),
        Some(Vec::from([
            StatusGroup {
                status: text("open"),
                label: text("open"),
                rows: Vec::from([row(OPENED, "Sign in")]),
            },
            StatusGroup {
                status: text("done"),
                label: text("done"),
                rows: Vec::from([row(CLOSED, "Pair")]),
            },
        ]))
    );
    let (typed, effects) = run(
        with_issues(),
        Vec::from([
            Intent::NewIssue(NewIssueIntent::TypeTitle(
                alpha(),
                text(" Remove "),
            )),
            Intent::NewIssue(NewIssueIntent::Create(
                HOST,
                alpha(),
                text("task"),
            )),
        ]),
    );
    assert_eq!(
        sent(&effects),
        [CommandBody::CreateIssue(NewIssue {
            project: text("alpha"),
            kind: text("task"),
            title: text("Remove"),
            fields: Vec::new(),
        })]
    );
    let reply = reply_of(&effects).ok_or("no reply")?;
    let (opened, _) = run(
        typed,
        Vec::from([Intent::Replied(
            reply,
            Ok(CommandResult::IssueWritten {
                issue: OPENED,
                warnings: Vec::new(),
            }),
        )]),
    );
    assert_eq!(
        issue_screen(&opened).map(|issue| issue.title),
        Some(text("Sign in"))
    );
    Ok(())
}

fn moving_to(status: &str) -> IssueChange {
    IssueChange::Move {
        part: Part::Status,
        to: text(status),
        reason: String::new(),
    }
}

fn setting(field: &str, value: &str) -> IssueChange {
    IssueChange::Set {
        field: text(field),
        value: text(value),
    }
}

fn adding(item: &str) -> IssueChange {
    IssueChange::Checklist(ChecklistChange::Add(text(item)))
}

fn on_issue() -> crate::State {
    run(
        with_issues(),
        Vec::from([Intent::Show(Place::Issue(HOST, OPENED))]),
    )
    .0
}

fn issue_screen(state: &crate::State) -> Option<IssueScreen> {
    screen(state).and_then(|shown| match shown.detail {
        Detail::Issue(issue) => Some(*issue),
        _ => None,
    })
}

#[test]
fn an_issue_shows_its_fields_and_sends_each_change() -> Result<(), &'static str>
{
    let shown = issue_screen(&on_issue());
    assert_eq!(
        shown.as_ref().map(|issue| (
            issue.subtitle.clone(),
            issue.detail.clone(),
            issue.notes.clone(),
            issue
                .checklist
                .iter()
                .map(|item| item.done)
                .collect::<Vec<_>>(),
            issue
                .status
                .iter()
                .find(|pick| pick.selected)
                .map(|pick| pick.word.clone()),
        )),
        Some((
            text("Project alpha · desk"),
            text("Task · issue/x · opened 2026-11-18T10:12:00Z"),
            Vec::from([text("2026-11-18 desk · iPhone: started")]),
            Vec::from([true, false]),
            Some(text("open")),
        ))
    );
    let row = shown
        .and_then(|issue| issue.checklist.get(1).cloned())
        .ok_or("the issue should have a second row")?;
    let (draft, effects) = run(
        on_issue(),
        Vec::from([
            Intent::Issue(IssueIntent::SetStatus(text("active"))),
            Intent::Issue(IssueIntent::SetStatus(text("open"))),
            Intent::Issue(row.toggle),
            Intent::Issue(IssueIntent::TypeItem(text("docs"))),
            Intent::Issue(IssueIntent::AddItem),
            Intent::Issue(IssueIntent::TypeNote(text("  "))),
            Intent::Issue(IssueIntent::AddNote),
            Intent::Issue(IssueIntent::TypeNote(text("halfway"))),
            Intent::Issue(IssueIntent::AddNote),
        ]),
    );
    let change = |change| CommandBody::ChangeIssue {
        issue: OPENED,
        change,
    };
    assert_eq!(
        sent(&effects),
        [
            change(moving_to("active")),
            change(IssueChange::Checklist(ChecklistChange::Check {
                item: Item {
                    index: 1,
                    seen: Some(issue(OPENED, "Sign in", "open").checklist),
                },
                done: true,
            })),
            change(adding("docs")),
            change(IssueChange::Note(text("halfway"))),
        ]
    );
    let replies = effects
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Send { reply, .. } => {
                Some(Intent::Replied(reply, Ok(CommandResult::Accepted)))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let (settled, _) = run(draft, replies);
    assert_eq!(
        issue_screen(&settled).map(|view| (view.item, view.note)),
        Some((String::new(), String::new()))
    );
    Ok(())
}

#[test]
fn editing_an_issue_sends_its_fields_once_saved() {
    let (edited, effects) = run(
        on_issue(),
        Vec::from([
            Intent::Issue(IssueIntent::Edit),
            Intent::Issue(IssueIntent::TypeTitle(text("Sign in from the app"))),
            Intent::Issue(IssueIntent::TypeLabels(text("app, credentials,"))),
            Intent::Issue(IssueIntent::Save),
        ]),
    );
    let change = |change| CommandBody::ChangeIssue {
        issue: OPENED,
        change,
    };
    assert_eq!(
        sent(&effects),
        [
            change(setting("title", "Sign in from the app")),
            change(setting("labels", "app, credentials,")),
        ],
        "the body didn't change, so it isn't sent"
    );
    assert!(
        issue_screen(&edited)
            .and_then(|issue| issue.editing)
            .is_some()
    );
    let replies = effects
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Send { reply, .. } => {
                Some(Intent::Replied(reply, Ok(CommandResult::Accepted)))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let (settled, _) = run(edited, replies);
    assert_eq!(
        issue_screen(&settled).map(|issue| issue.editing),
        Some(None)
    );
}

fn refresh_issue(update: impl FnOnce(&mut Issue)) -> Intent {
    let mut alpha = issue_project(Vec::from([at(ALPHA, Origin::Found)]));
    if let Some(open) = alpha.issues.first_mut() {
        update(open);
    }
    Intent::Link(
        HOST,
        Update::Feed(Feed::Home(home(HomeSync::Synced, Vec::from([alpha])))),
    )
}

#[test]
fn only_edited_fields_are_sent_and_a_sibling_success_keeps_the_failure()
-> Result<(), &'static str> {
    let (draft, _) = run(
        on_issue(),
        Vec::from([
            Intent::Issue(IssueIntent::Edit),
            Intent::Issue(IssueIntent::TypeTitle(text("New title"))),
            refresh_issue(|issue| {
                issue.sections = Vec::from([Section {
                    heading: String::new(),
                    text: text("Remote body edit"),
                }]);
            }),
            Intent::Issue(IssueIntent::TypeLabels(text("new labels"))),
        ]),
    );
    let (saving, effects) =
        run(draft, Vec::from([Intent::Issue(IssueIntent::Save)]));
    assert_eq!(
        sent(&effects),
        [
            CommandBody::ChangeIssue {
                issue: OPENED,
                change: setting("title", "New title"),
            },
            CommandBody::ChangeIssue {
                issue: OPENED,
                change: setting("labels", "new labels"),
            },
        ]
    );
    let mut replies = effects.into_iter().filter_map(|effect| match effect {
        Effect::Send { reply, .. } => Some(reply),
        _ => None,
    });
    let title = replies.next().ok_or("no title reply")?;
    let labels = replies.next().ok_or("no labels reply")?;
    let (settled, _) = run(
        saving,
        Vec::from([
            Intent::Replied(title, Err(text("Issue title was refused"))),
            Intent::Replied(labels, Ok(CommandResult::Accepted)),
        ]),
    );
    let screen = issue_screen(&settled).ok_or("no issue screen")?;
    assert_eq!(
        screen.submission_errors,
        vec![text("Title: Issue title was refused")]
    );
    assert_eq!(screen.body, "Remote body edit");
    assert_eq!(
        screen.editing.map(|draft| draft.title),
        Some(text("New title"))
    );
    let (_, retry) =
        run(settled, Vec::from([Intent::Issue(IssueIntent::Save)]));
    assert_eq!(
        sent(&retry),
        [CommandBody::ChangeIssue {
            issue: OPENED,
            change: setting("title", "New title"),
        }]
    );
    Ok(())
}

#[test]
fn transport_failures_keep_submitted_drafts_and_later_input()
-> Result<(), &'static str> {
    let (saving, effects) = run(
        on_issue(),
        Vec::from([
            Intent::Issue(IssueIntent::TypeItem(text("Checklist draft"))),
            Intent::Issue(IssueIntent::AddItem),
            Intent::Issue(IssueIntent::TypeNote(text("Note draft"))),
            Intent::Issue(IssueIntent::AddNote),
            Intent::Issue(IssueIntent::Edit),
            Intent::Issue(IssueIntent::TypeTitle(text("Issue draft"))),
            Intent::Issue(IssueIntent::Save),
        ]),
    );
    let mut intents = Vec::from([
        connection(HOST, LinkState::Disconnected, None),
        Intent::Issue(IssueIntent::TypeNote(text("More recent note draft"))),
    ]);
    intents.extend(effects.into_iter().filter_map(|effect| match effect {
        Effect::Send { reply, .. } => {
            Some(Intent::Replied(reply, Err(text("The host disconnected"))))
        }
        _ => None,
    }));
    let (settled, _) = run(saving, intents);
    let screen = issue_screen(&settled).ok_or("no issue screen")?;
    assert_eq!(screen.item, "Checklist draft");
    assert_eq!(screen.note, "More recent note draft");
    assert_eq!(
        screen.editing.map(|draft| draft.title),
        Some(text("Issue draft"))
    );
    assert_eq!(screen.submission_errors.len(), 3);

    let (_, retries) = run(
        settled,
        Vec::from([
            connection(HOST, LinkState::Live, None),
            Intent::Issue(IssueIntent::AddItem),
            Intent::Issue(IssueIntent::AddNote),
        ]),
    );
    let submitted_changes = sent(&retries)
        .into_iter()
        .filter_map(|body| match body {
            CommandBody::ChangeIssue { change, .. } => Some(change),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        submitted_changes,
        [
            adding("Checklist draft"),
            IssueChange::Note(text("More recent note draft")),
        ]
    );
    Ok(())
}

#[test]
fn a_rendered_checklist_row_uses_its_snapshot_after_a_refresh()
-> Result<(), &'static str> {
    let (initial, _) = run(
        on_issue(),
        Vec::from([refresh_issue(|issue| {
            issue.checklist = vec![
                ChecklistItem {
                    done: false,
                    text: text("repeat")
                };
                2
            ];
        })]),
    );
    let row = issue_screen(&initial)
        .and_then(|issue| issue.checklist.get(1).cloned())
        .ok_or("missing duplicate row")?;
    let (_, effects) = run(
        initial,
        Vec::from([
            refresh_issue(|issue| {
                let _ = issue.checklist.remove(0);
            }),
            Intent::Issue(row.toggle),
            Intent::Issue(row.remove),
        ]),
    );
    let seen = || Item {
        index: 1,
        seen: Some(vec![
            ChecklistItem {
                done: false,
                text: text("repeat")
            };
            2
        ]),
    };
    assert_eq!(
        sent(&effects),
        [
            CommandBody::ChangeIssue {
                issue: OPENED,
                change: IssueChange::Checklist(ChecklistChange::Check {
                    item: seen(),
                    done: true,
                }),
            },
            CommandBody::ChangeIssue {
                issue: OPENED,
                change: IssueChange::Checklist(ChecklistChange::Remove(seen())),
            },
        ]
    );
    Ok(())
}

#[test]
fn an_issue_gone_from_its_home_falls_back_to_the_placeholder() {
    let (gone, _) = run(
        paired(),
        Vec::from([
            welcomed(
                HOST,
                "desk",
                claude(signed_in()),
                Some(home(HomeSync::Synced, Vec::new())),
            ),
            connection(HOST, LinkState::Live, None),
            Intent::Show(Place::Issue(HOST, OPENED)),
        ]),
    );
    assert_eq!(
        screen(&gone).map(|shown| shown.detail),
        Some(Detail::Placeholder(
            "That issue is gone from its host's home."
        ))
    );
}

#[test]
fn an_issue_starts_a_session_on_it_and_lists_its_sessions() {
    let shown = issue_screen(&on_issue());
    assert_eq!(
        shown.as_ref().map(|issue| issue.start.clone()),
        Some(Intent::StartOn(HOST, OPENED))
    );
    let (opened, _) =
        run(on_issue(), Vec::from([Intent::StartOn(HOST, OPENED)]));
    let shown_form = form(&opened);
    assert_eq!(
        shown_form.as_ref().map(|it| (
            it.issue.clone(),
            it.projects.is_none(),
            it.directory_shown
        )),
        Some((Some(text("Sign in")), true, false))
    );
    let (_, effects) =
        run(opened, Vec::from([Intent::Form(FormIntent::Start)]));
    assert_eq!(
        sent(&effects),
        [CommandBody::StartSession {
            instance: id("claude"),
            at: StartAt::Issue(OPENED),
            model: None,
            effort: None,
        }]
    );
    let (worked, _) = run(
        on_issue(),
        Vec::from([
            event(1, created()),
            event(2, EventKind::OnIssue { issue: OPENED }),
            event(3, EventKind::SessionReady),
        ]),
    );
    assert_eq!(
        issue_screen(&worked).map(|issue| issue
            .sessions
            .into_iter()
            .map(|session| (session.label, session.open))
            .collect::<Vec<_>>()),
        Some(Vec::from([(
            text("Claude Code on desk · idle"),
            Intent::Show(Place::Session(HOST, SESSION)),
        )]))
    );
}

#[test]
fn an_issue_stays_selected_across_hosts_and_needs_its_project_checkout_there() {
    let (laptop, effects) = run(
        on_issue(),
        Vec::from([
            Intent::Show(Place::AddHost),
            Intent::Join(Ok(laptop())),
            welcomed(OTHER, "laptop", claude(signed_in()), Some(desk_home())),
            connection(OTHER, LinkState::Live, None),
            Intent::StartOn(HOST, OPENED),
            Intent::Form(FormIntent::PickHost(OTHER.key())),
            Intent::Form(FormIntent::Start),
        ]),
    );
    let readiness = |state: &crate::State| {
        form(state).map(|drawn| {
            (
                drawn.issue,
                drawn.start_disabled,
                drawn.error.map(|hint| hint.text),
            )
        })
    };
    assert_eq!(sent(&effects), []);
    assert_eq!(
        readiness(&laptop),
        Some((
            Some(text("Sign in")),
            true,
            Some(text("The selected issue is no longer listed on this host."))
        )),
    );

    let report = |checkouts| {
        Intent::Link(
            OTHER,
            Update::Feed(Feed::Home(home(
                HomeSync::Synced,
                Vec::from([issue_project(checkouts)]),
            ))),
        )
    };
    let (missing, effects) = run(
        laptop,
        Vec::from([report(Vec::new()), Intent::Form(FormIntent::Start)]),
    );
    assert_eq!(sent(&effects), []);
    assert_eq!(
        readiness(&missing),
        Some((
            Some(text("Sign in")),
            true,
            Some(text("The issue's project is not checked out on this host."))
        )),
    );

    let (ready, _) = run(
        missing,
        Vec::from([report(Vec::from([at("/src/alpha", Origin::Found)]))]),
    );
    assert_eq!(
        readiness(&ready),
        Some((Some(text("Sign in")), false, None))
    );
    let (_, effects) = run(ready, Vec::from([Intent::Form(FormIntent::Start)]));
    assert_eq!(
        sent(&effects),
        [CommandBody::StartSession {
            instance: id("claude"),
            at: StartAt::Issue(OPENED),
            model: None,
            effort: None,
        }]
    );
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Send { host, .. } if *host == OTHER
    )));
}

#[test]
fn a_project_shared_by_live_hosts_opens_its_issues_on_the_first() {
    let (typed, _) = run(
        two_homes(),
        Vec::from([Intent::NewIssue(NewIssueIntent::TypeTitle(
            ProjectId(text("alpha")),
            text("Remove"),
        ))]),
    );
    assert_eq!(
        projects(&typed)
            .and_then(|screen| screen.projects.into_iter().next())
            .and_then(|group| group.issues.create),
        Some(Intent::NewIssue(NewIssueIntent::Create(
            HOST,
            ProjectId(text("alpha")),
            text("task"),
        )))
    );
}

#[test]
fn a_refused_issue_is_the_notice_and_keeps_its_title()
-> Result<(), &'static str> {
    let alpha = || ProjectId(text("alpha"));
    let (typed, effects) = run(
        with_issues(),
        Vec::from([
            Intent::NewIssue(NewIssueIntent::TypeTitle(
                alpha(),
                text("Remove"),
            )),
            Intent::NewIssue(NewIssueIntent::Create(
                HOST,
                alpha(),
                text("task"),
            )),
        ]),
    );
    let reply = reply_of(&effects).ok_or("no reply")?;
    let (refused, _) = run(
        typed,
        Vec::from([Intent::Replied(reply, Err(text("offline")))]),
    );
    assert_eq!(
        projects(&refused).map(|screen| (
            screen.notice,
            screen
                .projects
                .into_iter()
                .map(|group| group.issues.new_title)
                .collect::<Vec<_>>()
        )),
        Some((Some(text("offline")), Vec::from([text("Remove")])))
    );
    Ok(())
}

#[test]
fn a_late_created_reply_cannot_open_an_issue_or_clear_a_new_title()
-> Result<(), &'static str> {
    let alpha = || ProjectId(text("alpha"));
    let (typed, effects) = run(
        with_issues(),
        Vec::from([
            Intent::NewIssue(NewIssueIntent::TypeTitle(
                alpha(),
                text("Remove"),
            )),
            Intent::NewIssue(NewIssueIntent::Create(
                HOST,
                alpha(),
                text("task"),
            )),
        ]),
    );
    let reply = reply_of(&effects).ok_or("no reply")?;
    let (on_issue, _) = run(
        typed,
        Vec::from([
            Intent::Show(Place::Issue(HOST, OPENED)),
            Intent::Issue(IssueIntent::TypeItem(text("docs"))),
        ]),
    );
    let answer = Intent::Replied(
        reply,
        Ok(CommandResult::IssueWritten {
            issue: CLOSED,
            warnings: Vec::new(),
        }),
    );
    let on_issue_before_reply = on_issue.clone();
    let (still_open, _) = run(on_issue.clone(), Vec::from([answer.clone()]));
    let (returned, _) = run(
        on_issue,
        Vec::from([
            Intent::Show(Place::Projects),
            Intent::NewIssue(NewIssueIntent::TypeTitle(
                alpha(),
                text("New draft"),
            )),
        ]),
    );
    let (still_typing, _) = run(returned.clone(), Vec::from([answer]));
    assert_eq!(
        issue_screen(&still_open),
        issue_screen(&on_issue_before_reply)
    );
    assert_eq!(projects(&still_typing), projects(&returned));
    Ok(())
}

#[test]
fn an_issue_marks_a_fields_word_and_sends_only_a_new_one() {
    let priority = |value: &str| {
        Intent::Issue(IssueIntent::SetField {
            field: text("priority"),
            value: text(value),
        })
    };
    let (_, effects) =
        run(on_issue(), Vec::from([priority("high"), priority("low")]));
    assert_eq!(
        (
            issue_screen(&on_issue()).map(|issue| issue
                .fields
                .into_iter()
                .flat_map(|picker| picker.picks)
                .filter(|pick| pick.selected)
                .map(|pick| pick.word)
                .collect::<Vec<_>>()),
            sent(&effects),
        ),
        (
            Some(Vec::from([text("high")])),
            Vec::from([CommandBody::ChangeIssue {
                issue: OPENED,
                change: IssueChange::Set {
                    field: text("priority"),
                    value: text("low"),
                },
            }]),
        )
    );
}

/// The issues view on desk, whose home has `flow` and alpha's issues.
fn under(flow: Flow, issues: Vec<Issue>) -> crate::State {
    let mut alpha = issue_project(Vec::from([at(ALPHA, Origin::Found)]));
    alpha.issues = issues;
    let mut known = home(HomeSync::Synced, Vec::from([alpha]));
    known.flow = Some(Box::new(flow));
    run(
        paired(),
        Vec::from([
            welcomed(HOST, "desk", claude(signed_in()), Some(known)),
            connection(HOST, LinkState::Live, None),
            Intent::Show(Place::Projects),
        ]),
    )
    .0
}

fn picks(words: &[&str], selected: &str) -> Vec<Pick> {
    words
        .iter()
        .map(|word| Pick {
            word: text(word),
            label: text(word),
            selected: *word == selected,
        })
        .collect()
}

#[test]
fn renamed_statuses_group_label_and_fill_the_picker_with_no_code_change()
-> Result<(), &'static str> {
    let mut renamed = flow();
    for (status, name) in renamed
        .statuses
        .iter_mut()
        .zip(["inbox", "queued", "doing", "checking", "shipped", "binned"])
    {
        status.name = text(name);
    }
    renamed.statuses.insert(
        2,
        Status {
            name: text("blocked"),
            role: Role::Idle,
            colour: None,
        },
    );
    let shown = under(
        renamed,
        Vec::from([
            issue(OPENED, "Sign in", "shipped"),
            issue(CLOSED, "Pair", "blocked"),
        ]),
    );
    let groups = projects(&shown)
        .and_then(|screen| screen.projects.into_iter().next())
        .map(|group| {
            group
                .issues
                .groups
                .into_iter()
                .map(|listed| (listed.status, listed.label, listed.rows.len()))
                .collect::<Vec<_>>()
        });
    assert_eq!(
        groups,
        Some(Vec::from([
            (text("blocked"), text("blocked"), 1),
            (text("shipped"), text("shipped"), 1),
        ])),
        "groups follow the flow's order, by its names"
    );
    let (on_issue, _) =
        run(shown, Vec::from([Intent::Show(Place::Issue(HOST, OPENED))]));
    let screen = issue_screen(&on_issue).ok_or("no issue screen")?;
    assert_eq!(
        screen.status,
        picks(
            &[
                "inbox", "queued", "blocked", "doing", "checking", "shipped",
                "binned"
            ],
            "shipped"
        )
    );
    let (_, effects) = run(
        on_issue,
        Vec::from([Intent::Issue(IssueIntent::SetStatus(text("checking")))]),
    );
    assert_eq!(
        sent(&effects),
        [CommandBody::ChangeIssue {
            issue: OPENED,
            change: moving_to("checking"),
        }]
    );
    Ok(())
}

#[test]
fn a_new_kind_and_field_bring_their_own_label_and_pickers()
-> Result<(), &'static str> {
    let mut grown = flow();
    grown
        .fields
        .push(enumerated("effort", &["small", "large"], Some("small")));
    grown.kinds.push(kind(
        "chore",
        "Chore",
        Vec::from([enumerated("area", &["ci", "docs"], None)]),
    ));
    let mut chore = issue(OPENED, "Tidy", "open");
    chore.kind = text("chore");
    chore.fields.insert(text("area"), Datum::Text(text("docs")));
    let shown = under(grown, Vec::from([chore]));
    let row = projects(&shown)
        .and_then(|screen| screen.projects.into_iter().next())
        .and_then(|group| group.issues.groups.into_iter().next())
        .and_then(|listed| listed.rows.into_iter().next())
        .map(|listed| listed.detail);
    assert_eq!(
        row,
        Some(text("high · app · small · docs · 1/2 done")),
        "a field left out reads as its default"
    );
    let (on_issue, _) =
        run(shown, Vec::from([Intent::Show(Place::Issue(HOST, OPENED))]));
    let screen = issue_screen(&on_issue).ok_or("no issue screen")?;
    assert_eq!(
        screen.fields,
        Vec::from([
            FieldPicker {
                field: text("priority"),
                picks: picks(&["low", "normal", "high", "urgent"], "high"),
            },
            FieldPicker {
                field: text("effort"),
                picks: picks(&["small", "large"], "small"),
            },
            FieldPicker {
                field: text("area"),
                picks: picks(&["ci", "docs"], "docs"),
            },
        ]),
        "a field the issue leaves out shows its default"
    );
    assert!(screen.detail.starts_with("Chore · "), "{}", screen.detail);
    Ok(())
}

#[test]
fn an_accepted_item_or_note_empties_its_field() -> Result<(), &'static str> {
    let item = Intent::Issue(IssueIntent::AddItem);
    let note = Intent::Issue(IssueIntent::AddNote);
    let (pending, effects) = run(
        on_issue(),
        Vec::from([
            Intent::Issue(IssueIntent::TypeItem(text("docs"))),
            item.clone(),
            Intent::Issue(IssueIntent::TypeNote(text("halfway"))),
            note.clone(),
        ]),
    );
    let fields = |state: &crate::State| {
        issue_screen(state).map(|issue| (issue.item, issue.note))
    };
    assert_eq!(fields(&pending), Some((text("docs"), text("halfway"))));
    let (blocked, duplicates) = run(pending, Vec::from([item, note]));
    assert_eq!(sent(&duplicates), []);

    let accept = |change| {
        effects.iter().find_map(|effect| match effect {
            Effect::Send {
                body:
                    CommandBody::ChangeIssue {
                        change: submitted, ..
                    },
                reply,
                ..
            } if submitted == &change => Some(Intent::Replied(
                reply.clone(),
                Ok(CommandResult::Accepted),
            )),
            _ => None,
        })
    };
    let (item_accepted, _) = run(
        blocked,
        Vec::from([accept(adding("docs")).ok_or("item was not submitted")?]),
    );
    assert_eq!(
        fields(&item_accepted),
        Some((String::new(), text("halfway")))
    );
    let (both_accepted, _) = run(
        item_accepted,
        Vec::from([accept(IssueChange::Note(text("halfway")))
            .ok_or("note was not submitted")?]),
    );
    assert_eq!(fields(&both_accepted), Some((String::new(), String::new())));
    Ok(())
}

#[test]
fn the_edit_form_saves_only_with_a_title_and_cancel_closes_it() {
    let edit = |intents: Vec<IssueIntent>| {
        run(
            on_issue(),
            core::iter::once(IssueIntent::Edit)
                .chain(intents)
                .map(Intent::Issue)
                .collect(),
        )
    };
    let editing = |state: &crate::State| {
        issue_screen(state).map(|issue| {
            issue
                .editing
                .map(|editing| (editing.title, editing.save_disabled))
        })
    };
    let (opened, _) = edit(Vec::new());
    let (untitled, unsaved) = edit(Vec::from([
        IssueIntent::TypeTitle(text("  ")),
        IssueIntent::Save,
    ]));
    let (cancelled, _) = edit(Vec::from([IssueIntent::Cancel]));
    assert_eq!(
        (
            editing(&opened),
            editing(&untitled),
            sent(&unsaved),
            editing(&cancelled),
        ),
        (
            Some(Some((text("Sign in"), false))),
            Some(Some((text("  "), true))),
            Vec::new(),
            Some(None),
        )
    );
}

#[test]
fn leaving_an_issue_drops_its_drafts() {
    let (typed, _) = run(
        on_issue(),
        Vec::from([
            Intent::Issue(IssueIntent::TypeItem(text("docs"))),
            Intent::Show(Place::Projects),
            Intent::Show(Place::Issue(HOST, OPENED)),
        ]),
    );
    assert_eq!(
        issue_screen(&typed).map(|issue| issue.item),
        Some(String::new())
    );
}

#[test]
fn a_change_written_with_a_warning_shows_the_warning()
-> Result<(), &'static str> {
    let (moved, effects) = run(
        on_issue(),
        Vec::from([Intent::Issue(IssueIntent::SetStatus(text("active")))]),
    );
    let reply = reply_of(&effects).ok_or("no reply")?;
    let (warned, _) = run(
        moved,
        Vec::from([Intent::Replied(
            reply,
            Ok(CommandResult::IssueWritten {
                issue: OPENED,
                warnings: Vec::from([text("lacks the section \"What\"")]),
            }),
        )]),
    );
    assert_eq!(
        issue_screen(&warned).and_then(|issue| issue.notice),
        Some(text("lacks the section \"What\""))
    );
    Ok(())
}
