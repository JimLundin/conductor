use std::collections::BTreeMap;

use proto::{
    Checkout, Cloning, Home, HomeSync, HostPath, Origin, ProjectId, Rejection,
    StartAt,
};

use crate::checkout::{Probe, Root, candidates, matched, normal, probed};
use crate::project::{Listing, Project, listed, parse};
use crate::sync::{Git, Sync, next};
use crate::{Found, Stored, cloned, place, remote_of, view};

const ROOT: &str = "fc27e50efb661d7ef4a45d3edbe0234ff0a6fbd5";
const OTHER: &str = "d2cef7849f3ce7cc16f17979e0acb8c21d9aed2a";
const REMOTE: &str = "https://github.com/JimLundin/conductor.git";

fn id(name: &str) -> ProjectId {
    ProjectId(String::from(name))
}

fn project(name: &str, remote: Option<&str>) -> Project {
    Project {
        id: id(name),
        name: String::from(name),
        title: format!("The {name}"),
        remote: remote.map(String::from),
        root: String::from(ROOT),
        profiles: Vec::from([String::from("rust")]),
    }
}

fn checkout(path: &str, origin: Origin) -> Checkout {
    Checkout {
        path: HostPath(String::from(path)),
        origin,
    }
}

fn probe(path: &str, roots: &[&str], remote: Option<&str>) -> Probe {
    Probe {
        path: String::from(path),
        roots: roots.iter().copied().map(String::from).collect(),
        remote: remote.map(String::from),
    }
}

#[test]
fn a_project_toml_gives_every_field_and_skips_ones_it_doesnt_know() {
    let folder = "01m7r5n23kp17wyfj138e0bcnf";
    let text = format!(
        "name = \"conductor\"\ntitle = \"Conductor\"\nremote = \"{REMOTE}\"\n\
         root = \"{ROOT}\"\nbranch = \"main\"\nprofiles = [\"rust\"]\n\
         path = \"apps/conductor\"\nissues = 3\n"
    );
    assert_eq!(
        parse(folder, &text),
        Ok(Project {
            id: id(folder),
            title: String::from("Conductor"),
            ..project("conductor", Some(REMOTE))
        })
    );
    let local =
        format!("name = \"sop\"\ntitle = \"The sop\"\nroot = \"{ROOT}\"\n");
    assert_eq!(
        parse("sop", &local),
        Ok(Project {
            profiles: Vec::new(),
            ..project("sop", None)
        })
    );
}

#[test]
fn a_project_toml_that_wont_do_says_why() {
    let fields = |root: &str| {
        format!("name = \"x\"\ntitle = \"X\"\nroot = \"{root}\"\n")
    };
    let good = fields(ROOT);
    assert_eq!(
        [
            parse("Sales", &good),
            parse("", &good),
            parse("-x", &good),
            parse("a.b", &good),
            parse("x", "name = \"x\"\n"),
            parse("x", &fields("FC27E50EFB661D7EF4A45D3EDBE0234FF0A6FBD5")),
            parse("x", &fields("fc27e50")),
            parse("x", &fields(&"g".repeat(40))),
        ]
        .map(|parsed| parsed.err().map(|err| first_words(&err))),
        [
            Some(String::from("the folder's name")),
            Some(String::from("the folder's name")),
            Some(String::from("the folder's name")),
            Some(String::from("the folder's name")),
            Some(String::from("project.toml: TOML parse")),
            Some(String::from("root is not")),
            Some(String::from("root is not")),
            Some(String::from("root is not")),
        ]
    );
    assert!(parse("x-2", &good).is_ok());
    assert!(parse("x", &fields(&"a".repeat(64))).is_ok());
}

fn first_words(text: &str) -> String {
    text.split(' ').take(3).collect::<Vec<&str>>().join(" ")
}

#[test]
fn a_listing_is_by_id_and_names_the_folders_that_arent_projects() {
    let text = |name: &str| {
        format!("name = \"{name}\"\ntitle = \"{name}\"\nroot = \"{ROOT}\"\n")
    };
    let Listing { projects, problems } = listed(Vec::from([
        (String::from("zeta"), Ok(text("zeta"))),
        (String::from("alpha"), Ok(text("alpha"))),
        (String::from("Bad"), Ok(text("bad"))),
        (String::from("gone"), Err(String::from("no project.toml"))),
        (String::from("one"), Ok(text("same"))),
        (String::from("two"), Ok(text("same"))),
    ]));
    assert_eq!(
        projects
            .iter()
            .map(|known| known.id.0.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "zeta"]
    );
    assert_eq!(
        problems,
        [
            "projects/Bad: the folder's name is not lower-case letters, digits and dashes",
            "projects/gone: no project.toml",
            "projects/one: another project is also called \"same\"",
            "projects/two: another project is also called \"same\"",
        ]
    );
}

/// A rename is an edit of `project.toml`: the project keeps its id, so its
/// checkouts, which are kept by id, stay its own.
#[test]
fn a_renamed_project_keeps_its_id_and_its_checkouts() {
    let folder = "01m7r5n23kp17wyfj138e0bcnf";
    let renamed = parse(
        folder,
        &format!(
            "name = \"orchestra\"\ntitle = \"Orchestra\"\nremote = \"{REMOTE}\"\n\
             root = \"{ROOT}\"\n"
        ),
    );
    let found = BTreeMap::from([(
        id(folder),
        Vec::from([checkout("/a", Origin::Found)]),
    )]);
    let shown = renamed.clone().map(|project| {
        view(
            REMOTE,
            &Sync::new(),
            &[project],
            Found {
                checkouts: &found,
                cloning: &BTreeMap::new(),
                issues: &BTreeMap::new(),
                unpushed: 0,
            },
        )
        .projects
    });
    assert_eq!(
        (
            renamed.map(|project| (project.id, project.name, project.title)),
            shown.map(|projects| {
                projects
                    .into_iter()
                    .map(|project| project.checkouts)
                    .collect::<Vec<_>>()
            }),
            Stored::kept(None, &found).checkouts.get("/a"),
        ),
        (
            Ok((
                id(folder),
                String::from("orchestra"),
                String::from("Orchestra"),
            )),
            Ok(Vec::from([Vec::from([checkout("/a", Origin::Found)])])),
            Some(&id(folder)),
        )
    );
}

#[test]
fn candidates_are_known_paths_then_each_root_s_directories_then_clones_each_once()
 {
    let projects = [project("conductor", Some(REMOTE)), project("sop", None)];
    let roots = [
        Root {
            path: String::from("/home/me/projects/"),
            entries: Vec::from([
                String::from(".cache"),
                String::from(".cache/sop"),
                String::from("apps"),
                String::from("apps/.git"),
                String::from("apps/conductor"),
                String::from("conductor"),
            ]),
        },
        Root {
            path: String::from("/srv"),
            entries: Vec::from([String::from("sop")]),
        },
    ];
    assert_eq!(
        candidates(
            &roots,
            &projects,
            &[String::from("/home/me/projects/conductor")],
            "/data/projects",
        ),
        [
            "/home/me/projects/conductor",
            "/home/me/projects/apps",
            "/home/me/projects/apps/conductor",
            "/srv/sop",
            "/data/projects/conductor",
            "/data/projects/sop",
        ]
    );
}

#[test]
fn only_the_top_of_a_repository_with_a_commit_is_probed() {
    let path = "/home/me/conductor";
    assert_eq!(
        [
            probed(
                path,
                Some("/home/me/conductor\n"),
                Some("a\nb\n"),
                Some(" url\n")
            ),
            probed(path, Some(path), Some("a"), Some("")),
            probed(path, Some(path), Some("a"), None),
            probed(path, Some("/home/me"), Some("a"), None),
            probed(path, None, Some("a"), None),
            probed(path, Some(path), None, None),
            probed(path, Some(path), Some(""), None),
        ],
        [
            Some(probe(path, &["a", "b"], Some("url"))),
            Some(probe(path, &["a"], None)),
            Some(probe(path, &["a"], None)),
            None,
            None,
            None,
            None,
        ]
    );
}

#[test]
fn a_checkout_has_the_root_and_the_remote_however_it_is_spelled() {
    let projects = [
        project("conductor", Some(REMOTE)),
        project("sop", None),
        project("other", Some("https://github.com/JimLundin/other.git")),
    ];
    let probes = [
        probe(
            "/a",
            &[ROOT],
            Some("git@github.com:JimLundin/conductor.git"),
        ),
        probe(
            "/b",
            &[OTHER, ROOT],
            Some("https://GitHub.com/jimlundin/conductor/"),
        ),
        probe("/c", &[OTHER], Some(REMOTE)),
        probe(
            "/d",
            &[ROOT],
            Some("https://github.com/someone/conductor.git"),
        ),
        probe("/e", &[ROOT], None),
        probe("/a", &[ROOT], Some(REMOTE)),
        probe("/data/projects/conductor", &[ROOT], Some(REMOTE)),
        probe("/data/projects/sop", &[OTHER], None),
    ];
    assert_eq!(
        matched(&projects, &probes, "/data/projects"),
        BTreeMap::from([
            (
                id("conductor"),
                Vec::from([
                    checkout("/a", Origin::Found),
                    checkout("/b", Origin::Found),
                    checkout("/data/projects/conductor", Origin::Cloned),
                ])
            ),
            (id("sop"), Vec::from([checkout("/e", Origin::Found)])),
        ])
    );
}

#[test]
fn a_remote_is_normal_without_scheme_user_suffix_or_case() {
    assert_eq!(
        [
            "https://github.com/JimLundin/conductor.git",
            "git@github.com:JimLundin/conductor.git",
            "ssh://git@github.com/JimLundin/conductor",
            " https://github.com/JimLundin/conductor/ ",
            "file:///srv/Home.git",
            "/srv/home",
            "https://host:8443/x",
        ]
        .map(normal),
        [
            "github.com/jimlundin/conductor",
            "github.com/jimlundin/conductor",
            "github.com/jimlundin/conductor",
            "github.com/jimlundin/conductor",
            "/srv/home",
            "/srv/home",
            "host:8443/x",
        ]
    );
}

#[test]
fn the_host_clones_without_a_clone_pulls_with_one_and_replays_its_changes() {
    assert_eq!(
        [next(false, false), next(true, false), next(true, true)],
        [Git::Clone, Git::Pull, Git::Replay]
    );
    assert_eq!(next(false, true), Git::Clone);
}

#[test]
fn a_sync_succeeds_falls_behind_or_fails_and_keeps_its_last_success() {
    let reason = || String::from("could not resolve host");
    let first = Sync::default();
    assert_eq!(first, Sync::new());
    assert_eq!(first.status, HomeSync::Syncing);
    let failed = first.after(Err(reason()), false, 10);
    assert_eq!(
        failed,
        Sync {
            synced: None,
            attempted: Some(10),
            status: HomeSync::Failed { reason: reason() },
        }
    );
    let synced = failed.after(Ok(()), true, 20);
    assert_eq!(
        synced,
        Sync {
            synced: Some(20),
            attempted: Some(20),
            status: HomeSync::Synced,
        }
    );
    assert_eq!(
        synced.after(Err(reason()), true, 30),
        Sync {
            synced: Some(20),
            attempted: Some(30),
            status: HomeSync::Behind { reason: reason() },
        }
    );
}

#[test]
fn a_read_pulls_first_unless_the_host_tried_within_the_bound() {
    let tried = Sync::new().after(Ok(()), true, 100);
    assert_eq!(
        [
            Sync::new().due(0, 30),
            tried.due(129, 30),
            tried.due(130, 30),
            tried.due(50, 30),
        ],
        [true, false, true, false]
    );
}

fn home(checkouts: &[&str]) -> Home {
    view(
        REMOTE,
        &Sync::new().after(Ok(()), true, 5),
        &[project("conductor", Some(REMOTE)), project("sop", None)],
        Found {
            checkouts: &BTreeMap::from([(
                id("conductor"),
                checkouts
                    .iter()
                    .map(|path| checkout(path, Origin::Found))
                    .collect(),
            )]),
            cloning: &BTreeMap::from([(
                id("sop"),
                Cloning::Failed {
                    reason: String::from("no"),
                },
            )]),
            issues: &BTreeMap::new(),
            unpushed: 2,
        },
    )
}

#[test]
fn the_view_gives_each_project_its_checkouts_and_its_clone() {
    let shown = home(&["/a", "/b"]);
    assert_eq!(
        (
            shown.remote.as_str(),
            shown.synced,
            &shown.sync,
            shown.unpushed
        ),
        (REMOTE, Some(5), &HomeSync::Synced, 2)
    );
    assert_eq!(
        shown.projects,
        [
            proto::Project {
                id: id("conductor"),
                name: String::from("conductor"),
                title: String::from("The conductor"),
                remote: Some(String::from(REMOTE)),
                profiles: Vec::from([String::from("rust")]),
                checkouts: Vec::from([
                    checkout("/a", Origin::Found),
                    checkout("/b", Origin::Found),
                ]),
                cloning: Cloning::Idle,
                issues: Vec::new(),
            },
            proto::Project {
                id: id("sop"),
                name: String::from("sop"),
                title: String::from("The sop"),
                remote: None,
                profiles: Vec::from([String::from("rust")]),
                checkouts: Vec::new(),
                cloning: Cloning::Failed {
                    reason: String::from("no"),
                },
                issues: Vec::new(),
            },
        ]
    );
}

#[test]
fn a_session_starts_in_a_directory_or_a_projects_first_checkout() {
    let directory = HostPath(String::from("/work"));
    let shown = home(&["/a", "/b"]);
    let unchecked = home(&[]);
    assert_eq!(
        [
            place(StartAt::Directory(directory.clone()), None),
            place(StartAt::Project(id("conductor")), Some(&shown)),
            place(StartAt::Project(id("conductor")), None),
            place(StartAt::Project(id("nope")), Some(&shown)),
            place(StartAt::Project(id("conductor")), Some(&unchecked)),
        ]
        .map(|placed| placed.map(|at| at.directory)),
        [
            Ok(directory),
            Ok(HostPath(String::from("/a"))),
            Err(Rejection::NoHome),
            Err(Rejection::UnknownProject),
            Err(Rejection::NotCheckedOut),
        ]
    );
}

#[test]
fn only_a_known_project_with_a_remote_clones() {
    let projects = [project("conductor", Some(REMOTE)), project("sop", None)];
    assert_eq!(
        [
            remote_of(&projects, &id("conductor")),
            remote_of(&projects, &id("sop")),
            remote_of(&projects, &id("nope")),
        ],
        [
            Ok(String::from(REMOTE)),
            Err(Rejection::NoRemote),
            Err(Rejection::UnknownProject),
        ]
    );
    assert_eq!(
        [cloned(Ok(())), cloned(Err(String::from("denied")))],
        [
            Cloning::Idle,
            Cloning::Failed {
                reason: String::from("denied")
            }
        ]
    );
}

#[test]
fn the_store_keeps_the_remote_unless_given_one_and_every_checkout() {
    let stored = Stored::kept(
        Some(String::from("kept")),
        &BTreeMap::from([
            (
                id("a"),
                Vec::from([
                    checkout("/a2", Origin::Found),
                    checkout("/a", Origin::Cloned),
                ]),
            ),
            (id("b"), Vec::from([checkout("/b", Origin::Found)])),
        ]),
    );
    assert_eq!(
        stored.checkouts,
        BTreeMap::from([
            (String::from("/a"), id("a")),
            (String::from("/a2"), id("a")),
            (String::from("/b"), id("b")),
        ])
    );
    assert_eq!(
        [
            stored.remote(Some(String::from("given"))),
            stored.remote(None),
            Stored::default().remote(None),
        ],
        [
            Some(String::from("given")),
            Some(String::from("kept")),
            None
        ]
    );
    assert_eq!(stored.known(), ["/a", "/a2", "/b"]);
}
