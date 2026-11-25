use std::collections::BTreeMap;

use crate::{Draws, ProjectId};

use super::{
    Checkout, Distance, Fetch, New, Scanning, Upstream, local, placed,
    profiles, regenerated, report, scan, sync, table, unmatched, warnings,
};
use crate::checkout::Probe;
use crate::project::{Project, parse};

const ROOT: &str = "fc27e50efb661d7ef4a45d3edbe0234ff0a6fbd5";
const OTHER: &str = "d2cef7849f3ce7cc16f17979e0acb8c21d9aed2a";

fn project(id: &str, name: &str, root: &str, remote: Option<&str>) -> Project {
    Project {
        id: ProjectId::parse(id).unwrap_or(ProjectId::new(0)),
        name: String::from(name),
        title: String::from(name),
        remote: remote.map(String::from),
        root: String::from(root),
        profiles: Vec::new(),
        provides: Vec::new(),
    }
}

fn repository(path: &str, root: &str, remote: Option<&str>) -> Probe {
    Probe {
        path: String::from(path),
        roots: Vec::from([String::from(root)]),
        remote: remote.map(String::from),
    }
}

#[test]
fn a_repository_is_named_by_its_path_from_the_home() {
    let found = ["/home/me/projects", "/home/me/projects/apps/x", "/srv/y"]
        .map(|path| repository(path, ROOT, None))
        .to_vec();
    let paths: Vec<String> = local("/home/me/projects/", found)
        .into_iter()
        .map(|probe| probe.path)
        .collect();
    assert_eq!(paths, [".", "apps/x", "/srv/y"]);
}

#[test]
fn a_checkout_has_the_project_s_root_and_its_remote_in_any_form() {
    let projects = [
        project(
            "cnd000",
            "conductor",
            ROOT,
            Some("https://github.com/JimLundin/conductor.git"),
        ),
        project("s0p000", "sop", OTHER, None),
    ];
    let probes = [
        repository(
            "apps/conductor",
            ROOT,
            Some("git@GitHub.com:JimLundin/conductor"),
        ),
        repository("fork", ROOT, Some("https://github.com/someone/conductor")),
        repository("sop", OTHER, None),
        repository("unrelated", OTHER, Some("https://example.com/x.git")),
    ];
    let found = placed(&projects, &probes);
    let paths: Vec<(&str, Vec<&str>)> = found
        .iter()
        .map(|placed| {
            (
                placed.name.as_str(),
                placed.checkouts.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        paths,
        [
            ("conductor", Vec::from(["apps/conductor"])),
            ("sop", Vec::from(["sop"])),
        ]
    );
    let left: Vec<String> = unmatched(&projects, &probes)
        .into_iter()
        .map(|probe| probe.path)
        .collect();
    assert_eq!(left, ["fork", "unrelated"]);
}

#[test]
fn the_report_gives_each_checkout_s_branch_and_changes_by_project_name() {
    let projects = [
        project("zeta00", "zeta", OTHER, None),
        project("a1fa00", "alpha", ROOT, None),
    ];
    let found = placed(&projects, &[repository("alpha", ROOT, None)]);
    let states = BTreeMap::from([(
        String::from("alpha"),
        Some(Checkout::parse(
            "# branch.oid 1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b\n\
             # branch.head main\n\
             # branch.upstream origin/main\n\
             # branch.ab +1 -0\n\
             1 .M N... 100644 100644 100644 1a2b 1a2b a.rs\n\
             ? b.rs\n",
        )),
    )]);
    assert_eq!(
        report(&found, &states),
        "alpha                  alpha: main...origin/main [ahead 1]  [2 changed]\n\
         zeta                   not checked out here\n"
    );
}

#[test]
fn a_checkout_s_state_reads_its_commit_branch_upstream_distance_and_changes() {
    assert_eq!(
        Checkout::parse(
            "# branch.oid 1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b\n\
             # branch.head main\n\
             # branch.upstream origin/main\n\
             # branch.ab +2 -3\n\
             ? b.rs\n",
        ),
        Checkout {
            commit: Some(String::from(
                "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b"
            )),
            branch: Some(String::from("main")),
            upstream: Some(Upstream {
                name: String::from("origin/main"),
                distance: Some(Distance {
                    ahead: 2,
                    behind: 3
                }),
            }),
            changed: 1,
        }
    );
}

#[test]
fn a_detached_checkout_without_commits_or_upstream_has_none_of_them() {
    let state =
        Checkout::parse("# branch.oid (initial)\n# branch.head (detached)\n");
    assert_eq!(state, Checkout::default());
}

#[test]
fn only_a_clean_checkout_on_the_default_branch_level_with_origin_is_level() {
    let level = "# branch.oid 1a2b3c4d\n# branch.head main\n\
                 # branch.upstream origin/main\n# branch.ab +0 -0\n";
    assert!(Checkout::parse(level).is_level("main"));
    for off in [
        "# branch.oid 1a2b3c4d\n# branch.head main\n\
         # branch.upstream origin/main\n# branch.ab +0 -1\n",
        "# branch.oid 1a2b3c4d\n# branch.head main\n\
         # branch.upstream origin/main\n",
        "# branch.oid 1a2b3c4d\n# branch.head work\n\
         # branch.upstream origin/work\n# branch.ab +0 -0\n",
        "# branch.oid 1a2b3c4d\n# branch.head main\n\
         # branch.upstream fork/main\n# branch.ab +0 -0\n",
        "# branch.oid 1a2b3c4d\n# branch.head main\n\
         # branch.upstream origin/main\n# branch.ab +0 -0\n? new.py\n",
    ] {
        assert!(!Checkout::parse(off).is_level("main"), "{off}");
    }
    assert!(!Checkout::parse(level).is_level("trunk"));
}

#[test]
fn a_checkout_s_state_reads_as_git_s_short_status_branch_line() {
    let said = |porcelain: &str| Checkout::parse(porcelain).to_string();
    assert_eq!(
        said(
            "# branch.oid 1a\n# branch.head main\n\
              # branch.upstream origin/main\n# branch.ab +1 -2\n"
        ),
        "main...origin/main [ahead 1, behind 2]"
    );
    assert_eq!(
        said(
            "# branch.oid 1a\n# branch.head main\n\
              # branch.upstream origin/main\n"
        ),
        "main...origin/main [gone]"
    );
    assert_eq!(said("# branch.oid 1a\n# branch.head main\n"), "main");
    assert_eq!(
        said("# branch.oid 1a\n# branch.head (detached)\n"),
        "HEAD (no branch)"
    );
}

#[test]
fn sync_clones_each_project_with_a_remote_and_no_checkout_into_its_name() {
    let projects = [
        project("a00000", "a", ROOT, Some("https://example.com/a.git")),
        project("b00000", "b", OTHER, Some("https://example.com/b.git")),
        project("s0p000", "sop", OTHER, None),
    ];
    let found = placed(
        &projects,
        &[repository("a", ROOT, Some("https://example.com/a"))],
    );
    assert_eq!(
        sync(&found),
        [
            Fetch {
                said: String::from("clone https://example.com/b.git into b\n"),
                clone: Some((
                    String::from("https://example.com/b.git"),
                    String::from("b")
                )),
            },
            Fetch {
                said: String::from("skip sop: local only, no remote\n"),
                clone: None,
            },
        ]
    );
}

#[test]
fn a_repository_s_profiles_follow_the_files_at_its_top() {
    let names = ["Cargo.toml", "README.md", "package.json"]
        .map(String::from)
        .to_vec();
    assert_eq!(profiles(&names), ["rust", "typescript"]);
    assert_eq!(profiles(&[String::from("pyproject.toml")]), ["python"]);
    assert_eq!(profiles(&[]), Vec::<String>::new());
}

#[test]
fn the_table_lists_each_project_by_name_with_its_remote() {
    let projects = [
        project("s0p000", "sop", OTHER, None),
        project("a00000", "home", ROOT, Some("https://example.com/home.git")),
    ];
    assert_eq!(
        table(&projects),
        "<!-- repos:start (generated by home repos scan from projects/) -->\n\
         | Project | Remote |\n\
         |---|---|\n\
         | `home` | https://example.com/home.git |\n\
         | `sop` | local only |\n\
         <!-- repos:end -->\n"
    );
}

#[test]
fn the_table_replaces_the_lines_between_its_markers_only() {
    let doc = "# Home\n\n<!-- repos:start (old) -->\n| old |\n<!-- repos:end -->\nafter\n";
    assert_eq!(
        regenerated(doc, "<!-- repos:start -->\nnew\n<!-- repos:end -->\n"),
        "# Home\n\n<!-- repos:start -->\nnew\n<!-- repos:end -->\nafter\n"
    );
    assert_eq!(regenerated("no table\n", "table\n"), "no table\n");
}

fn draws(given: &[u64]) -> Draws {
    core::array::from_fn(|at| {
        given.get(at).or_else(|| given.last()).copied().unwrap_or(0)
    })
}

fn new(probe: Probe, names: &[&str], given: &[u64]) -> New {
    New {
        probe,
        names: names.iter().copied().map(String::from).collect(),
        draws: draws(given),
    }
}

fn scanning(new: Vec<New>) -> Scanning {
    let home = project("a00000", "home", ROOT, None);
    Scanning {
        home: String::from("projects"),
        projects: Vec::from([home]),
        new,
        problems: Vec::new(),
        doc: Ok(String::from(
            "intro\n<!-- repos:start -->\n<!-- repos:end -->\n",
        )),
    }
}

#[test]
fn scan_registers_a_new_repository_named_after_its_folder() -> Result<(), String>
{
    let done = scan(scanning(Vec::from([new(
        repository("apps/My App", OTHER, Some("https://example.com/app.git")),
        &["Cargo.toml"],
        &[5],
    )])))?;
    assert_eq!(
        done.writes,
        [
            (
                String::from("projects/000005/project.toml"),
                String::from(
                    "name = \"my-app\"\ntitle = \"My App\"\n\
                     remote = \"https://example.com/app.git\"\n\
                     root = \"d2cef7849f3ce7cc16f17979e0acb8c21d9aed2a\"\n\
                     profiles = [\"rust\"]\n"
                )
            ),
            (
                String::from("AGENTS.md"),
                String::from(
                    "intro\n\
                     <!-- repos:start (generated by home repos scan from projects/) -->\n\
                     | Project | Remote |\n|---|---|\n\
                     | `home` | local only |\n\
                     | `my-app` | https://example.com/app.git |\n\
                     <!-- repos:end -->\n"
                )
            ),
        ]
    );
    assert_eq!(
        done.said,
        "added my-app as projects/000005 from apps/My App\nhome\nmy-app\n"
    );
    Ok(())
}

#[test]
fn scan_draws_again_for_an_id_the_home_holds_or_has_just_minted()
-> Result<(), String> {
    let mut input = scanning(Vec::from([
        new(
            repository(".", ROOT, Some("https://example.com/home")),
            &[],
            &[0, 3, 3],
        ),
        new(repository("sop", OTHER, None), &[], &[0, 3, 7]),
    ]));
    input.projects = Vec::from([project("000000", "home", ROOT, None)]);
    let done = scan(input)?;
    let folders: Vec<&str> =
        done.writes.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(
        folders,
        [
            "projects/000003/project.toml",
            "projects/000007/project.toml",
            "AGENTS.md"
        ]
    );
    let titles: Vec<String> = done
        .writes
        .iter()
        .zip(["000003", "000007"])
        .filter_map(|((_, text), id)| parse(id, text).ok())
        .map(|project| project.title)
        .collect();
    assert_eq!(titles, ["projects", "sop"]);
    Ok(())
}

#[test]
fn scan_gives_up_when_every_draw_is_held() {
    let mut input =
        scanning(Vec::from([new(repository("sop", OTHER, None), &[], &[0])]));
    input.projects = Vec::from([project("000000", "home", ROOT, None)]);
    assert_eq!(
        scan(input),
        Err(String::from("no fresh project id for sop"))
    );
}

#[test]
fn scan_registers_nothing_while_a_project_folder_does_not_read() {
    let mut input =
        scanning(Vec::from([new(repository("sop", OTHER, None), &[], &[1])]));
    input.problems = Vec::from([String::from("projects/x: no project.toml")]);
    assert_eq!(
        scan(input),
        Err(String::from(
            "scan registers nothing while the home's projects don't read:\n\
             projects/x: no project.toml"
        ))
    );
}

#[test]
fn each_listing_problem_is_a_warning_line() {
    let problems = Vec::from([
        String::from("projects/x: bad"),
        String::from("projects/y: bad"),
    ]);
    assert_eq!(
        warnings(&problems),
        "home: projects/x: bad\nhome: projects/y: bad\n"
    );
}
